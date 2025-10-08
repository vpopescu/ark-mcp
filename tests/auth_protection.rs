use std::sync::Arc;

use ark::{
    config::components::{AuthConfig, IdentityProviderConfig, SessionConfig},
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

// Build a minimal enabled auth state with a fake provider (no external calls).
async fn enabled_auth_state() -> Arc<auth::AuthState> {
    let provider = IdentityProviderConfig {
        name: "fake".into(),
        client_id: "client".into(),
        client_secret: None,
        authority: "https://example.invalid".into(),
        scopes: None,
        audience: None,
        discovery: false,
        jwks_uri: None,
        authorization_endpoint: None,
        token_endpoint: None,
        redirect_uri: None,
        additional_scopes: None,
    };

    let auth_cfg = AuthConfig {
        enabled: true,
        provider: Some("fake".into()),
        providers: vec![provider],
        session: Some(SessionConfig::default()),
    };
    Arc::new(auth::AuthState::new(&Some(auth_cfg)).await.unwrap())
}

// Build a disabled auth state (no provider).
async fn disabled_auth_state() -> Arc<auth::AuthState> {
    Arc::new(auth::AuthState::new(&None).await.unwrap())
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
                if let Some(cookie) = cookie_str {
                    if let Some(principal) =
                        auth::extract_session_user_from_cookie(&auth, cookie).await
                    {
                        let mut req = req;
                        req.extensions_mut().insert(principal);
                        return next.run(req).await;
                    }
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

    let auth_state = enabled_auth_state().await;
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
    let public_uris = ["/admin", "/metrics", "/livez", "/readyz"];
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
        tenant_id: None,
        oid: None,
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
