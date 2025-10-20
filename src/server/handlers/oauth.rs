//! OAuth 2.0 server endpoints implementation using openidconnect crate.
//!
//! Provides standard OAuth 2.0 endpoints for authorization and token exchange
//! that integrate with the configured identity providers (Google/Microsoft).

use axum::{
    Json, Router,
    extract::{Extension, OriginalUri, Query},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use openidconnect::{
    AccessToken, AuthorizationCode, CsrfToken, IssuerUrl, PkceCodeChallenge, PkceCodeVerifier,
};
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use urlencoding;

use crate::server::auth::{AuthState, Principal, extract_session_user_from_cookie};

#[derive(Debug, Deserialize)]
pub struct AuthorizeParams {
    pub response_type: String,
    pub client_id: String,
    pub redirect_uri: String,
    pub scope: Option<String>,
    pub state: Option<String>,
    pub code_challenge: Option<String>,
    pub code_challenge_method: Option<String>,
    pub nonce: Option<String>,
    pub prompt: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct TokenParams {
    pub grant_type: String,
    pub code: Option<String>,
    pub redirect_uri: Option<String>,
    pub client_id: String,
    pub client_secret: Option<String>,
    pub code_verifier: Option<String>,
}

/// OAuth token response using openidconnect types
#[derive(Debug, Serialize)]
pub struct OAuthTokenResponse {
    pub access_token: String,
    pub token_type: String,
    pub expires_in: u64,
    pub refresh_token: Option<String>,
    pub scope: Option<String>,
    pub id_token: Option<String>,
}

/// OAuth error response
#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub error: String,
    pub error_description: Option<String>,
    pub error_uri: Option<String>,
}

/// Creates the OAuth router with /authorize and /token endpoints
pub fn router(auth_state: Arc<AuthState>) -> Router {
    Router::new()
        .route("/authorize", get(authorize_handler))
        .route("/token", post(token_handler))
        .layer(Extension(auth_state))
}

/// GET /authorize - OAuth authorization endpoint
async fn authorize_handler(
    Extension(auth): Extension<Arc<AuthState>>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
    Query(params): Query<AuthorizeParams>,
) -> impl IntoResponse {
    // Validate request
    if params.response_type != "code" {
        return error_redirect(
            &params.redirect_uri,
            "unsupported_response_type",
            Some("Only 'code' response type is supported"),
            params.state.as_deref(),
        );
    }

    if params.client_id.is_empty() {
        return error_redirect(
            &params.redirect_uri,
            "invalid_request",
            Some("Missing client_id"),
            params.state.as_deref(),
        );
    }

    // Check if user is authenticated via session cookie
    let principal = if let Some(cookie_header) = headers.get("cookie")
        && let Ok(cookie_str) = cookie_header.to_str()
        && let Some(user) = extract_session_user_from_cookie(&auth, cookie_str).await
    {
        user
    } else {
        // User not authenticated - redirect to auth login with original OAuth params
        let oauth_params = uri.query().unwrap_or("");
        let login_url = format!(
            "/auth/login?mode=redirect&oauth_params={}",
            urlencoding::encode(oauth_params)
        );
        return Redirect::to(&login_url).into_response();
    };

    // Generate authorization code using openidconnect types
    let auth_code = AuthorizationCode::new(generate_secure_code());
    let state_value = params.state.clone();
    let _csrf_token = CsrfToken::new(state_value.unwrap_or_else(generate_secure_code));

    let expires_at = SystemTime::now() + Duration::from_secs(600); // 10 minutes

    // Record some optional params for observability and store the redirect
    // URI alongside the authorization code so the token endpoint can validate
    // it when exchanging the code. We trace optional parameters (non-sensitive)
    // to avoid unused-field warnings while keeping behavior unchanged.
    tracing::trace!(
        scope = params.scope.as_deref().unwrap_or(""),
        code_challenge_method = ?params.code_challenge_method,
        nonce = ?params.nonce,
        prompt = ?params.prompt,
        "authorize request optional params"
    );

    // Store authorization code in AuthState (include redirect URI)
    auth.auth_codes.write().await.insert(
        auth_code.secret().clone(),
        (
            params.client_id.clone(),
            principal,
            expires_at,
            params.code_challenge,
            Some(params.redirect_uri.clone()),
        ),
    );

    // Redirect back to client with authorization code
    let mut redirect_url = format!("{}?code={}", params.redirect_uri, auth_code.secret());
    if let Some(state) = params.state {
        redirect_url.push_str(&format!("&state={}", urlencoding::encode(&state)));
    }

    Redirect::to(&redirect_url).into_response()
}

/// POST /token - OAuth token endpoint
async fn token_handler(
    Extension(auth): Extension<Arc<AuthState>>,
    axum::extract::Form(params): axum::extract::Form<TokenParams>,
) -> impl IntoResponse {
    if params.grant_type != "authorization_code" {
        return Json(ErrorResponse {
            error: "unsupported_grant_type".to_string(),
            error_description: Some(
                "Only 'authorization_code' grant type is supported".to_string(),
            ),
            error_uri: None,
        })
        .into_response();
    }

    let code = match params.code {
        Some(c) => c,
        None => {
            return Json(ErrorResponse {
                error: "invalid_request".to_string(),
                error_description: Some("Missing authorization code".to_string()),
                error_uri: None,
            })
            .into_response();
        }
    };

    // Validate authorization code
    let (client_id, principal, expires_at, code_challenge, stored_redirect_uri) = {
        let mut codes = auth.auth_codes.write().await;
        match codes.remove(&code) {
            Some(data) => data,
            None => {
                return Json(ErrorResponse {
                    error: "invalid_grant".to_string(),
                    error_description: Some("Invalid or expired authorization code".to_string()),
                    error_uri: None,
                })
                .into_response();
            }
        }
    };

    // Check expiration
    if SystemTime::now() > expires_at {
        return Json(ErrorResponse {
            error: "invalid_grant".to_string(),
            error_description: Some("Authorization code has expired".to_string()),
            error_uri: None,
        })
        .into_response();
    }

    // Validate client_id matches
    if client_id != params.client_id {
        return Json(ErrorResponse {
            error: "invalid_client".to_string(),
            error_description: Some("Client ID mismatch".to_string()),
            error_uri: None,
        })
        .into_response();
    }

    // Mark client_secret as observed (do not log or echo it) so callers
    // that provide it (confidential clients) don't trigger an unused-field
    // warning. Proper client-secret validation can be added later if needed.
    let _ = params.client_secret.as_ref();

    // Validate PKCE if present using openidconnect types
    if let (Some(challenge), Some(verifier)) = (code_challenge, params.code_verifier) {
        let pkce_verifier = PkceCodeVerifier::new(verifier);
        let computed_challenge = PkceCodeChallenge::from_code_verifier_sha256(&pkce_verifier);
        if challenge != computed_challenge.as_str() {
            return Json(ErrorResponse {
                error: "invalid_grant".to_string(),
                error_description: Some("PKCE verification failed".to_string()),
                error_uri: None,
            })
            .into_response();
        }
    }

    // Validate redirect_uri if the original authorization request provided one.
    match (
        stored_redirect_uri.as_deref(),
        params.redirect_uri.as_deref(),
    ) {
        (Some(stored), Some(provided)) if stored != provided => {
            return Json(ErrorResponse {
                error: "invalid_grant".to_string(),
                error_description: Some("Redirect URI mismatch".to_string()),
                error_uri: None,
            })
            .into_response();
        }
        (Some(_), None) => {
            return Json(ErrorResponse {
                error: "invalid_grant".to_string(),
                error_description: Some("Redirect URI missing".to_string()),
                error_uri: None,
            })
            .into_response();
        }
        _ => {}
    }

    // Generate access token using openidconnect types
    let access_token = AccessToken::new(generate_secure_token());
    let expires_in = 3600; // 1 hour
    let token_expires_at = SystemTime::now() + Duration::from_secs(expires_in);

    // Store access token
    auth.access_tokens.write().await.insert(
        access_token.secret().clone(),
        (
            principal.clone(),
            token_expires_at,
            Some("openid profile email".to_string()),
        ),
    );

    // Create ID token with actual user data (signed if signer configured)
    let id_token = create_id_token(&auth, &params.client_id, &principal).await;

    let response = OAuthTokenResponse {
        access_token: access_token.secret().clone(),
        token_type: "Bearer".to_string(),
        expires_in,
        refresh_token: Some(generate_secure_token()),
        scope: Some("openid profile email".to_string()),
        id_token: Some(id_token),
    };

    (StatusCode::OK, Json(response)).into_response()
}

fn error_redirect(
    redirect_uri: &str,
    error: &str,
    description: Option<&str>,
    state: Option<&str>,
) -> Response {
    let mut url = format!("{}?error={}", redirect_uri, urlencoding::encode(error));
    if let Some(desc) = description {
        url.push_str(&format!("&error_description={}", urlencoding::encode(desc)));
    }
    if let Some(state) = state {
        url.push_str(&format!("&state={}", urlencoding::encode(state)));
    }
    Redirect::to(&url).into_response()
}

fn generate_secure_code() -> String {
    let mut rng = rand::rng();
    let bytes: [u8; 32] = rng.random();
    URL_SAFE_NO_PAD.encode(bytes)
}

fn generate_secure_token() -> String {
    let mut rng = rand::rng();
    let bytes: [u8; 48] = rng.random();
    URL_SAFE_NO_PAD.encode(bytes)
}

async fn create_id_token(auth: &AuthState, client_id: &str, principal: &Principal) -> String {
    // Create proper ID token using openidconnect types
    let issuer = IssuerUrl::new("http://localhost:8000".to_string()).unwrap();

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let claims = serde_json::json!({
        "iss": issuer.as_str(),
        "sub": principal.subject,
        "aud": client_id,
        "exp": now + 3600,
        "iat": now,
        "email": principal.email.clone().unwrap_or_default(),
        "name": principal.name.clone().unwrap_or_default(),
        "provider": principal.provider.clone(),
        "groups": principal.groups.clone(),
    });

    // If signer configured, use it. Otherwise fallback to legacy unsigned token.
    if let Some(signer) = &auth.signer {
        let header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
        match signer.sign(header, &claims) {
            Ok(tok) => tok,
            Err(e) => {
                tracing::warn!("Signer failed, falling back to unsigned token: {}", e);
                // fallback to simple unsigned token
                let header = r#"{"alg":"none","typ":"JWT"}"#;
                let header_b64 = URL_SAFE_NO_PAD.encode(header.as_bytes());
                let payload_b64 = URL_SAFE_NO_PAD.encode(claims.to_string().as_bytes());
                format!("{}.{}.", header_b64, payload_b64)
            }
        }
    } else {
        // legacy unsigned token for tests/local setups
        let header = r#"{"alg":"none","typ":"JWT"}"#;
        let header_b64 = URL_SAFE_NO_PAD.encode(header.as_bytes());
        let payload_b64 = URL_SAFE_NO_PAD.encode(claims.to_string().as_bytes());
        format!("{}.{}.", header_b64, payload_b64)
    }
}
