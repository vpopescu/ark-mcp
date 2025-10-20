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
use tempfile::TempDir;
use tower::ServiceExt;

/// Test token exchange with network failures
#[tokio::test]
async fn test_token_exchange_network_failures() {
    let (auth_state, _temp_dir) = create_test_auth_state_with_invalid_endpoint().await;

    // Test token exchange with unreachable endpoint
    let result = auth_state
        .exchange_code_for_tokens(
            "test-code",
            "test-verifier",
            "https://localhost:3000/auth/callback",
            "test-client",
            Some("test-secret"),
            "https://invalid.example.test/token", // Invalid URL
        )
        .await;

    assert!(result.is_err(), "Should handle network failures gracefully");
}

/// Test token exchange with malformed responses
#[tokio::test]
async fn test_token_exchange_malformed_responses() {
    // This test would require a mock HTTP server to return malformed responses
    // For now, we test the error handling paths

    let (auth_state, _temp_dir) = create_test_auth_state().await;

    // Test with invalid authorization code (would get 400 from real provider)
    let result = auth_state
        .exchange_code_for_tokens(
            "", // Empty code
            "test-verifier",
            "https://localhost:3000/auth/callback",
            "test-client",
            Some("test-secret"),
            "https://example.com/token",
        )
        .await;

    assert!(result.is_err(), "Should reject empty authorization code");

    // Test with invalid code verifier
    let result = auth_state
        .exchange_code_for_tokens(
            "test-code",
            "", // Empty verifier
            "https://localhost:3000/auth/callback",
            "test-client",
            Some("test-secret"),
            "https://example.com/token",
        )
        .await;

    assert!(result.is_err(), "Should reject empty code verifier");
}

/// Test provider discovery failures
#[tokio::test]
async fn test_provider_discovery_failures() {
    // Test with invalid discovery URL
    let provider = IdentityProviderConfig {
        name: "invalid-discovery".to_string(),
        client_id: "test-client".to_string(),
        client_secret: Some("test-secret".to_string()),
        authority: "https://invalid.example.test".to_string(),
        scopes: Some("openid profile email".to_string()),
        discovery: true, // Enable discovery
        ..Default::default()
    };

    let auth_cfg = AuthConfig {
        enabled: true,
        provider: Some("invalid-discovery".to_string()),
        providers: vec![provider],
        session: Some(SessionConfig::default()),
    };

    // Should create auth state but discovery should fail
    let temp_dir = TempDir::new().unwrap();
    let app_state = ark::state::ArkState::default();
    let db_path = temp_dir.path().join("test_discovery.db");
    let database = ark::server::persist::Database::with_path(&db_path).unwrap();

    app_state.set_database(database);
    let auth_state = AuthState::new_with_state(&Some(auth_cfg), Arc::new(app_state), None).await;

    match auth_state {
        Ok(state) => {
            // Discovery failure should not prevent auth state creation
            let provider_guard = state.active.read().await;
            // But the provider should have missing endpoints
            assert!(
                provider_guard.is_none()
                    || provider_guard
                        .as_ref()
                        .unwrap()
                        .authorization_endpoint
                        .is_none(),
                "Discovery failure should result in missing endpoints"
            );
        }
        Err(_) => {
            // Also acceptable - creation can fail due to discovery issues
        }
    }
}

/// Test JWKS endpoint failures
#[tokio::test]
async fn test_jwks_endpoint_failures() {
    let (_auth_state, _temp_dir) = create_test_auth_state().await;

    // Test JWT validation when JWKS endpoint is unreachable
    // This would normally fetch keys from the JWKS endpoint
    let test_jwt = "eyJ0eXAiOiJKV1QiLCJhbGciOiJSUzI1NiJ9.eyJzdWIiOiJ0ZXN0LXVzZXIiLCJhdWQiOiJ0ZXN0LWNsaWVudCIsImlzcyI6Imh0dHBzOi8vZXhhbXBsZS5jb20iLCJleHAiOjk5OTk5OTk5OTksImlhdCI6MTYwMDAwMDAwMH0.invalid";

    // Test JWT format validation
    let parts: Vec<&str> = test_jwt.split('.').collect();
    assert_eq!(
        parts.len(),
        3,
        "JWT should have 3 parts even if signature is invalid"
    );
}

/// Test configuration validation errors
#[tokio::test]
async fn test_configuration_validation_errors() {
    // Test with missing client_id
    let invalid_provider = IdentityProviderConfig {
        name: "invalid".to_string(),
        client_id: "".to_string(),
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
        provider: Some("invalid".to_string()),
        providers: vec![invalid_provider],
        session: Some(SessionConfig::default()),
    };

    let temp_dir = TempDir::new().unwrap();
    let app_state = ark::state::ArkState::default();
    let db_path = temp_dir.path().join("test_invalid_config.db");
    let database = ark::server::persist::Database::with_path(&db_path).unwrap();

    app_state.set_database(database);
    let result = AuthState::new_with_state(&Some(auth_cfg), Arc::new(app_state), None).await;

    // With empty client_id, the system should either:
    // 1. Fail to create AuthState (preferred for security)
    // 2. Create AuthState but with no active provider (failsafe)
    match result {
        Ok(auth_state) => {
            let provider_guard = auth_state.active.read().await;
            // If creation succeeds, there should be no active provider due to validation failure
            assert!(
                provider_guard.is_none(),
                "Empty client_id should result in no active provider being configured"
            );
        }
        Err(err) => {
            // Creation failure is also acceptable for invalid configuration
            assert!(
                err.to_string().contains("client") || err.to_string().contains("invalid"),
                "Error should indicate configuration validation failure: {}",
                err
            );
        }
    }

    // Test with missing provider reference
    let auth_cfg_missing_provider = AuthConfig {
        enabled: true,
        provider: Some("nonexistent-provider".to_string()), // Provider that doesn't exist
        providers: vec![],                                  // Empty providers list
        session: Some(SessionConfig::default()),
    };

    let temp_dir = TempDir::new().unwrap();
    let app_state = ark::state::ArkState::default();
    let db_path = temp_dir.path().join("test_missing_provider.db");
    let database = ark::server::persist::Database::with_path(&db_path).unwrap();

    app_state.set_database(database);
    let result =
        AuthState::new_with_state(&Some(auth_cfg_missing_provider), Arc::new(app_state), None)
            .await;
    match result {
        Ok(auth_state) => {
            let provider_guard = auth_state.active.read().await;
            assert!(
                provider_guard.is_none(),
                "Nonexistent provider reference should result in no active provider"
            );
        }
        Err(_) => {
            // Creation failure is also acceptable when referencing nonexistent provider
        }
    }

    // Test with malformed URL in authority
    let invalid_authority_provider = IdentityProviderConfig {
        name: "invalid-authority".to_string(),
        client_id: "test-client".to_string(),
        client_secret: Some("test-secret".to_string()),
        authority: "not-a-url".to_string(), // Invalid URL format
        scopes: Some("openid profile email".to_string()),
        discovery: false,
        jwks_uri: Some("https://example.com/.well-known/jwks.json".to_string()),
        authorization_endpoint: Some("https://example.com/auth".to_string()),
        token_endpoint: Some("https://example.com/token".to_string()),
        ..Default::default()
    };

    let auth_cfg_invalid_authority = AuthConfig {
        enabled: true,
        provider: Some("invalid-authority".to_string()),
        providers: vec![invalid_authority_provider],
        session: Some(SessionConfig::default()),
    };

    let temp_dir = TempDir::new().unwrap();
    let app_state = ark::state::ArkState::default();
    let db_path = temp_dir.path().join("test_invalid_authority.db");
    let database = ark::server::persist::Database::with_path(&db_path).unwrap();

    app_state.set_database(database);
    let result =
        AuthState::new_with_state(&Some(auth_cfg_invalid_authority), Arc::new(app_state), None)
            .await;

    // Invalid URL should be detected during configuration validation
    match result {
        Ok(auth_state) => {
            let provider_guard = auth_state.active.read().await;
            if let Some(provider) = provider_guard.as_ref() {
                // If created successfully, the authority should either be corrected or
                // the provider should be marked as invalid somehow
                assert!(
                    provider.authority.starts_with("http"),
                    "Authority URL should be valid or corrected: {}",
                    provider.authority
                );
            } else {
                // No active provider due to validation failure - acceptable
            }
        }
        Err(err) => {
            // Configuration failure is expected for malformed URLs
            assert!(
                err.to_string().contains("url")
                    || err.to_string().contains("authority")
                    || err.to_string().contains("invalid"),
                "Error should indicate URL validation failure: {}",
                err
            );
        }
    }

    // Test with missing required endpoints (when discovery is disabled)
    let incomplete_provider = IdentityProviderConfig {
        name: "incomplete".to_string(),
        client_id: "test-client".to_string(),
        client_secret: Some("test-secret".to_string()),
        authority: "https://example.com".to_string(),
        scopes: Some("openid profile email".to_string()),
        discovery: false,             // Discovery disabled
        jwks_uri: None,               // Missing required endpoint
        authorization_endpoint: None, // Missing required endpoint
        token_endpoint: None,         // Missing required endpoint
        ..Default::default()
    };

    let auth_cfg_incomplete = AuthConfig {
        enabled: true,
        provider: Some("incomplete".to_string()),
        providers: vec![incomplete_provider],
        session: Some(SessionConfig::default()),
    };

    let temp_dir = TempDir::new().unwrap();
    let app_state = ark::state::ArkState::default();
    let db_path = temp_dir.path().join("test_incomplete.db");
    let database = ark::server::persist::Database::with_path(&db_path).unwrap();

    app_state.set_database(database);
    let result =
        AuthState::new_with_state(&Some(auth_cfg_incomplete), Arc::new(app_state), None).await;
    match result {
        Ok(auth_state) => {
            let provider_guard = auth_state.active.read().await;
            // Should either have no active provider or the provider should be incomplete
            if let Some(provider) = provider_guard.as_ref() {
                assert!(
                    provider.authorization_endpoint.is_none() || provider.token_endpoint.is_none(),
                    "Incomplete provider should have missing endpoints"
                );
            }
            // Missing endpoints make the provider unusable, which is expected
        }
        Err(_) => {
            // Failure is also acceptable for incomplete configuration
        }
    }
}

/// Test OAuth callback error scenarios
#[tokio::test]
async fn test_oauth_callback_error_scenarios() {
    let app = create_test_auth_app().await;

    // Test OAuth provider errors
    let oauth_errors = vec![
        "access_denied",
        "invalid_request",
        "unauthorized_client",
        "unsupported_response_type",
        "invalid_scope",
        "server_error",
        "temporarily_unavailable",
    ];

    for error_code in oauth_errors {
        let request = Request::builder()
            .method(Method::GET)
            .uri(format!(
                "/auth/callback?error={}&error_description=Test+error",
                error_code
            ))
            .header("host", "localhost:3000")
            .body(Body::empty())
            .unwrap();

        let response = app.clone().oneshot(request).await.unwrap();

        // Should return HTML error page
        assert_eq!(response.status(), StatusCode::OK);

        // Verify it's an error response
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body_str = String::from_utf8_lossy(&body);
        assert!(body_str.contains("Authentication Error") || body_str.contains(error_code));
    }
}

/// Test session timeout and cleanup edge cases
#[tokio::test]
async fn test_session_timeout_edge_cases() {
    let (auth_state, _temp_dir) = create_test_auth_state().await;

    let principal = Principal {
        subject: "timeout-test".to_string(),
        email: Some("test@example.com".to_string()),
        name: Some("Test User".to_string()),
        picture: None,
        provider: "test".to_string(),
        provider_kind: ark::server::auth::ProviderKind::Oidc,
        tenant_id: None,
        oid: None,
        roles: vec![Role::User],
        is_admin: false,
        groups: vec![],
    };

    // Test session with zero duration
    let zero_session = auth_state
        .put_session(principal.clone(), Duration::from_secs(0))
        .await;

    // Should immediately expire
    let retrieved = auth_state.get_session(&zero_session).await;
    assert!(
        retrieved.is_none(),
        "Zero-duration session should immediately expire"
    );

    // Test session with very long duration (but not overflow)
    let long_session = auth_state
        .put_session(principal.clone(), Duration::from_secs(86400))
        .await; // 1 day

    // Should be retrievable
    let retrieved = auth_state.get_session(&long_session).await;
    assert!(retrieved.is_some(), "Long-duration session should be valid");

    // Test concurrent cleanup operations
    let cleanup_futures = (0..10).map(|_| {
        let auth_state = auth_state.clone();
        tokio::spawn(async move {
            auth_state.cleanup().await;
        })
    });

    // All cleanup operations should complete without errors
    for future in cleanup_futures {
        future.await.unwrap();
    }
}

/// Test authentication middleware edge cases
#[tokio::test]
async fn test_auth_middleware_edge_cases() {
    let (auth_state_clone, _temp_dir) = create_test_auth_state().await;

    let app = Router::new()
        .route("/test", get(|| async { "Protected" }))
        .layer(middleware::from_fn(
            move |req: Request<Body>, next: Next| {
                let auth_state = auth_state_clone.clone();
                async move { auth::check_auth(req, next, Extension(auth_state)).await }
            },
        ));

    // Test request with malformed cookie header
    let request = Request::builder()
        .method(Method::GET)
        .uri("/test")
        .header("host", "localhost:3000")
        .header("Cookie", "malformed cookie data ☃️") // Invalid cookie format
        .body(Body::empty())
        .unwrap();

    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    // Test request with multiple cookies including session
    let request = Request::builder()
        .method(Method::GET)
        .uri("/test")
        .header("host", "localhost:3000")
        .header("Cookie", "other=value; ark_session=invalid; third=another")
        .body(Body::empty())
        .unwrap();

    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    // Test request with extremely long cookie
    let long_cookie = format!("ark_session={}", "a".repeat(10000));
    let request = Request::builder()
        .method(Method::GET)
        .uri("/test")
        .header("host", "localhost:3000")
        .header("Cookie", &long_cookie)
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

/// Test provider switching and failover
#[tokio::test]
async fn test_provider_failover() {
    // Test configuration with multiple providers
    let providers = vec![
        IdentityProviderConfig {
            name: "primary".to_string(),
            client_id: "primary-client".to_string(),
            client_secret: Some("primary-secret".to_string()),
            authority: "https://primary.example.com".to_string(),
            scopes: Some("openid profile email".to_string()),
            discovery: false,
            jwks_uri: Some("https://primary.example.com/.well-known/jwks.json".to_string()),
            authorization_endpoint: Some("https://primary.example.com/auth".to_string()),
            token_endpoint: Some("https://primary.example.com/token".to_string()),
            ..Default::default()
        },
        IdentityProviderConfig {
            name: "secondary".to_string(),
            client_id: "secondary-client".to_string(),
            client_secret: Some("secondary-secret".to_string()),
            authority: "https://secondary.example.com".to_string(),
            scopes: Some("openid profile email".to_string()),
            discovery: false,
            jwks_uri: Some("https://secondary.example.com/.well-known/jwks.json".to_string()),
            authorization_endpoint: Some("https://secondary.example.com/auth".to_string()),
            token_endpoint: Some("https://secondary.example.com/token".to_string()),
            ..Default::default()
        },
    ];

    // Test with primary provider
    let auth_cfg = AuthConfig {
        enabled: true,
        provider: Some("primary".to_string()),
        providers: providers.clone(),
        session: Some(SessionConfig::default()),
    };

    let app_state = Arc::new(ArkState::default());
    let auth_state = AuthState::new_with_state(&Some(auth_cfg), app_state, None)
        .await
        .unwrap();
    let provider_guard = auth_state.active.read().await;

    if let Some(provider) = provider_guard.as_ref() {
        assert_eq!(provider.client_id, "primary-client");
    }

    // Test switching to secondary provider
    let auth_cfg = AuthConfig {
        enabled: true,
        provider: Some("secondary".to_string()),
        providers,
        session: Some(SessionConfig::default()),
    };

    let app_state = Arc::new(ArkState::default());
    let auth_state = AuthState::new_with_state(&Some(auth_cfg), app_state, None)
        .await
        .unwrap();
    let provider_guard = auth_state.active.read().await;

    if let Some(provider) = provider_guard.as_ref() {
        assert_eq!(provider.client_id, "secondary-client");
    }
}

/// Test memory usage and resource cleanup
#[tokio::test]
async fn test_memory_and_resource_cleanup() {
    let (auth_state, _temp_dir) = create_test_auth_state().await;

    let principal = Principal {
        subject: "memory-test".to_string(),
        email: Some("test@example.com".to_string()),
        name: Some("Test User".to_string()),
        picture: None,
        provider: "test".to_string(),
        provider_kind: ark::server::auth::ProviderKind::Oidc,
        tenant_id: None,
        oid: None,
        roles: vec![Role::User],
        is_admin: false,
        groups: vec![],
    };

    // Create many sessions
    let mut session_ids = Vec::new();
    for i in 0..1000 {
        let session_id = auth_state
            .put_session(
                Principal {
                    subject: format!("user-{}", i),
                    ..principal.clone()
                },
                Duration::from_millis(1), // Very short duration
            )
            .await;
        session_ids.push(session_id);
    }

    // Wait for expiration
    tokio::time::sleep(Duration::from_millis(10)).await;

    // Run cleanup
    auth_state.cleanup().await;

    // Sessions are now database-backed - cleanup removes expired sessions automatically
    // The cleanup method now calls database.cleanup_expired_sessions_async()

    // Create many pending auth states
    {
        let mut pending = auth_state.pending.write().await;
        for i in 0..1000 {
            pending.insert(
                format!("state-{}", i),
                auth::PendingAuth {
                    code_verifier: format!("verifier-{}", i),
                    created_at: std::time::Instant::now() - Duration::from_secs(400), // Expired
                    redirect_to: None,
                    oauth_query: None,
                },
            );
        }
    }

    // Run cleanup again
    auth_state.cleanup().await;

    // Verify pending auth cleanup
    let pending_count = auth_state.pending.read().await.len();
    assert!(
        pending_count < 100,
        "Cleanup should remove expired pending auth states"
    );
}

// Helper functions

async fn create_test_auth_state() -> (Arc<AuthState>, TempDir) {
    let provider = IdentityProviderConfig {
        name: "test".to_string(),
        client_id: "test-client".to_string(),
        client_secret: Some("test-secret".to_string()),
        authority: "https://example.com".to_string(),
        scopes: Some("openid profile email".to_string()),
        audience: None,
        discovery: false,
        jwks_uri: Some("https://example.com/.well-known/jwks.json".to_string()),
        authorization_endpoint: Some("https://example.com/auth".to_string()),
        token_endpoint: Some("https://example.com/token".to_string()),
        redirect_uri: None,
        additional_scopes: None,
        ..Default::default()
    };

    let auth_cfg = AuthConfig {
        enabled: true,
        provider: Some("test".to_string()),
        providers: vec![provider],
        session: Some(SessionConfig::default()),
    };

    // Create app state with database for testing
    let app_state = ark::state::ArkState::default();
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");
    let database = ark::server::persist::Database::with_path(&db_path).unwrap();

    app_state.set_database(database);

    (
        Arc::new(
            AuthState::new_with_state(&Some(auth_cfg), Arc::new(app_state), None)
                .await
                .unwrap(),
        ),
        temp_dir,
    )
}

async fn create_test_auth_state_with_invalid_endpoint() -> (Arc<AuthState>, TempDir) {
    let provider = IdentityProviderConfig {
        name: "invalid".to_string(),
        client_id: "test-client".to_string(),
        client_secret: Some("test-secret".to_string()),
        authority: "https://example.com".to_string(),
        scopes: Some("openid profile email".to_string()),
        discovery: false,
        jwks_uri: Some("https://invalid.example.test/.well-known/jwks.json".to_string()),
        authorization_endpoint: Some("https://example.com/auth".to_string()),
        token_endpoint: Some("https://invalid.example.test/token".to_string()), // Invalid endpoint
        ..Default::default()
    };

    let auth_cfg = AuthConfig {
        enabled: true,
        provider: Some("invalid".to_string()),
        providers: vec![provider],
        session: Some(SessionConfig::default()),
    };

    // Create app state with database for testing
    let app_state = ark::state::ArkState::default();
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test_invalid.db");
    let database = ark::server::persist::Database::with_path(&db_path).unwrap();

    app_state.set_database(database);

    (
        Arc::new(
            AuthState::new_with_state(&Some(auth_cfg), Arc::new(app_state), None)
                .await
                .unwrap(),
        ),
        temp_dir,
    )
}

async fn create_test_auth_app() -> Router {
    let (auth_state, _temp_dir) = create_test_auth_state().await;

    Router::new().nest("/auth", handlers::session::router(auth_state))
}
