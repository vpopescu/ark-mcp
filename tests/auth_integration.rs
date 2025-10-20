use std::sync::Arc;
use std::time::Duration;

use ark::server::roles::Role;
use ark::{
    config::models::{AuthConfig, IdentityProviderConfig, SessionConfig},
    server::{
        auth::{self, AuthState, Principal},
        handlers,
    },
    state::ArkState,
};
use axum::{
    Router,
    body::Body,
    extract::{Extension, Request},
    http::{Method, StatusCode},
    middleware::{self, Next},
    routing::get,
};
use tower::ServiceExt;

/// Test OAuth login initiation
#[tokio::test]
async fn test_oauth_login_flow() {
    let app = create_test_auth_app().await;

    let request = Request::builder()
        .method(Method::GET)
        .uri("/auth/login?mode=redirect") // Add mode=redirect to get HTTP redirect
        .header("host", "localhost:3000")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    // Should redirect to identity provider
    assert_eq!(response.status(), StatusCode::FOUND);

    let location = response.headers().get("location").unwrap();
    let location_str = location.to_str().unwrap();

    // Verify redirect contains required OAuth parameters
    assert!(location_str.contains("response_type=code"));
    assert!(location_str.contains("code_challenge="));
    assert!(location_str.contains("code_challenge_method=S256"));
    assert!(location_str.contains("state="));
    assert!(location_str.contains("scope="));
    assert!(location_str.contains("client_id="));
}

/// Test OAuth callback with invalid state
#[tokio::test]
async fn test_oauth_callback_invalid_state() {
    let app = create_test_auth_app().await;

    let request = Request::builder()
        .method(Method::GET)
        .uri("/auth/callback?code=test-code&state=invalid-state")
        .header("host", "localhost:3000")
        .body(Body::empty())
        .unwrap();

    let response = app.clone().oneshot(request).await.unwrap();

    // Should return HTML error page (200 status)
    assert_eq!(response.status(), StatusCode::OK);

    // Response should be HTML content
    let content_type = response.headers().get("content-type");
    if let Some(ct) = content_type {
        assert!(ct.to_str().unwrap().contains("text/html"));
    }
}

/// Test OAuth callback with missing parameters
#[tokio::test]
async fn test_oauth_callback_missing_params() {
    let app = create_test_auth_app().await;

    let test_cases = vec![
        ("/auth/callback", "No parameters"),
        ("/auth/callback?code=test-code", "Missing state"),
        ("/auth/callback?state=test-state", "Missing code"),
        ("/auth/callback?error=access_denied", "OAuth error"),
    ];

    for (uri, description) in test_cases {
        let request = Request::builder()
            .method(Method::GET)
            .uri(uri)
            .body(Body::empty())
            .unwrap();

        let response = app.clone().oneshot(request).await.unwrap();

        // Should return HTML error page with proper status
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "Failed for case: {}",
            description
        );

        // Verify response is HTML content (error page)
        let content_type = response.headers().get("content-type");
        assert!(
            content_type.is_some(),
            "Missing content-type for case: {}",
            description
        );

        let ct_str = content_type.unwrap().to_str().unwrap();
        assert!(
            ct_str.contains("text/html"),
            "Expected HTML error page for case: {}, got content-type: {}",
            description,
            ct_str
        );
    }
}

/// Test authentication status endpoint
#[tokio::test]
async fn test_auth_status_endpoint() {
    let app = create_test_auth_app().await;

    // Test without session
    let request = Request::builder()
        .method(Method::GET)
        .uri("/auth/status")
        .header("host", "localhost:3000")
        .body(Body::empty())
        .unwrap();

    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    // Test with invalid session cookie
    let request = Request::builder()
        .method(Method::GET)
        .uri("/auth/status")
        .header("host", "localhost:3000")
        .header("Cookie", "ark_session=invalid-session-id")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

/// Test logout functionality
#[tokio::test]
async fn test_logout_endpoint() {
    let app = create_test_auth_app().await;

    let request = Request::builder()
        .method(Method::GET)
        .uri("/auth/logout")
        .header("host", "localhost:3000")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    // Should redirect to IDP logout endpoint
    assert_eq!(response.status(), StatusCode::FOUND);

    // Should have Location header for redirect
    let location = response.headers().get("location");
    assert!(location.is_some());

    // Should set cookie to expire
    let set_cookie = response.headers().get("set-cookie");
    if let Some(cookie) = set_cookie {
        let cookie_str = cookie.to_str().unwrap();
        assert!(cookie_str.contains("Max-Age=0") || cookie_str.contains("expires="));
    }
}

/// Test protected endpoint access without auth
#[tokio::test]
async fn test_protected_endpoint_without_auth() {
    let app = create_test_protected_app().await;

    let protected_endpoints = vec!["/api/plugins", "/mcp", "/sse", "/message"];

    for endpoint in protected_endpoints {
        let request = Request::builder()
            .method(Method::GET)
            .uri(endpoint)
            .body(Body::empty())
            .unwrap();

        let response = app.clone().oneshot(request).await.unwrap();

        // Should return unauthorized (not redirect)
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
}

/// Test public endpoint access without auth
#[tokio::test]
async fn test_public_endpoint_access() {
    let app = create_test_protected_app().await;

    let public_endpoints = vec![
        ("/", "Should serve home page"),
        ("/admin", "Should serve admin page"),
        ("/auth/login", "Should serve login page"),
        ("/auth/callback", "Should serve callback handler"),
        ("/auth/status", "Should return auth status"),
        ("/livez", "Should return liveness check"),
        ("/readyz", "Should return readiness check"),
        ("/health", "Should return health check"),
        ("/metrics", "Should return metrics"),
    ];

    for (endpoint, description) in public_endpoints {
        let request = Request::builder()
            .method(Method::GET)
            .uri(endpoint)
            .body(Body::empty())
            .unwrap();

        let response = app.clone().oneshot(request).await.unwrap();

        // Should not redirect to login - public endpoints should be accessible
        assert_ne!(
            response.status(),
            StatusCode::FOUND,
            "Public endpoint {} should not redirect to login: {}",
            endpoint,
            description
        );

        // Should return success or client error (not server error or redirect)
        assert!(
            response.status().is_success() || response.status().is_client_error(),
            "Public endpoint {} should return success or client error, got {}: {}",
            endpoint,
            response.status(),
            description
        );

        // Auth status endpoint specifically should return 401 for unauthenticated requests
        if endpoint == "/auth/status" {
            assert_eq!(
                response.status(),
                StatusCode::UNAUTHORIZED,
                "Auth status endpoint should return 401 for unauthenticated requests"
            );
        }
    }
}

/// Test session-based authentication middleware
#[tokio::test]
async fn test_session_authentication_middleware() {
    let (auth_state, _temp_dir) = create_test_auth_state().await;
    let auth_state = &auth_state;

    // Create a valid session
    let principal = Principal {
        subject: "test-user".to_string(),
        email: Some("test@example.com".to_string()),
        name: Some("Test User".to_string()),
        provider: "test".to_string(),
        picture: None,
        provider_kind: ark::server::auth::ProviderKind::Oidc,
        tenant_id: None,
        oid: None,
        roles: vec![Role::User],
        is_admin: false,
        groups: vec![],
    };

    let session_id = auth_state
        .put_session(principal, Duration::from_secs(3600))
        .await;

    let app = create_test_protected_app_with_state(auth_state.clone()).await;

    // Test protected endpoint with valid session
    let request = Request::builder()
        .method(Method::GET)
        .uri("/api/plugins")
        .header("Cookie", format!("ark_session={}", session_id))
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    // Should allow access (not redirect)
    assert_ne!(response.status(), StatusCode::FOUND);
    // Note: Might be 404 since we don't have actual API handlers, but not 302 redirect
}

/// Test error handling in authentication endpoints
#[tokio::test]
async fn test_auth_error_handling() {
    let app = create_test_auth_app().await;

    // Test malformed requests
    let malformed_requests = vec![
        (
            "/auth/callback?code=&state=".to_string(),
            "Empty parameters",
        ),
        (
            format!("/auth/callback?code=test&state={}", "x".repeat(300)),
            "State too long",
        ),
        (
            format!("/auth/login?{}", "x".repeat(1000)),
            "Query too long",
        ),
    ];

    for (uri, description) in malformed_requests {
        let request = Request::builder()
            .method(Method::GET)
            .uri(&uri)
            .header("host", "localhost:3000")
            .body(Body::empty())
            .unwrap();

        let response = app.clone().oneshot(request).await.unwrap();

        // Should handle gracefully without panicking
        assert!(
            response.status().is_success() || response.status().is_client_error(),
            "Request should be handled gracefully for case: {}, got status: {}",
            description,
            response.status()
        );

        // Should not return server errors (500-599) for malformed client input
        assert!(
            !response.status().is_server_error(),
            "Malformed client input should not cause server error for case: {}, got status: {}",
            description,
            response.status()
        );

        // For callback with malformed parameters, should return HTML error page
        if uri.starts_with("/auth/callback") {
            assert_eq!(
                response.status(),
                StatusCode::OK,
                "Callback with malformed params should return HTML error page for case: {}",
                description
            );

            let content_type = response.headers().get("content-type");
            if let Some(ct) = content_type {
                assert!(
                    ct.to_str().unwrap().contains("text/html"),
                    "Callback error should return HTML for case: {}",
                    description
                );
            }
        }
    }
}

/// Test CSRF protection through state parameter
#[tokio::test]
async fn test_csrf_protection() {
    let app = create_test_auth_app().await;

    // Multiple login requests should generate different state parameters
    let mut states = std::collections::HashSet::new();

    for _ in 0..5 {
        let request = Request::builder()
            .method(Method::GET)
            .uri("/auth/login?mode=redirect")
            .header("host", "localhost:3000")
            .body(Body::empty())
            .unwrap();

        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::FOUND);

        let location = response.headers().get("location").unwrap();
        let location_str = location.to_str().unwrap();

        // Extract state parameter
        if let Some(state_start) = location_str.find("state=") {
            let state_part = &location_str[state_start + 6..];
            let state_end = state_part.find('&').unwrap_or(state_part.len());
            let state = &state_part[..state_end];
            states.insert(state.to_string());
        }
    }

    // All states should be unique
    assert_eq!(
        states.len(),
        5,
        "State parameters should be unique for CSRF protection"
    );
}

// Helper functions for test setup

async fn create_test_auth_app() -> Router {
    let (auth_state, _temp_dir) = create_test_auth_state().await;

    // Mount the auth router at /auth path
    Router::new().nest("/auth", handlers::session::router(auth_state))
}

async fn create_test_protected_app() -> Router {
    let (auth_state, _temp_dir) = create_test_auth_state().await;
    create_test_protected_app_with_state(auth_state).await
}

async fn create_test_protected_app_with_state(auth_state: Arc<AuthState>) -> Router {
    let auth_state_clone = auth_state.clone();

    Router::new()
        .route("/", get(|| async { "Home" }))
        .route("/admin", get(|| async { "Admin" }))
        .route("/livez", get(|| async { "OK" }))
        .route("/readyz", get(|| async { "OK" }))
        .route("/health", get(|| async { "OK" }))
        .route("/metrics", get(|| async { "metrics" }))
        .route("/api/plugins", get(|| async { "API" }))
        .route("/mcp", get(|| async { "MCP" }))
        .route("/sse", get(|| async { "SSE" }))
        .route("/message", get(|| async { "Message" }))
        .layer(middleware::from_fn(
            move |req: Request<Body>, next: Next| {
                let auth_state = auth_state_clone.clone();
                async move { auth::check_auth(req, next, Extension(auth_state)).await }
            },
        ))
        .merge(Router::new().nest("/auth", handlers::session::router(auth_state)))
}

async fn create_test_auth_state() -> (Arc<AuthState>, tempfile::TempDir) {
    let provider = IdentityProviderConfig {
        name: "test".to_string(),
        client_id: "test-client".to_string(),
        client_secret: Some("test-secret".to_string()),
        authority: "https://example.com".to_string(),
        scopes: Some("openid profile email".to_string()),
        discovery: false,
        jwks_uri: Some("https://example.com/.well-known/jwks.json".to_string()),
        authorization_endpoint: Some("https://example.com/auth".to_string()),
        token_endpoint: Some("https://example.com/token".to_string()),
        ..Default::default()
    };

    let auth_cfg = AuthConfig {
        enabled: true,
        provider: Some("test".to_string()),
        providers: vec![provider],
        session: Some(SessionConfig::default()),
    };

    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("test.db");
    let app_state = Arc::new(ArkState::default());
    let database = ark::server::persist::Database::with_path(&db_path).unwrap();
    {
        let mut db_guard = app_state.database.write().unwrap();
        *db_guard = Some(database);
    }

    let auth_state = Arc::new(
        AuthState::new_with_state(&Some(auth_cfg), app_state, None)
            .await
            .unwrap(),
    );

    (auth_state, temp_dir)
}
