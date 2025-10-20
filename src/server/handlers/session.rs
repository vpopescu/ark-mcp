//! HTTP handlers for authentication endpoints.
//!
//! This module contains all the Axum HTTP handlers for authentication-related
//! endpoints including login, logout, callback processing, and status checking.

use anyhow::Result;
use axum::http::HeaderMap;
use axum::{
    Json, Router,
    extract::{Extension, OriginalUri, Query},
    http::{Request, StatusCode, header},
    response::{Html, IntoResponse, Response},
    routing::get,
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
// OsRng is used locally in the helper; no top-level import needed here.
use rand::TryRngCore;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use urlencoding;

use crate::server::auth::{AuthState, PendingAuth, Principal, ProviderKind, ResolvedProvider};
use crate::server::roles::Role;
use jsonwebtoken::jwk::JwkSet;

/// Extract OAuth parameters from the query string
fn extract_oauth_params_from_query(query: &str) -> Option<String> {
    // Parse query string to find oauth_params
    for param in query.split('&') {
        if let Some((key, value)) = param.split_once('=')
            && key == "oauth_params"
        {
            return Some(value.to_string());
        }
    }
    None
}

/// Response structure for authentication status endpoint.
#[derive(serde::Serialize)]
struct StatusResponse {
    /// Current authentication status ("ok", "unauthenticated").
    status: &'static str,
    /// The authenticated user, if any.
    user: Option<Principal>,
    /// Additional auth information, if applicable.
    #[serde(skip_serializing_if = "Option::is_none")]
    auth: Option<&'static str>,
    /// Whether authentication is disabled.
    #[serde(skip_serializing_if = "Option::is_none")]
    auth_disabled: Option<bool>,
}

/// Creates the authentication router with all auth-related endpoints.
///
/// Sets up routes for `/status`, `/logout`, `/login`, `/callback` endpoints,
/// all protected by the provided authentication state.
///
/// # Arguments
///
/// * `state` - The shared authentication state.
///
/// # Returns
///
/// An Axum router configured with authentication routes.
pub fn router(state: Arc<AuthState>) -> Router {
    Router::new()
        .route("/status", get(status_handler))
        .route("/.well-known/jwks.json", get(jwks_handler))
        .route("/logout", get(logout_handler))
        .route("/login", get(login_handler))
        .route("/callback", get(callback_handler))
        .layer(Extension(state))
}

async fn jwks_handler(Extension(auth): Extension<Arc<AuthState>>) -> impl IntoResponse {
    if let Some(j) = auth.jwks() {
        (StatusCode::OK, Json(j)).into_response()
    } else {
        (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error":"no jwks configured"})),
        )
            .into_response()
    }
}

/// Checks for a valid session cookie and returns the authenticated user
/// information if found. Returns unauthorized status if no valid session exists.
///
/// # Arguments
///
/// * `auth` - The authentication state extension.
/// * `headers` - Request headers containing potential session cookie.
///
/// # Returns
///
/// JSON response with authentication status and user information.
async fn status_handler(
    Extension(auth): Extension<Arc<AuthState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !auth.enabled {
        // When auth is disabled, return a dummy admin user so frontend doesn't show login
        let dummy_principal = Principal {
            subject: "admin".to_string(),
            email: None,
            name: Some("Admin User".to_string()),
            picture: None,
            provider: "disabled".to_string(),
            provider_kind: ProviderKind::Oidc,
            tenant_id: None,
            oid: None,
            groups: vec![],
            roles: vec![Role::Admin],
            is_admin: true,
        };
        return (
            StatusCode::OK,
            Json(StatusResponse {
                status: "ok",
                user: Some(dummy_principal),
                auth: None,
                auth_disabled: Some(true),
            }),
        );
    }

    // Check for session cookie
    if let Some(cookie_header) = headers.get(header::COOKIE)
        && let Ok(cookie_str) = cookie_header.to_str()
    {
        for cookie_pair in cookie_str.split(';') {
            let cookie_pair = cookie_pair.trim();
            if let Some(session_id) = cookie_pair.strip_prefix("ark_session=")
                && let Some(principal) = auth.get_session(session_id).await
            {
                return (
                    StatusCode::OK,
                    Json(StatusResponse {
                        status: "ok",
                        user: Some(principal.clone()),
                        auth: None,
                        auth_disabled: Some(false),
                    }),
                );
            }
        }
    }

    (
        StatusCode::UNAUTHORIZED,
        Json(StatusResponse {
            status: "unauthenticated",
            user: None,
            auth: Some("required"),
            auth_disabled: Some(false),
        }),
    )
}

/// Handler for GET /auth/logout - logs out the current user.
///
/// Extracts the session ID from the cookie, removes the session from storage,
/// clears the session cookie, and redirects to IDP logout to terminate the
/// identity provider session as well.
///
/// # Arguments
///
/// * `auth` - The authentication state extension.
/// * `headers` - Request headers containing the session cookie.
/// * `uri` - The original request URI for building post-logout redirect.
/// * `req` - The full request for extracting headers.
///
/// # Returns
///
/// Redirect to IDP logout endpoint or JSON response.
async fn logout_handler(
    Extension(auth): Extension<Arc<AuthState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    req: Request<axum::body::Body>,
) -> impl IntoResponse {
    if !auth.enabled {
        return Json(serde_json::json!({"status":"ok"})).into_response();
    }

    // Extract session ID from cookie and remove it
    let mut session_removed = false;
    if let Some(cookie_header) = headers.get(header::COOKIE)
        && let Ok(cookie_str) = cookie_header.to_str()
    {
        for cookie_pair in cookie_str.split(';') {
            let cookie_pair = cookie_pair.trim();
            if let Some(session_id) = cookie_pair.strip_prefix("ark_session=") {
                // Remove session from database
                session_removed = auth.delete_session(session_id).await;
                if session_removed {
                    tracing::debug!("Logout: removed session {}", session_id);
                } else {
                    tracing::warn!("Logout: session {} not found in store", session_id);
                }
                break;
            }
        }
    }

    if !session_removed {
        tracing::debug!("Logout: no session found to remove");
    }

    // Get provider for logout URL construction
    let provider_guard = auth.active.read().await;
    let provider = match provider_guard.as_ref() {
        Some(p) => p.clone(),
        None => {
            // No provider configured, just clear cookie and return
            let mut response = Json(serde_json::json!({"status":"ok"})).into_response();
            response.headers_mut().insert(
                header::SET_COOKIE,
                "ark_session=deleted; Path=/; Max-Age=0; HttpOnly; Secure"
                    .parse()
                    .unwrap(),
            );
            return response;
        }
    };

    // Build post-logout redirect URI
    let scheme = uri.scheme().map(|s| s.as_str()).unwrap_or("http");
    let authority = if let Some(a) = uri.authority().map(|a| a.as_str()) {
        a
    } else if let Some(h) = req.headers().get("host").and_then(|h| h.to_str().ok()) {
        h
    } else {
        // Fallback to simple cookie clearing if we can't build redirect
        let mut response = Json(serde_json::json!({"status":"ok"})).into_response();
        response.headers_mut().insert(
            header::SET_COOKIE,
            "ark_session=deleted; Path=/; Max-Age=0; HttpOnly; Secure"
                .parse()
                .unwrap(),
        );
        return response;
    };

    let effective_scheme = get_effective_scheme_from_headers(&headers, scheme);
    let post_logout_redirect = format!("{}://{}/admin", effective_scheme, authority);

    // Build IDP logout URL
    let logout_url = match provider.provider_kind {
        ProviderKind::Microsoft => {
            format!(
                "{}/oauth2/v2.0/logout?post_logout_redirect_uri={}",
                provider.authority.trim_end_matches('/'),
                urlencoding::encode(&post_logout_redirect)
            )
        }
        ProviderKind::Google => {
            format!(
                "https://accounts.google.com/logout?continue={}",
                urlencoding::encode(&post_logout_redirect)
            )
        }
        ProviderKind::Oidc => {
            // Generic OIDC - try end_session_endpoint from discovery or fallback
            format!(
                "{}/connect/endsession?post_logout_redirect_uri={}",
                provider.authority.trim_end_matches('/'),
                urlencoding::encode(&post_logout_redirect)
            )
        }
    };

    // Create redirect response with cookie deletion
    let mut response = Response::builder()
        .status(StatusCode::FOUND)
        .header(header::LOCATION, logout_url)
        .body(axum::body::Body::empty())
        .unwrap()
        .into_response();

    response.headers_mut().insert(
        header::SET_COOKIE,
        "ark_session=deleted; Path=/; Max-Age=0; HttpOnly; Secure"
            .parse()
            .unwrap(),
    );

    response
}

/// Handler for GET /auth/login - initiates OAuth authorization flow.
///
/// Generates PKCE parameters, stores the state for callback validation,
/// and returns either a redirect URL or performs a direct redirect based
/// on the request parameters.
///
/// # Security Notes
/// - This endpoint should be rate-limited in production to prevent abuse
/// - PKCE parameters provide protection against authorization code interception
/// - State parameter prevents CSRF attacks
///
/// # Arguments
///
/// * `auth` - The authentication state extension.
/// * `uri` - The original request URI.
/// * `req` - The full request for parameter extraction.
///
/// # Returns
///
/// Redirect response or JSON with redirect URL.
async fn login_handler(
    Extension(auth): Extension<Arc<AuthState>>,
    OriginalUri(uri): OriginalUri,
    req: Request<axum::body::Body>,
) -> impl IntoResponse {
    if !auth.enabled {
        return (StatusCode::BAD_REQUEST, "auth disabled").into_response();
    }
    // Read current provider
    let provider_guard = auth.active.read().await;
    let provider = match provider_guard.as_ref() {
        Some(p) => p.clone(),
        None => return (StatusCode::BAD_REQUEST, "no provider").into_response(),
    };
    // Ensure authorization endpoint (perform inline discovery if needed to avoid race with startup async discovery)
    let authz = if let Some(u) = &provider.authorization_endpoint {
        u.clone()
    } else if provider.discovery {
        drop(provider_guard); // release before discovery mutation
        if let Err(e) =
            perform_discovery(auth.http.clone(), auth.active.clone(), &provider.authority).await
        {
            tracing::warn!(error=%e, "inline OIDC discovery failed in /auth/login");
        }
        // Re-acquire and check again
        let pg2 = auth.active.read().await;
        match pg2.as_ref().and_then(|p| p.authorization_endpoint.clone()) {
            Some(u2) => u2,
            None => {
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "authorization endpoint unavailable",
                )
                    .into_response();
            }
        }
    } else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "authorization endpoint unavailable",
        )
            .into_response();
    };
    // Parse query params for challenge, state, verifier (for public client)
    let query = req.uri().query().unwrap_or("");
    let params: HashMap<_, _> = url::form_urlencoded::parse(query.as_bytes())
        .into_owned()
        .collect();
    let (state_val, code_verifier, code_challenge) = match (
        params.get("challenge"),
        params.get("state"),
        params.get("verifier"),
    ) {
        (Some(ch), Some(st), Some(v)) => (st.clone(), v.clone(), ch.clone()),
        _ => generate_pkce_triplet(),
    };

    tracing::debug!(
        "Generated PKCE state: '{}', storing for callback",
        state_val
    );

    // Check if this is a direct redirect mode (from OAuth flow)
    let direct_redirect = req
        .uri()
        .query()
        .map(|q| q.contains("mode=redirect"))
        .unwrap_or(false);

    // Extract OAuth parameters if present
    let oauth_query = if direct_redirect {
        if let Some(query) = req.uri().query() {
            if let Some(oauth_params_encoded) = extract_oauth_params_from_query(query) {
                // URL decode the OAuth parameters
                match urlencoding::decode(&oauth_params_encoded) {
                    Ok(decoded) => Some(decoded.to_string()),
                    Err(_) => req.uri().query().map(|q| q.to_string()),
                }
            } else {
                req.uri().query().map(|q| q.to_string())
            }
        } else {
            None
        }
    } else {
        None
    };

    // store verifier for callback (future implementation)
    auth.pending.write().await.insert(
        state_val.clone(),
        PendingAuth {
            code_verifier,
            created_at: Instant::now(),
            redirect_to: if direct_redirect {
                Some("oauth".to_string())
            } else {
                None
            },
            oauth_query,
        },
    );
    let scopes = provider.scopes.join(" ");
    let scheme = uri.scheme().map(|s| s.as_str()).unwrap_or("http");
    let authority = if let Some(a) = uri.authority().map(|a| a.as_str()) {
        a
    } else if let Some(h) = req.headers().get("host").and_then(|h| h.to_str().ok()) {
        h
    } else {
        return (StatusCode::BAD_REQUEST, "Invalid request URI: missing host").into_response();
    };
    let effective_scheme = get_effective_scheme_from_headers(req.headers(), scheme);
    let redirect_uri = format!("{}://{}{}", effective_scheme, authority, "/auth/callback");

    tracing::debug!(
        "OAuth login: original_scheme={}, effective_scheme={}, authority={}, redirect_uri={}",
        scheme,
        effective_scheme,
        authority,
        redirect_uri
    );

    let mut url = format!(
        "{}?response_type=code&client_id={}&scope={}&state={}&code_challenge={}&code_challenge_method=S256&prompt=login",
        authz,
        urlencoding::encode(&provider.client_id),
        urlencoding::encode(&scopes),
        urlencoding::encode(&state_val),
        urlencoding::encode(&code_challenge)
    );
    url.push_str("&redirect_uri=");
    url.push_str(&urlencoding::encode(&redirect_uri));
    // Support optional mode=redirect for direct 302 flow
    let direct_redirect = req
        .uri()
        .query()
        .map(|q| q.contains("mode=redirect"))
        .unwrap_or(false);
    if direct_redirect {
        return Response::builder()
            .status(StatusCode::FOUND)
            .header(header::LOCATION, url)
            .body(axum::body::Body::empty())
            .unwrap()
            .into_response();
    }
    (StatusCode::OK, Json(serde_json::json!({ "redirect": url }))).into_response()
}

/// Handler for GET /auth/callback - processes OAuth authorization code.
///
/// Validates the callback parameters, exchanges the code for tokens (TODO),
/// creates a session, and redirects to the admin interface.
///
/// # Arguments
///
/// * `auth` - The authentication state extension.
/// * `query` - Query parameters from the callback URL.
///
/// # Returns
///
/// Redirect response to admin interface or error page.
#[axum::debug_handler]
async fn callback_handler(
    Extension(auth): Extension<Arc<AuthState>>,
    OriginalUri(uri): OriginalUri,
    headers: axum::http::HeaderMap,
    query: Query<HashMap<String, String>>,
) -> impl IntoResponse {
    if !auth.enabled {
        return Html(
            std::fs::read_to_string("www/dist/index.html")
                .unwrap_or_else(|_| r#"<h1>Auth disabled</h1>"#.to_string()),
        )
        .into_response();
    }

    let code = query.get("code");
    let state = query.get("state");
    let error = query.get("error");

    tracing::debug!(
        "OAuth callback received - code: {}, state: {}, error: {}",
        code.map(|_| "present").unwrap_or("missing"),
        state.map(|s| s.as_str()).unwrap_or("missing"),
        error.map(|e| e.as_str()).unwrap_or("none")
    );

    if let Some(err) = error {
        tracing::warn!("OAuth callback error: {}", err);
        return Html(format!(r#"<h1>Authentication Error</h1><p>{}</p>"#, err)).into_response();
    }

    if let (Some(code), Some(state)) = (code, state) {
        // Validate state parameter format for security
        if state.len() > 256
            || !state
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            tracing::warn!(
                "Invalid state parameter format in OAuth callback: '{}'",
                state
            );
            return Html("<h1>Invalid authentication state format</h1>").into_response();
        }
        let pending_auth = auth.pending.write().await.remove(state);

        if let Some(pending) = pending_auth {
            let provider = match auth.active.read().await.clone() {
                Some(p) => p,
                None => {
                    return Html("<h1>No active authentication provider configured.</h1>")
                        .into_response();
                }
            };

            let (token_url, jwks_uri) = match (
                provider.token_endpoint.as_ref(),
                provider.jwks_uri.as_ref(),
            ) {
                (Some(t), Some(j)) => (t.clone(), j.clone()),
                _ => {
                    return Html("<h1>Provider is not fully configured (missing token or jwks endpoint).</h1>").into_response();
                }
            };

            let scheme = uri.scheme_str().unwrap_or("http");
            let authority = if let Some(auth) = uri.authority() {
                auth.as_str()
            } else if let Some(host) = headers.get(header::HOST).and_then(|h| h.to_str().ok()) {
                host
            } else {
                return Html("<h1>Missing host header</h1>").into_response();
            };

            let effective_scheme = get_effective_scheme_from_headers(&headers, scheme);
            let redirect_uri = format!("{}://{}{}", effective_scheme, authority, "/auth/callback");

            // Exchange code for tokens
            let token_response = match auth
                .exchange_code_for_tokens(
                    code,
                    &pending.code_verifier,
                    &redirect_uri,
                    &provider.client_id,
                    provider.client_secret.as_deref(),
                    &token_url,
                )
                .await
            {
                Ok(res) => res,
                Err(e) => {
                    tracing::error!("Failed to exchange code for tokens: {}", e);
                    return Html(format!(
                        "<h1>Token Exchange Failed</h1><p>Could not get tokens: {}</p>",
                        e
                    ))
                    .into_response();
                }
            };

            // Fetch JWKS
            let jwks: JwkSet = match auth.http.get(&jwks_uri).send().await {
                Ok(resp) => match resp.json().await {
                    Ok(keys) => keys,
                    Err(e) => {
                        tracing::error!("Failed to parse JWKS: {}", e);
                        return Html(format!("<h1>JWKS Parse Error</h1><p>{}</p>", e))
                            .into_response();
                    }
                },
                Err(e) => {
                    tracing::error!("Failed to fetch JWKS: {}", e);
                    return Html(format!("<h1>JWKS Fetch Error</h1><p>{}</p>", e)).into_response();
                }
            };

            // Validate ID token
            let claims = match auth
                .validate_id_token(
                    &token_response.id_token,
                    &provider.client_id,
                    &provider.authority,
                    &jwks,
                )
                .await
            {
                Ok(c) => c,
                Err(e) => {
                    tracing::error!("ID token validation failed: {}", e);
                    return Html(format!("<h1>Invalid Token</h1><p>{}</p>", e)).into_response();
                }
            };

            // If this is Microsoft AAD, require tenant id (tid) and object id (oid) claims
            if provider.provider_kind == ProviderKind::Microsoft
                && (claims.tid.is_none() || claims.oid.is_none())
            {
                tracing::warn!(
                    "AAD token missing required claims tid/oid - tid={:?} oid={:?}",
                    claims.tid,
                    claims.oid
                );
                return Html(
                    "<h1>Invalid Token</h1><p>Missing required AAD claims (tid or oid)</p>",
                )
                .into_response();
            }

            // Create principal from claims (carry provider_kind into the session)
            let mut principal = Principal {
                subject: claims.sub,
                email: claims.email,
                name: claims.name,
                picture: claims.picture,
                provider: provider.authority.clone(),
                provider_kind: provider.provider_kind.clone(),
                tenant_id: claims.tid,
                oid: claims.oid,
                roles: vec![Role::User], // Default role
                is_admin: false,         // Will be updated below
                groups: claims.groups.unwrap_or_default(),
            };

            // Fetch profile picture for Microsoft users
            if provider.provider_kind == ProviderKind::Microsoft {
                if let Some(profile_photo) = auth
                    .fetch_microsoft_profile_photo(&token_response.access_token)
                    .await
                {
                    principal.picture = Some(profile_photo);
                    tracing::debug!(
                        "Fetched Microsoft profile photo for user: {}",
                        principal.subject
                    );
                } else {
                    tracing::debug!(
                        "Failed to fetch Microsoft profile photo for user: {}",
                        principal.subject
                    );
                }
            }

            // Assign roles based on group membership
            if let Some(groups) = &provider.groups {
                if let Some(admin_group) = &groups.admin {
                    tracing::debug!(
                        "Role assignment: user groups = {:?}, admin_group = {}, provider = {}",
                        principal.groups,
                        admin_group,
                        provider.authority
                    );
                    if principal.groups.iter().any(|g| g == admin_group) {
                        principal.roles.push(Role::Admin);
                        principal.is_admin = true;
                        tracing::debug!("User assigned Admin role");
                    } else {
                        tracing::debug!("User not in admin group, keeping User role only");
                    }
                } else {
                    tracing::debug!("No admin group configured, user gets User role only");
                }

                // Check user group restriction for non-admin users
                if !principal.is_admin
                    && let Some(users_group) = &groups.users
                    && !principal.groups.iter().any(|g| g == users_group)
                {
                    tracing::warn!(
                        "Access denied: user not in required users group '{}', user groups = {:?}",
                        users_group,
                        principal.groups
                    );
                    return Html(
                        "<h1>Access Denied</h1><p>You are not authorized to access this system. Please contact your administrator.</p>"
                            .to_string(),
                    )
                    .into_response();
                }
            } else {
                tracing::debug!("No groups configured, user gets User role only");
            }

            // Create session
            let session_id = auth.put_session(principal, Duration::from_secs(3600)).await;
            let cookie_value = format!(
                "ark_session={}; Path=/; HttpOnly; SameSite=Lax; Max-Age=3600; Secure",
                session_id
            );

            // Check if this specific auth was initiated from OAuth flow
            let redirect_location = if pending.redirect_to.as_deref() == Some("oauth") {
                if let Some(query) = &pending.oauth_query {
                    let oauth_path = format!("/authorize?{}", query);
                    tracing::debug!("Redirecting to OAuth path: {}", oauth_path);
                    oauth_path
                } else {
                    tracing::debug!("Redirecting to OAuth authorize endpoint");
                    "/authorize".to_string()
                }
            } else {
                tracing::debug!("Redirecting to root - no OAuth flow found");
                "/".to_string()
            };

            // Redirect with session cookie
            let mut response = Response::builder()
                .status(StatusCode::FOUND)
                .header(header::LOCATION, &redirect_location)
                .body(axum::body::Body::empty())
                .unwrap()
                .into_response();

            response.headers_mut().insert(
                header::SET_COOKIE,
                cookie_value
                    .parse()
                    .map_err(|e| {
                        tracing::error!("Failed to parse cookie value: {}", e);
                        e
                    })
                    .unwrap_or_else(|_| {
                        // Fallback to a basic cookie if parsing fails
                        format!("ark_session={}; Path=/; HttpOnly", session_id)
                            .parse()
                            .expect("Basic cookie format should always parse")
                    }),
            );

            return response;
        } else {
            tracing::warn!("Invalid or expired state in OAuth callback: '{}'", state);
            return Html("<h1>Invalid or expired authentication state</h1>").into_response();
        }
    }

    tracing::debug!("No code/state in callback, serving SPA");
    Html(
        std::fs::read_to_string("www/dist/index.html")
            .unwrap_or_else(|_| "<h1>index.html not found</h1>".to_string()),
    )
    .into_response()
}

// ------------------------- PKCE Helpers -------------------------

/// Determines the effective scheme for redirect URIs.
///
/// For security, prefers HTTPS for redirect URIs even if the request
/// came over HTTP.
///
/// # Arguments
///
/// * `_req` - The request (currently unused).
/// * `scheme` - The scheme from the request URI.
///
/// # Returns
///
/// The effective scheme string ("https" or the original scheme).
fn get_effective_scheme_from_headers(headers: &axum::http::HeaderMap, scheme: &str) -> String {
    // Check for forwarded protocol header first (for proxies)
    if let Some(forwarded_proto) = headers.get("x-forwarded-proto")
        && let Ok(proto) = forwarded_proto.to_str()
    {
        return proto.to_string();
    }

    // For OAuth redirect URIs, prefer HTTPS for security unless explicitly HTTP
    // This helps ensure OAuth providers accept our redirect URIs
    if scheme == "http" {
        "https".to_string()
    } else {
        scheme.to_string()
    }
}

/// Generates a PKCE triplet (state, verifier, challenge).
///
/// Creates cryptographically secure random values for OAuth PKCE flow.
///
/// # Returns
///
/// A tuple of (state, verifier, challenge) strings.
fn generate_pkce_triplet() -> (String, String, String) {
    let state = random_urlsafe(24);
    let verifier = random_urlsafe(48);
    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    let challenge = URL_SAFE_NO_PAD.encode(hasher.finalize());
    (state, verifier, challenge)
}

/// Generates a URL-safe random string.
///
/// Uses cryptographically secure random bytes and base64url encoding.
///
/// # Arguments
///
/// * `bytes` - Number of random bytes to generate.
///
/// # Returns
///
/// A URL-safe base64-encoded random string.
fn random_urlsafe(bytes: usize) -> String {
    use rand::rngs::OsRng;
    let mut rng = OsRng;
    let mut buf = vec![0u8; bytes];
    rng.try_fill_bytes(&mut buf)
        .expect("OsRng failed to produce random bytes");
    URL_SAFE_NO_PAD.encode(buf)
}

/// Perform OIDC discovery for the authority and update the active provider if successful.
///
/// Fetches the OpenID Connect configuration from the provider's well-known endpoint
/// and updates the active provider with discovered endpoints (JWKS URI, authorization
/// endpoint, token endpoint).
///
/// # Arguments
///
/// * `authority_http` - HTTP client to use for the discovery request.
/// * `active` - Shared reference to the active provider configuration.
/// * `authority` - The base authority URL for the provider.
///
/// # Returns
///
/// `Result<()>` indicating success or failure of the discovery process.
async fn perform_discovery(
    authority_http: reqwest::Client,
    active: Arc<RwLock<Option<ResolvedProvider>>>,
    authority: &str,
) -> Result<()> {
    let well_known = format!(
        "{}/.well-known/openid-configuration",
        authority.trim_end_matches('/')
    );
    let resp = authority_http
        .get(&well_known)
        .send()
        .await?
        .error_for_status()?;
    let doc: serde_json::Value = resp.json().await?;
    let jwks_uri = doc
        .get("jwks_uri")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let authorization_endpoint = doc
        .get("authorization_endpoint")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let token_endpoint = doc
        .get("token_endpoint")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    {
        let mut guard = active.write().await;
        if let Some(ref mut p) = *guard
            && p.authority == authority
        {
            if let Some(uri) = jwks_uri {
                p.jwks_uri = Some(uri);
            }
            if let Some(ep) = authorization_endpoint {
                p.authorization_endpoint = Some(ep);
            }
            if let Some(tok_ep) = token_endpoint {
                p.token_endpoint = Some(tok_ep);
            }
        }
    }
    Ok(())
}
