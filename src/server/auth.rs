//! Core authentication logic and data structures.
//!
//! This module provides the core authentication functionality including
//! session management, provider resolution, bearer token validation,
//! and authentication state management.

use crate::config::components::{AuthConfig, IdentityProviderConfig};
use anyhow::{Context, Result, anyhow};
use axum::{
    Extension,
    extract::Request,
    http::{StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use jsonwebtoken::{DecodingKey, Validation, decode, decode_header};
use rand::RngCore;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

/// Represents the response from a token exchange request.
#[derive(Debug, Deserialize)]
pub struct TokenResponse {
    pub id_token: String,
    // The following fields are part of the standard token response but are not
    // currently used in this application. They are kept for completeness and
    // potential future use-cases like API access with the access_token.
    // pub access_token: String,
    // pub refresh_token: Option<String>,
    // pub expires_in: u64,
    // pub scope: String,
}

/// Represents the claims in an OIDC ID token.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdTokenClaims {
    pub iss: String,
    pub sub: String,
    pub aud: String,
    pub exp: u64,
    pub iat: u64,
    pub email: Option<String>,
    pub name: Option<String>,
    pub tid: Option<String>,
    pub oid: Option<String>,
}

/// Represents an authenticated user principal.
///
/// Contains the core identity information extracted from authentication tokens
/// or OAuth provider responses.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Principal {
    /// The subject identifier (unique within the provider).
    pub subject: String,
    /// Optional email address.
    pub email: Option<String>,
    /// Optional display name.
    pub name: Option<String>,
    /// The identity provider that authenticated this user.
    pub provider: String,
    /// Optional tenant identifier for multi-tenant providers.
    pub tenant_id: Option<String>,
    /// Optional object identifier for directory services.
    pub oid: Option<String>,
}

impl Principal {
    /// Generates a global unique identifier for this principal.
    ///
    /// Combines the provider and subject to create a globally unique identifier
    /// that can be used across different authentication systems.
    ///
    /// # Returns
    ///
    /// A string in the format "provider:subject".
    pub fn global_id(&self) -> String {
        format!("{}:{}", self.provider, self.subject)
    }
}

/// Resolved identity provider configuration.
///
/// Contains the complete configuration for an identity provider after
/// discovery and validation, including all necessary endpoints and settings.
#[derive(Clone, Debug)]
pub struct ResolvedProvider {
    /// The base authority URL for this provider.
    pub authority: String,
    /// The client ID registered with the provider.
    pub client_id: String,
    /// The client secret for confidential clients.
    #[allow(dead_code)]
    pub client_secret: Option<String>,
    /// List of OAuth scopes to request.
    pub scopes: Vec<String>,
    /// Whether to perform OIDC discovery.
    pub discovery: bool,
    /// Optional pre-configured JWKS URI.
    pub jwks_uri: Option<String>,
    /// Optional pre-configured authorization endpoint.
    pub authorization_endpoint: Option<String>,
    /// Optional pre-configured token endpoint.
    pub token_endpoint: Option<String>,
}

/// Supported identity provider types.
///
/// Defines the different types of identity providers that can be configured,
/// each with their specific configuration requirements.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IdentityProvider {
    /// Microsoft Entra ID (Azure AD) provider.
    Microsoft(IdentityProviderConfig),
    /// Google Identity provider.
    Google(IdentityProviderConfig),
    /// Generic OIDC provider.
    Oidc(IdentityProviderConfig),
}

impl IdentityProvider {
    /// Resolves this provider configuration into a ResolvedProvider.
    ///
    /// Performs any necessary validation and sets default values for the provider.
    ///
    /// # Returns
    ///
    /// A `ResolvedProvider` with complete configuration.
    pub fn resolve(&self) -> Result<ResolvedProvider> {
        match self {
            IdentityProvider::Microsoft(config) => {
                let scopes = config
                    .scopes
                    .clone()
                    .unwrap_or_else(|| "openid profile email".to_string());
                let scopes_vec: Vec<String> =
                    scopes.split_whitespace().map(|s| s.to_string()).collect();
                Ok(ResolvedProvider {
                    authority: config.authority.clone(),
                    client_id: config.client_id.clone(),
                    client_secret: config.client_secret.clone(),
                    scopes: scopes_vec,
                    discovery: config.discovery,
                    jwks_uri: config.jwks_uri.clone(),
                    authorization_endpoint: config.authorization_endpoint.clone(),
                    token_endpoint: config.token_endpoint.clone(),
                })
            }
            IdentityProvider::Google(config) => {
                let scopes = config
                    .scopes
                    .clone()
                    .unwrap_or_else(|| "openid profile email".to_string());
                let scopes_vec: Vec<String> =
                    scopes.split_whitespace().map(|s| s.to_string()).collect();
                Ok(ResolvedProvider {
                    authority: "https://accounts.google.com".to_string(),
                    client_id: config.client_id.clone(),
                    client_secret: config.client_secret.clone(),
                    scopes: scopes_vec,
                    discovery: config.discovery,
                    jwks_uri: config.jwks_uri.clone(),
                    authorization_endpoint: config.authorization_endpoint.clone(),
                    token_endpoint: config.token_endpoint.clone(),
                })
            }
            IdentityProvider::Oidc(config) => {
                let scopes = config
                    .scopes
                    .clone()
                    .unwrap_or_else(|| "openid".to_string());
                let scopes_vec: Vec<String> =
                    scopes.split_whitespace().map(|s| s.to_string()).collect();
                Ok(ResolvedProvider {
                    authority: config.authority.clone(),
                    client_id: config.client_id.clone(),
                    client_secret: config.client_secret.clone(),
                    scopes: scopes_vec,
                    discovery: config.discovery,
                    jwks_uri: config.jwks_uri.clone(),
                    authorization_endpoint: config.authorization_endpoint.clone(),
                    token_endpoint: config.token_endpoint.clone(),
                })
            }
        }
    }
}

/// Pending authentication state for OAuth flows.
///
/// Stores temporary state during OAuth authorization code flow,
/// including the PKCE verifier and creation timestamp.
#[derive(Clone, Debug)]
pub struct PendingAuth {
    /// The PKCE code verifier for this authentication attempt.
    pub code_verifier: String,
    /// When this pending auth was created.
    #[allow(dead_code)]
    pub created_at: Instant,
    /// Optional redirect URL after successful authentication.
    #[allow(dead_code)]
    pub redirect_to: Option<String>,
}

/// Core authentication state and session management.
///
/// Manages user sessions, pending authentications, and provider configurations.
/// This is the central state object shared across all authentication operations.
#[derive(Clone, Debug)]
pub struct AuthState {
    /// Whether authentication is enabled.
    pub enabled: bool,
    /// HTTP client for making external requests (OIDC discovery, token exchange).
    pub http: Client,
    /// Active resolved provider configuration.
    pub active: Arc<RwLock<Option<ResolvedProvider>>>,
    /// Active user sessions (session_id -> (principal, expiry)).
    pub sessions: Arc<RwLock<HashMap<String, (Principal, Instant)>>>,
    /// Pending OAuth authentications (state -> PendingAuth).
    pub pending: Arc<RwLock<HashMap<String, PendingAuth>>>,
}

impl AuthState {
    /// Creates a new AuthState from configuration.
    ///
    /// Initializes the authentication state with the provided configuration,
    /// sets up HTTP client, and resolves the identity provider.
    ///
    /// # Arguments
    ///
    /// * `config` - Authentication configuration.
    ///
    /// # Returns
    ///
    /// A new `AuthState` instance.
    pub async fn new(config: &Option<AuthConfig>) -> Result<Self> {
        let http = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .context("Failed to create HTTP client")?;

        let active = if let Some(auth_config) = config {
            if let Some(provider_name) = &auth_config.provider {
                // Find the provider config by name
                let provider_config = auth_config
                    .providers
                    .iter()
                    .find(|p| p.name == *provider_name)
                    .ok_or_else(|| {
                        anyhow!("Provider '{}' not found in providers list", provider_name)
                    })?;

                // Create IdentityProvider enum from config
                let identity_provider =
                    if provider_name.starts_with("microsoft") || provider_name == "entra" {
                        IdentityProvider::Microsoft(provider_config.clone())
                    } else if provider_name.starts_with("google") {
                        IdentityProvider::Google(provider_config.clone())
                    } else {
                        IdentityProvider::Oidc(provider_config.clone())
                    };

                Some(identity_provider.resolve()?)
            } else {
                None
            }
        } else {
            None
        };

        Ok(Self {
            enabled: config.as_ref().map(|c| c.enabled).unwrap_or(false),
            http,
            active: Arc::new(RwLock::new(active)),
            sessions: Arc::new(RwLock::new(HashMap::new())),
            pending: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    /// Retrieves a user session by session ID.
    ///
    /// Looks up the session and checks if it hasn't expired.
    ///
    /// # Arguments
    ///
    /// * `session_id` - The session identifier.
    ///
    /// # Returns
    ///
    /// `Some(Principal)` if the session is valid, `None` otherwise.
    pub async fn get_session(&self, session_id: &str) -> Option<Principal> {
        let sessions = self.sessions.read().await;
        if let Some((principal, expiry)) = sessions.get(session_id) {
            if Instant::now() < *expiry {
                Some(principal.clone())
            } else {
                // Session expired, remove it
                drop(sessions);
                self.sessions.write().await.remove(session_id);
                None
            }
        } else {
            None
        }
    }

    /// Creates a new user session.
    ///
    /// Generates a unique session ID, stores the principal with the given TTL,
    /// and returns the session ID.
    ///
    /// # Arguments
    ///
    /// * `principal` - The authenticated user principal.
    /// * `ttl` - Time-to-live for the session.
    ///
    /// # Returns
    ///
    /// The generated session ID.
    pub async fn put_session(&self, principal: Principal, ttl: Duration) -> String {
        let session_id = random_urlsafe(&mut rand::rng(), 32);
        let expiry = Instant::now() + ttl;
        self.sessions
            .write()
            .await
            .insert(session_id.clone(), (principal, expiry));
        session_id
    }

    /// Cleans up expired sessions and pending authentications.
    ///
    /// Should be called periodically to maintain the state size.
    pub async fn cleanup(&self) {
        let now = Instant::now();
        let mut sessions = self.sessions.write().await;
        sessions.retain(|_, (_, expiry)| *expiry > now);

        let mut pending = self.pending.write().await;
        pending.retain(|_, auth| auth.created_at.elapsed() < Duration::from_secs(300)); // 5 minutes
    }

    /// Exchanges an authorization code for an ID token and access token.
    pub async fn exchange_code_for_tokens(
        &self,
        code: &str,
        pkce_verifier: &str,
        redirect_uri: &str,
        client_id: &str,
        client_secret: Option<&str>,
        token_url: &str,
    ) -> Result<TokenResponse, anyhow::Error> {
        let mut params = HashMap::new();
        params.insert("grant_type", "authorization_code");
        params.insert("code", code);
        params.insert("redirect_uri", redirect_uri);
        params.insert("client_id", client_id);
        params.insert("code_verifier", pkce_verifier);

        // Add client_secret if provided (required for Web apps)
        if let Some(secret) = client_secret {
            params.insert("client_secret", secret);
        }

        let response = self.http.post(token_url).form(&params).send().await?;

        if !response.status().is_success() {
            let error_text = response.text().await?;
            return Err(anyhow::anyhow!(
                "Failed to exchange code for token: {}",
                error_text
            ));
        }

        let token_response: TokenResponse = response.json().await?;
        Ok(token_response)
    }

    /// Validates the ID token's signature and claims.
    pub async fn validate_id_token(
        &self,
        id_token: &str,
        client_id: &str,
        issuer_url: &str,
        jwks: &jsonwebtoken::jwk::JwkSet,
    ) -> Result<IdTokenClaims, anyhow::Error> {
        let header = decode_header(id_token)?;

        let kid = header
            .kid
            .ok_or_else(|| anyhow::anyhow!("Token header does not contain 'kid' (Key ID)"))?;

        let jwk = jwks
            .keys
            .iter()
            .find(|k| k.common.key_id.as_deref() == Some(&kid))
            .ok_or_else(|| anyhow::anyhow!("No matching JWK found for Key ID '{}'", kid))?;

        let decoding_key = DecodingKey::from_jwk(jwk)?;
        let mut validation = Validation::new(header.alg);
        validation.set_audience(&[client_id]);
        validation.set_issuer(&[issuer_url]);

        let token_data = decode::<IdTokenClaims>(id_token, &decoding_key, &validation)?;

        // Additional security: check token is not too old (max 1 hour)
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        if now > token_data.claims.exp {
            return Err(anyhow::anyhow!("ID token has expired"));
        }

        if now < token_data.claims.iat || (now - token_data.claims.iat) > 3600 {
            return Err(anyhow::anyhow!(
                "ID token issued time is invalid or too old"
            ));
        }

        Ok(token_data.claims)
    }
}

/// Authentication middleware function.
///
/// Checks if the request requires authentication and validates the session.
/// Returns an error response if authentication is required but not provided or invalid.
///
/// # Arguments
///
/// * `req` - The incoming request.
/// * `next` - The next middleware in the chain.
/// * `auth` - The authentication state.
///
/// # Returns
///
/// The response from the next middleware or an authentication error.
pub async fn check_auth(
    req: Request,
    next: Next,
    Extension(auth): Extension<Arc<AuthState>>,
) -> Response {
    if !auth.enabled {
        return next.run(req).await;
    }

    let path = req.uri().path();

    // Check if this path requires authentication
    if !path_requires_auth(path) {
        return next.run(req).await;
    }

    // Try session cookie first
    if let Some(cookie_header) = req.headers().get(header::COOKIE)
        && let Ok(cookie_str) = cookie_header.to_str()
        && let Some(principal) = extract_session_user_from_cookie(&auth, cookie_str).await
    {
        tracing::debug!("Authenticated user via session: {}", principal.global_id());
        return next.run(req).await;
    }

    // Authentication required but not provided
    tracing::debug!("Authentication required for path: {}", path);
    (StatusCode::UNAUTHORIZED, "Authentication required").into_response()
}

/// Determines if a request path requires authentication.
///
/// Checks the path against a list of protected and unprotected routes.
///
/// # Arguments
///
/// * `path` - The request path.
///
/// # Returns
///
/// `true` if authentication is required, `false` otherwise.
pub fn path_requires_auth(path: &str) -> bool {
    // Public paths that don't require auth
    let public_paths = [
        "/",
        "/auth/status",
        "/auth/login",
        "/auth/logout",
        "/auth/callback",
        "/health",
        "/metrics",
        "/livez",
        "/readyz",
        "/admin",
    ];

    // Check exact match for root path
    if path == "/" {
        return false;
    }

    // Check if path starts with any public path
    for public in &public_paths {
        if *public != "/" && path.starts_with(public) {
            return false;
        }
    }

    // Check for static assets
    if path.starts_with("/assets/") || path.starts_with("/static/") {
        return false;
    }

    // Everything else requires auth
    true
}

// ------------------------- Helper Functions -------------------------

/// Generates a URL-safe random string.
///
/// Uses cryptographically secure random bytes and base64url encoding.
///
/// # Arguments
///
/// * `rng` - Random number generator.
/// * `bytes` - Number of random bytes to generate.
///
/// # Returns
///
/// A URL-safe base64-encoded random string.
fn random_urlsafe(rng: &mut impl RngCore, bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    rng.fill_bytes(&mut buf);
    URL_SAFE_NO_PAD.encode(buf)
}

/// Extracts session user from cookie string.
///
/// Parses the cookie header and looks up the session ID.
///
/// # Arguments
///
/// * `state` - Authentication state.
/// * `cookie_str` - Cookie header string.
///
/// # Returns
///
/// `Some(Principal)` if a valid session is found, `None` otherwise.
pub async fn extract_session_user_from_cookie(
    state: &AuthState,
    cookie_str: &str,
) -> Option<Principal> {
    let id = cookie_str
        .split(';')
        .find_map(|p| p.trim().strip_prefix("ark_session="))
        .map(|s| s.to_string())?;
    state.get_session(&id).await
}
