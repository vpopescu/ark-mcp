use std::sync::Arc;
use tempfile::TempDir;

use ark::server::roles::Role;
use ark::{
    config::models::{AuthConfig, IdentityProviderConfig, SessionConfig},
    server::{auth, service::create_api_router},
    state::{ApplicationState, ArkState},
};
use axum::{
    Router,
    body::Body,
    extract::Request,
    http::{Method, StatusCode, header},
    middleware,
    response::IntoResponse,
    routing::{get, post},
};
use tower::ServiceExt;

/// Creates a test AuthState with database backing for proper session functionality
async fn create_test_auth_state(
    auth_cfg: Option<AuthConfig>,
) -> (auth::AuthState, Option<TempDir>) {
    if let Some(config) = auth_cfg {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let app_state = Arc::new(ArkState::default());

        // Initialize database using the persist module
        let database = ark::server::persist::Database::with_path(&db_path).unwrap();
        {
            let mut db_guard = app_state.database.write().unwrap();
            *db_guard = Some(database);
        }

        let auth_state = auth::AuthState::new_with_state(&Some(config), app_state, None)
            .await
            .unwrap();

        (auth_state, Some(temp_dir))
    } else {
        // For disabled auth, we can use the simple constructor
        let app_state = Arc::new(ArkState::default());
        let auth_state = auth::AuthState::new_with_state(&None, app_state, None)
            .await
            .unwrap();
        (auth_state, None)
    }
}

// Build a disabled auth state (no provider).
async fn disabled_auth_state() -> Arc<auth::AuthState> {
    let (auth_state, _temp_dir) = create_test_auth_state(None).await;
    Arc::new(auth_state)
}

/// Helper to build a test router that mirrors the management server routes we want to
/// validate, and applies the same path-based auth middleware used in production.
async fn build_test_router(auth_state: Arc<auth::AuthState>, app_state: Arc<ArkState>) -> Router {
    // Basic application routes used in tests
    let app = Router::new()
        // SSE endpoints
        .route("/sse", axum::routing::any(|| async { StatusCode::OK }))
        .route("/message", post(|| async { StatusCode::OK }))
        // Plugin API
        .nest("/api", create_api_router(app_state))
        // Simple MCP placeholder
        .route("/mcp", axum::routing::any(|| async { StatusCode::OK }))
        // Admin/console and misc endpoints
        .route("/admin", get(|| async { StatusCode::OK }))
        .route("/metrics", get(|| async { StatusCode::OK }))
        .route("/livez", get(|| async { StatusCode::OK }))
        .route("/readyz", get(|| async { StatusCode::OK }));

    // Apply same middleware logic as in service.rs
    let auth_clone = auth_state.clone();
    app.layer(middleware::from_fn(
        move |req: Request<Body>, next: axum::middleware::Next| {
            let auth = auth_clone.clone();
            async move {
                if !auth.enabled {
                    return next.run(req).await;
                }

                let path = req.uri().path();
                let protected = auth::path_requires_auth(path);
                if !protected {
                    return next.run(req).await;
                }

                // Extract headers
                let cookie_str = req
                    .headers()
                    .get(header::COOKIE)
                    .and_then(|h| h.to_str().ok());

                // Check session cookie
                if let Some(cookie) = cookie_str
                    && let Some(principal) =
                        auth::extract_session_user_from_cookie(&auth, cookie).await
                {
                    // Check if this path requires admin privileges
                    if auth::path_requires_admin(path) && !principal.is_admin {
                        let body = axum::Json(serde_json::json!({
                            "error": "admin_required",
                        }));
                        let mut resp = body.into_response();
                        *resp.status_mut() = StatusCode::FORBIDDEN;
                        return resp;
                    }

                    let mut req = req;
                    req.extensions_mut().insert(principal);
                    return next.run(req).await;
                }
                // For now, skip bearer token validation in tests
                // TODO: Add bearer token validation if needed

                let body = axum::Json(serde_json::json!({
                    "error": "unauthorized",
                }));
                let mut resp = body.into_response();
                *resp.status_mut() = StatusCode::UNAUTHORIZED;
                resp
            }
        },
    ))
}

#[tokio::test]
async fn auth_enabled_protects_and_exposes_expected_endpoints() {
    let app_state = Arc::new(ArkState::default());
    app_state.set_state(ApplicationState::StartingNetwork);

    // Create auth state and keep temp dir alive for the test duration
    let provider = IdentityProviderConfig {
        name: "fake".into(),
        client_id: "client".into(),
        client_secret: None,
        authority: "https://example.invalid".into(),
        discovery: false,
        ..Default::default()
    };

    let auth_cfg = AuthConfig {
        enabled: true,
        provider: Some("fake".into()),
        providers: vec![provider],
        session: Some(SessionConfig::default()),
    };
    let (auth_state, _temp_dir) = create_test_auth_state(Some(auth_cfg)).await;
    let auth_state = Arc::new(auth_state);

    let router = build_test_router(auth_state.clone(), app_state.clone()).await;

    // Unauthenticated requests to protected endpoints should be rejected
    let protected_endpoints = [
        (Method::POST, "/sse"),
        (Method::POST, "/message"),
        (Method::GET, "/api/plugins"),
        (Method::POST, "/mcp"),
    ];
    for (method, uri) in protected_endpoints.iter() {
        let req = Request::builder()
            .method(method.clone())
            .uri(*uri)
            .body(Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "{} must be protected",
            uri
        );
    }

    // Unauthenticated requests to unprotected endpoints should pass
    let public_uris = ["/admin", "/livez", "/readyz"];
    for uri in public_uris.iter() {
        let req = Request::get(*uri).body(Body::empty()).unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "{} must be public", uri);
    }

    // Authenticate via session cookie and verify protected endpoint passes
    let principal = auth::Principal {
        subject: "tester".into(),
        email: None,
        name: Some("Tester".into()),
        provider: "fake".into(),
        picture: None,
        provider_kind: ark::server::auth::ProviderKind::Oidc,
        tenant_id: None,
        oid: None,
        roles: vec![Role::User],
        is_admin: false,
        groups: vec![],
    };
    let session_id = auth_state
        .put_session(principal, std::time::Duration::from_secs(60))
        .await;
    let cookie = format!("ark_session={}", session_id);

    let req = Request::get("/api/plugins")
        .header(header::COOKIE, cookie)
        .body(Body::empty())
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "/api/plugins should be accessible with session"
    );
}

#[tokio::test]
async fn metrics_endpoint_requires_admin() {
    let app_state = Arc::new(ArkState::default());
    app_state.set_state(ApplicationState::StartingNetwork);

    // Create auth state with a fake provider
    let provider = IdentityProviderConfig {
        name: "fake".into(),
        client_id: "client".into(),
        client_secret: None,
        authority: "https://example.invalid".into(),
        discovery: false,
        ..Default::default()
    };

    let auth_cfg = AuthConfig {
        enabled: true,
        provider: Some("fake".into()),
        providers: vec![provider],
        session: Some(SessionConfig::default()),
    };
    let (auth_state, _temp_dir) = create_test_auth_state(Some(auth_cfg)).await;
    let auth_state = Arc::new(auth_state);

    let router = build_test_router(auth_state.clone(), app_state.clone()).await;

    // Test that non-admin user cannot access /metrics
    let non_admin_principal = auth::Principal {
        subject: "non-admin".into(),
        email: None,
        name: Some("Non Admin".into()),
        provider: "fake".into(),
        picture: None,
        provider_kind: ark::server::auth::ProviderKind::Oidc,
        tenant_id: None,
        oid: None,
        roles: vec![Role::User],
        is_admin: false,
        groups: vec![],
    };
    let session_id = auth_state
        .put_session(non_admin_principal, std::time::Duration::from_secs(60))
        .await;
    let cookie = format!("ark_session={}", session_id);

    let req = Request::get("/metrics")
        .header(header::COOKIE, cookie)
        .body(Body::empty())
        .unwrap();
    let resp = router.clone().oneshot(req).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "/metrics should require admin privileges"
    );

    // Test that admin user can access /metrics
    let admin_principal = auth::Principal {
        subject: "admin".into(),
        email: None,
        name: Some("Admin".into()),
        provider: "fake".into(),
        picture: None,
        provider_kind: ark::server::auth::ProviderKind::Oidc,
        tenant_id: None,
        oid: None,
        roles: vec![Role::Admin],
        is_admin: true,
        groups: vec![],
    };
    let admin_session_id = auth_state
        .put_session(admin_principal, std::time::Duration::from_secs(60))
        .await;
    let admin_cookie = format!("ark_session={}", admin_session_id);

    let admin_req = Request::get("/metrics")
        .header(header::COOKIE, admin_cookie)
        .body(Body::empty())
        .unwrap();
    let admin_resp = router.oneshot(admin_req).await.unwrap();
    assert_eq!(
        admin_resp.status(),
        StatusCode::OK,
        "/metrics should be accessible to admins"
    );
}

#[tokio::test]
async fn auth_disabled_all_endpoints_are_public() {
    let app_state = Arc::new(ArkState::default());
    app_state.set_state(ApplicationState::StartingNetwork);

    let auth_state = disabled_auth_state().await;
    let router = build_test_router(auth_state, app_state.clone()).await;

    // All endpoints should be accessible without authentication
    let uris = [
        (Method::POST, "/sse"),
        (Method::POST, "/message"),
        (Method::GET, "/api/plugins"),
        (Method::POST, "/mcp"),
        (Method::GET, "/admin"),
        (Method::GET, "/metrics"),
        (Method::GET, "/livez"),
        (Method::GET, "/readyz"),
    ];
    for (method, uri) in uris.iter() {
        let req = Request::builder()
            .method(method.clone())
            .uri(*uri)
            .body(Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "{} should be public when auth disabled",
            uri
        );
    }
}
