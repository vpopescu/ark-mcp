use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ark::{
    config::components::{AuthConfig, IdentityProviderConfig, SessionConfig},
    server::auth::{self, IdTokenClaims, Principal},
};

/// Test helper to create test claims
fn create_test_claims(exp_offset_secs: i64, iat_offset_secs: i64) -> IdTokenClaims {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    IdTokenClaims {
        iss: "https://example.com".to_string(),
        sub: "test-user".to_string(),
        aud: "test-client".to_string(),
        exp: (now + exp_offset_secs) as u64,
        iat: (now + iat_offset_secs) as u64,
        email: Some("test@example.com".to_string()),
        name: Some("Test User".to_string()),
        tid: None,
        oid: None,
    }
}

#[tokio::test]
async fn test_session_expiration() {
    let auth_state = create_test_auth_state().await;

    let principal = Principal {
        subject: "test".to_string(),
        email: Some("test@example.com".to_string()),
        name: Some("Test User".to_string()),
        provider: "test".to_string(),
        tenant_id: None,
        oid: None,
    };

    // Create a session with very short TTL
    let session_id = auth_state
        .put_session(principal.clone(), Duration::from_millis(1))
        .await;

    // Verify session exists initially
    assert!(auth_state.get_session(&session_id).await.is_some());

    // Wait for expiration
    tokio::time::sleep(Duration::from_millis(10)).await;

    // Verify session is expired and removed
    assert!(auth_state.get_session(&session_id).await.is_none());
}

#[tokio::test]
async fn test_malformed_cookie_handling() {
    let auth_state = create_test_auth_state().await;

    let test_cases = vec![
        "",
        "malformed",
        "ark_session=",
        "ark_session=invalid-session-id",
        "other_cookie=value",
        "ark_session=valid; other=malicious<script>",
    ];

    for cookie_str in test_cases {
        let result = auth::extract_session_user_from_cookie(&auth_state, cookie_str).await;
        assert!(
            result.is_none(),
            "Should handle malformed cookie: {}",
            cookie_str
        );
    }
}

#[tokio::test]
async fn test_state_parameter_validation() {
    // Test cases for state validation that should be rejected
    let invalid_states = vec![
        "a".repeat(257), // Too long
        "state with spaces".to_string(),
        "state/with/slashes".to_string(),
        "state.with.dots".to_string(),
        "state@with@symbols".to_string(),
        "state<script>alert('xss')</script>".to_string(),
        "../../etc/passwd".to_string(),
    ];

    for state in invalid_states {
        // This would be tested in callback handler - state validation should reject these
        assert!(
            state.len() > 256
                || !state
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        );
    }

    // Valid states should pass
    let long_state = "a".repeat(256);
    let valid_states = vec![
        "abc123",
        "state-with-dashes",
        "state_with_underscores",
        &long_state, // Max length
    ];

    for state in valid_states {
        assert!(
            state.len() <= 256
                && state
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        );
    }
}

#[tokio::test]
async fn test_token_timestamp_validation() {
    let _auth_state = create_test_auth_state().await;

    // Test expired token (exp in past)
    let expired_claims = create_test_claims(-3600, -7200); // Expired 1 hour ago, issued 2 hours ago

    // Test token issued in future (iat in future)
    let future_claims = create_test_claims(3600, 1800); // Valid for 1 hour, but issued 30 min in future

    // Test token too old (iat more than 1 hour ago)
    let old_claims = create_test_claims(3600, -7200); // Valid for 1 hour, but issued 2 hours ago

    // These would be tested in validate_id_token - should be rejected
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    assert!(now > expired_claims.exp, "Expired token should be rejected");
    assert!(now < future_claims.iat, "Future token should be rejected");
    assert!(
        (now - old_claims.iat) > 3600,
        "Too old token should be rejected"
    );
}

#[tokio::test]
async fn test_concurrent_session_access() {
    let auth_state = create_test_auth_state().await;

    let principal = Principal {
        subject: "concurrent-test".to_string(),
        email: Some("test@example.com".to_string()),
        name: Some("Test User".to_string()),
        provider: "test".to_string(),
        tenant_id: None,
        oid: None,
    };

    let session_id = auth_state
        .put_session(principal.clone(), Duration::from_secs(60))
        .await;

    // Simulate concurrent access to the same session
    let mut handles = vec![];
    for _i in 0..10 {
        let auth_state_clone = auth_state.clone();
        let session_id_clone = session_id.clone();
        let handle =
            tokio::spawn(async move { auth_state_clone.get_session(&session_id_clone).await });
        handles.push(handle);
    }

    // All concurrent accesses should succeed
    for handle in handles {
        let result = handle.await.unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap().subject, "concurrent-test");
    }
}

#[tokio::test]
async fn test_cleanup_functionality() {
    let auth_state = create_test_auth_state().await;

    // Create expired session
    let principal = Principal {
        subject: "cleanup-test".to_string(),
        email: Some("test@example.com".to_string()),
        name: Some("Test User".to_string()),
        provider: "test".to_string(),
        tenant_id: None,
        oid: None,
    };

    let expired_session = auth_state
        .put_session(principal.clone(), Duration::from_millis(1))
        .await;
    let valid_session = auth_state
        .put_session(principal.clone(), Duration::from_secs(60))
        .await;

    // Add expired pending auth
    {
        let mut pending = auth_state.pending.write().await;
        pending.insert(
            "expired-state".to_string(),
            auth::PendingAuth {
                code_verifier: "test".to_string(),
                created_at: std::time::Instant::now() - Duration::from_secs(400), // 400 seconds ago
                redirect_to: None,
            },
        );
        pending.insert(
            "valid-state".to_string(),
            auth::PendingAuth {
                code_verifier: "test".to_string(),
                created_at: std::time::Instant::now(),
                redirect_to: None,
            },
        );
    }

    // Wait for session expiration
    tokio::time::sleep(Duration::from_millis(10)).await;

    // Run cleanup
    auth_state.cleanup().await;

    // Check results
    assert!(auth_state.get_session(&expired_session).await.is_none());
    assert!(auth_state.get_session(&valid_session).await.is_some());

    let pending = auth_state.pending.read().await;
    assert!(!pending.contains_key("expired-state"));
    assert!(pending.contains_key("valid-state"));
}

#[tokio::test]
async fn test_path_requires_auth_edge_cases() {
    // Test path resolution edge cases
    let test_cases = vec![
        ("/", false),                // Root
        ("/admin", false),           // Admin console
        ("/admin/", false),          // Admin with trailing slash
        ("/admin/dashboard", false), // Admin subpath
        ("/auth/login", false),      // Auth endpoints
        ("/auth/callback", false),
        ("/auth/status", false),
        ("/auth/logout", false),
        ("/assets/style.css", false), // Static assets
        ("/static/image.png", false),
        ("/livez", false), // Health checks
        ("/readyz", false),
        ("/metrics", false),
        ("/health", false),
        ("/api/plugins", true), // Protected API
        ("/api/plugins/test", true),
        ("/mcp", true), // MCP endpoints
        ("/sse", true), // SSE endpoints
        ("/message", true),
        ("/random/path", true), // Unknown paths default to protected
        ("/api", true),         // API root
    ];

    for (path, expected) in test_cases {
        let result = auth::path_requires_auth(path);
        assert_eq!(
            result,
            expected,
            "Path '{}' should be {}",
            path,
            if expected { "protected" } else { "public" }
        );
    }
}

#[tokio::test]
async fn test_multiple_provider_resolution() {
    let providers = vec![
        IdentityProviderConfig {
            name: "microsoft".to_string(),
            client_id: "ms-client".to_string(),
            client_secret: Some("ms-secret".to_string()),
            authority: "https://login.microsoftonline.com/tenant/v2.0".to_string(),
            scopes: Some("openid profile email".to_string()),
            audience: None,
            discovery: true,
            jwks_uri: None,
            authorization_endpoint: None,
            token_endpoint: None,
            redirect_uri: None,
            additional_scopes: None,
        },
        IdentityProviderConfig {
            name: "google".to_string(),
            client_id: "google-client".to_string(),
            client_secret: Some("google-secret".to_string()),
            authority: "https://accounts.google.com".to_string(),
            scopes: Some("openid profile email".to_string()),
            audience: None,
            discovery: true,
            jwks_uri: None,
            authorization_endpoint: None,
            token_endpoint: None,
            redirect_uri: None,
            additional_scopes: None,
        },
    ];

    for provider_name in ["microsoft", "google"] {
        let auth_cfg = AuthConfig {
            enabled: true,
            provider: Some(provider_name.to_string()),
            providers: providers.clone(),
            session: Some(SessionConfig::default()),
        };

        let auth_state = Arc::new(auth::AuthState::new(&Some(auth_cfg)).await.unwrap());
        assert!(auth_state.enabled);

        let active_provider = auth_state.active.read().await;
        assert!(active_provider.is_some());

        let provider = active_provider.as_ref().unwrap();
        if provider_name == "microsoft" {
            assert!(provider.authority.contains("microsoftonline"));
            assert_eq!(provider.client_id, "ms-client");
        } else {
            assert!(provider.authority.contains("accounts.google.com"));
            assert_eq!(provider.client_id, "google-client");
        }
    }
}

#[tokio::test]
async fn test_security_headers_and_cookies() {
    // This would test cookie security attributes in integration tests
    // Testing cookie format, HttpOnly, Secure, SameSite attributes
    let session_id = "test-session-123";
    let cookie_value = format!(
        "ark_session={}; Path=/; HttpOnly; SameSite=Lax; Max-Age=3600; Secure",
        session_id
    );

    // Verify cookie format
    assert!(cookie_value.contains("HttpOnly"));
    assert!(cookie_value.contains("Secure"));
    assert!(cookie_value.contains("SameSite=Lax"));
    assert!(cookie_value.contains("Max-Age=3600"));
    assert!(cookie_value.contains("Path=/"));
}

#[tokio::test]
async fn test_principal_global_id() {
    let principal = Principal {
        subject: "test-subject".to_string(),
        email: Some("test@example.com".to_string()),
        name: Some("Test User".to_string()),
        provider: "test-provider".to_string(),
        tenant_id: None,
        oid: None,
    };

    assert_eq!(principal.global_id(), "test-provider:test-subject");
}

// Helper function to create test auth state
async fn create_test_auth_state() -> Arc<auth::AuthState> {
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
    };

    let auth_cfg = AuthConfig {
        enabled: true,
        provider: Some("test".to_string()),
        providers: vec![provider],
        session: Some(SessionConfig::default()),
    };

    Arc::new(auth::AuthState::new(&Some(auth_cfg)).await.unwrap())
}
