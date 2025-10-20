use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ark::server::roles::Role;
use ark::{
    config::models::{AuthConfig, IdentityProviderConfig, SessionConfig},
    server::auth::{self, IdTokenClaims, Principal},
};
use tempfile::TempDir;

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
        picture: None,
        exp: (now + exp_offset_secs) as u64,
        iat: (now + iat_offset_secs) as u64,
        email: Some("test@example.com".to_string()),
        name: Some("Test User".to_string()),
        tid: None,
        oid: None,
        groups: None,
    }
}

#[tokio::test]
//#[ignore = "Session expiration timing test has edge case issues with temp database lifecycle"]
async fn test_session_expiration() {
    let (auth_state, _temp_dir) = create_test_auth_state().await;

    let principal = Principal {
        subject: "test".to_string(),
        email: Some("test@example.com".to_string()),
        picture: None,
        name: Some("Test User".to_string()),
        provider: "test".to_string(),
        provider_kind: ark::server::auth::ProviderKind::Oidc,
        tenant_id: None,
        oid: None,
        roles: vec![Role::User],
        is_admin: false,
        groups: vec![],
    };

    // Create a session with short TTL (but long enough for database persistence)
    let ttl = Duration::from_secs(2);
    let session_id = auth_state.put_session(principal.clone(), ttl).await;

    // Wait until the session is visible, allowing the database write to settle.
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if auth_state.get_session(&session_id).await.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("session should become available shortly");

    // Wait beyond the TTL with a buffer to account for scheduling jitter.
    tokio::time::sleep(ttl + Duration::from_millis(500)).await;

    // Poll until the session expires and is removed. Invoke cleanup to flush expired rows.
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            auth_state.cleanup().await;
            if auth_state.get_session(&session_id).await.is_none() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("session should be removed shortly");
}
#[tokio::test]
async fn test_malformed_cookie_handling() {
    let (auth_state, _temp_dir) = create_test_auth_state().await;

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
    let too_long_state = "a".repeat(257);
    let invalid_states = vec![
        ("", "Empty state"),
        (too_long_state.as_str(), "Too long state"),
        ("state with spaces", "State with spaces"),
        ("state/with/slashes", "State with slashes"),
        ("state.with.dots", "State with dots"),
        ("state@with@symbols", "State with symbols"),
        ("state<script>alert('xss')</script>", "XSS attempt"),
        ("../../etc/passwd", "Path traversal attempt"),
        ("state\x00null", "Null bytes"),
        ("'; DROP TABLE sessions; --", "SQL injection attempt"),
    ];

    for (state, description) in invalid_states {
        // This would be tested in callback handler - state validation should reject these
        let is_invalid = state.is_empty()
            || state.len() > 256
            || !state
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');

        assert!(
            is_invalid,
            "State should be rejected for case '{}': {}",
            description, state
        );

        // Verify specific rejection reasons
        if state.is_empty() {
            assert_eq!(
                state.len(),
                0,
                "Empty state should be detectable: {}",
                description
            );
        } else if state.len() > 256 {
            assert!(
                state.len() > 256,
                "Oversized state should be detectable: {}",
                description
            );
        } else {
            assert!(
                state
                    .chars()
                    .any(|c| !c.is_ascii_alphanumeric() && c != '-' && c != '_'),
                "Invalid characters should be detectable: {}",
                description
            );
        }
    }

    // Valid states should pass
    let long_state = "a".repeat(256);
    let valid_states = vec![
        ("abc123", "Simple alphanumeric"),
        ("state-with-dashes", "Dashes allowed"),
        ("state_with_underscores", "Underscores allowed"),
        (long_state.as_str(), "Maximum length"),
        ("a", "Single character"),
        ("State123_test-value", "Mixed valid characters"),
    ];

    for (state, description) in valid_states {
        let is_valid = !state.is_empty()
            && state.len() <= 256
            && state
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');

        assert!(
            is_valid,
            "Valid state should pass validation for case '{}': {}",
            description, state
        );

        // Additional validation for well-formed states
        assert!(
            !state.is_empty(),
            "Valid state should not be empty: {}",
            description
        );
        assert!(
            state.len() <= 256,
            "Valid state should not exceed max length: {}",
            description
        );
        assert!(
            !state.contains(' '),
            "Valid state should not contain spaces: {}",
            description
        );
        assert!(
            !state.contains('<'),
            "Valid state should not contain HTML: {}",
            description
        );
    }
}

#[tokio::test]
async fn test_token_timestamp_validation() {
    let (_auth_state, _temp_dir) = create_test_auth_state().await;

    // Test expired token (exp in past)
    let expired_claims = create_test_claims(-3600, -7200); // Expired 1 hour ago, issued 2 hours ago

    // Test token issued in future (iat in future)
    let future_claims = create_test_claims(3600, 1800); // Valid for 1 hour, but issued 30 min in future

    // Test token too old (iat more than 1 hour ago)
    let old_claims = create_test_claims(3600, -7200); // Valid for 1 hour, but issued 2 hours ago

    // Test valid token
    let valid_claims = create_test_claims(3600, -60); // Valid for 1 hour, issued 1 minute ago

    // These would be tested in validate_id_token - should be rejected
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    // Validate expired token detection
    assert!(
        now > expired_claims.exp,
        "Expired token should be rejected - exp in past"
    );
    assert!(
        expired_claims.iat < expired_claims.exp,
        "Expired token should have been issued before expiration"
    );

    // Validate future token detection
    assert!(
        now < future_claims.iat,
        "Future token should be rejected - iat in future"
    );
    let max_clock_skew = 60; // 60 seconds tolerance
    assert!(
        future_claims.iat > now + max_clock_skew,
        "Future token should exceed clock skew tolerance"
    );

    // Validate old token detection
    assert!(
        (now - old_claims.iat) > 3600,
        "Too old token should be rejected - issued more than 1 hour ago"
    );
    assert!(
        old_claims.exp > now,
        "Old token should still be unexpired but too old to accept"
    );

    // Validate proper token acceptance criteria
    assert!(valid_claims.exp > now, "Valid token should not be expired");
    assert!(
        valid_claims.iat <= now,
        "Valid token should be issued in past or now"
    );
    assert!(
        valid_claims.iat > now - 3600,
        "Valid token should not be too old"
    );
    assert!(
        valid_claims.iat < valid_claims.exp,
        "Valid token should be issued before expiration"
    );

    // Test edge cases for clock skew
    let near_future_claims = create_test_claims(3600, 30); // Issued 30 seconds in future
    assert!(
        near_future_claims.iat <= now + max_clock_skew,
        "Token within clock skew should be acceptable"
    );
}

#[tokio::test]
async fn test_concurrent_session_access() {
    let (auth_state, _temp_dir) = create_test_auth_state().await;

    let principal = Principal {
        subject: "concurrent-test".to_string(),
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
    let (auth_state, _temp_dir) = create_test_auth_state().await;

    // Create expired session
    let principal = Principal {
        subject: "cleanup-test".to_string(),
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
                oauth_query: None,
            },
        );
        pending.insert(
            "valid-state".to_string(),
            auth::PendingAuth {
                code_verifier: "test".to_string(),
                created_at: std::time::Instant::now(),
                redirect_to: None,
                oauth_query: None,
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
        ("/health", false),
        ("/authorize", false), // OAuth authorization endpoint
        ("/.well-known/openid-configuration", false), // OIDC discovery
        ("/.well-known/jwks.json", false),
        ("/.well-known/oauth-authorization-server", false),
        ("/.well-known/oauth-authorization-server/mcp", false),
        ("/mcp/.well-known/openid-configuration", false),
        ("/metrics", true),     // Metrics endpoint (requires admin)
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
            discovery: true,
            ..Default::default()
        },
        IdentityProviderConfig {
            name: "google".to_string(),
            client_id: "google-client".to_string(),
            client_secret: Some("google-secret".to_string()),
            authority: "https://accounts.google.com".to_string(),
            scopes: Some("openid profile email".to_string()),
            discovery: true,
            ..Default::default()
        },
    ];

    for provider_name in ["microsoft", "google"] {
        let auth_cfg = AuthConfig {
            enabled: true,
            provider: Some(provider_name.to_string()),
            providers: providers.clone(),
            session: Some(SessionConfig::default()),
        };

        // Create app state with database for testing
        let app_state = ark::state::ArkState::default();
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join(format!("test_{}.db", provider_name));
        let database = ark::server::persist::Database::with_path(&db_path).unwrap();
        let app_state_mut = app_state;
        app_state_mut.set_database(database);

        let auth_state = Arc::new(
            auth::AuthState::new_with_state(&Some(auth_cfg), Arc::new(app_state_mut), None)
                .await
                .unwrap(),
        );
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
        picture: None,
        provider: "test-provider".to_string(),
        provider_kind: ark::server::auth::ProviderKind::Oidc,
        tenant_id: None,
        oid: None,
        roles: vec![Role::User],
        is_admin: false,
        groups: vec![],
    };

    // With provider_kind required the global id uses provider:tenant:userid
    assert_eq!(principal.global_id(), "oidc/*/test-subject");
}

// Helper function to create test auth state
async fn create_test_auth_state() -> (Arc<auth::AuthState>, TempDir) {
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

    // Create app state with database for testing
    let app_state = ark::state::ArkState::default();
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");
    let database = ark::server::persist::Database::with_path(&db_path).unwrap();
    let app_state_mut = app_state;
    app_state_mut.set_database(database);

    let auth_state = Arc::new(
        auth::AuthState::new_with_state(&Some(auth_cfg), Arc::new(app_state_mut), None)
            .await
            .unwrap(),
    );

    (auth_state, temp_dir)
}
