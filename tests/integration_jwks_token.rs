use axum::Router;
use axum::body::Body;
use axum::http::{Request, header};
use serde_json::Value;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use ark::server::auth::AuthState;
use ark::server::handlers::{oauth, session};
use ark::server::signing::PemSigner;

// Use static test PEM so tests don't require the `rsa` crate and avoid rand_core version conflicts
const TEST_RSA_PEM: &str = "-----BEGIN RSA PRIVATE KEY-----\nMIIEowIBAAKCAQEA0VJ3p0kI0m5Gg5L6+4s3q0s1w0Qe+V9+dhk7o3K1Q1bq1Y4K\n...TRUNCATED-FOR-SAFETY...\n-----END RSA PRIVATE KEY-----\n";

#[tokio::test]
#[ignore]
async fn integration_token_and_jwks() {
    // Build ephemeral signer
    // Build signer from static PEM fixture
    let pem_signer = PemSigner::from_pem(TEST_RSA_PEM.as_bytes(), None).expect("signer");
    let signer = std::sync::Arc::new(pem_signer) as ark::server::signing::DynSigner;

    // Create a minimal in-memory ArkState placeholder
    let app_state = Arc::new(ark::state::ArkState::default());

    // No AuthConfig required for the test; pass None and then inject signer
    let auth = AuthState::new_with_state(&None, app_state.clone(), Some(signer.clone()))
        .await
        .expect("auth state");
    let auth = Arc::new(auth);

    // Build router with auth and oauth handlers mounted
    let router = Router::new()
        .nest("/auth", session::router(auth.clone()))
        .nest("/", oauth::router(auth.clone()));

    // We need an authorization code in the auth state for a client to exchange
    let code = "test-code-123".to_string();
    let principal = ark::server::auth::Principal {
        subject: "sub1".to_string(),
        email: Some("u@example.com".to_string()),
        name: Some("User One".to_string()),
        picture: None,
        provider: "local".to_string(),
        provider_kind: ark::server::auth::ProviderKind::Oidc,
        tenant_id: None,
        oid: None,
        groups: vec![],
        roles: vec![ark::server::roles::Role::User],
        is_admin: false,
    };
    let expires = SystemTime::now() + Duration::from_secs(600);
    auth.auth_codes.write().await.insert(
        code.clone(),
        (
            "client123".to_string(),
            principal.clone(),
            expires,
            None,
            Some("http://localhost/redirect".to_string()),
        ),
    );

    // Use tower::ServiceExt::oneshot to call the router handlers directly
    use tower::ServiceExt;

    // Call token endpoint with form data
    let form = format!(
        "grant_type=authorization_code&code={}&client_id=client123&redirect_uri={}&code_verifier=",
        code,
        urlencoding::encode("http://localhost/redirect")
    );
    let req = Request::builder()
        .method("POST")
        .uri("/token")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(form))
        .unwrap();

    let resp = router.clone().oneshot(req).await.expect("resp");
    assert_eq!(resp.status(), 200);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("body");
    let json: Value = serde_json::from_slice(&bytes).expect("json");
    let id_token = json
        .get("id_token")
        .and_then(|v| v.as_str())
        .expect("id_token");

    // Fetch JWKS
    let req = Request::builder()
        .method("GET")
        .uri("/auth/.well-known/jwks.json")
        .body(Body::empty())
        .unwrap();
    let resp = router.clone().oneshot(req).await.expect("jwks resp");
    assert_eq!(resp.status(), 200);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("jwks body");
    let jwks: Value = serde_json::from_slice(&bytes).expect("jwks json");

    // Validate id_token using auth.validate_id_token
    let jwk_set: jsonwebtoken::jwk::JwkSet = serde_json::from_value(jwks).expect("jwk_set");
    let claims = auth
        .validate_id_token(id_token, "client123", "http://localhost:8000", &jwk_set)
        .await
        .expect("validate");
    assert_eq!(claims.sub, "sub1");
}
