//! Core authentication logic and data structures.

use crate::config::models::{AuthConfig, IdentityProviderConfig};
use crate::server::roles::Role;
use crate::server::signing::{DynSigner, load_pem_signer_from_paths};
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
use rand::TryRngCore;
use rand::rngs::OsRng;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;

/// Represents the response from a token exchange request.
#[derive(Debug, Deserialize)]
pub struct TokenResponse {
    pub id_token: String,
    // The following fields are part of the standard token response but are not
    // currently used in this application. They are kept for completeness and
    // potential future use-cases like API access with the access_token.
    pub access_token: String,
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
    pub picture: Option<String>,
    pub tid: Option<String>,
    pub oid: Option<String>,
    /// Groups the user belongs to (from the 'groups' claim).
    pub groups: Option<Vec<String>>,
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
    /// Optional profile picture URL.
    pub picture: Option<String>,
    /// The identity provider that authenticated this user.
    pub provider: String,
    /// The canonical provider kind (required, no fallback).
    pub provider_kind: ProviderKind,
    /// Optional tenant identifier for multi-tenant providers.
    pub tenant_id: Option<String>,
    /// Optional object identifier for directory services.
    pub oid: Option<String>,
    /// Groups the user belongs to.
    pub groups: Vec<String>,
    /// Roles assigned to this principal.
    pub roles: Vec<Role>,
    /// Convenience flag for quick admin checks.
    pub is_admin: bool,
}

impl Principal {
    /// Generates a global unique identifier for this principal.
    ///
    /// Combines the provider and subject to create a globally unique identifier
    /// that can be used across different authentication systems.
    ///
    /// # Returns
    ///
    /// A string in the format "provider/tenant/userid".
    pub fn global_id(&self) -> String {
        // build a global id based on provider. Unwrap should be safe for
        // oid and tenant_id since those fields should've previously been asserted
        match self.provider_kind {
            ProviderKind::Microsoft => {
                format!(
                    "aad/{}/{}",
                    self.tenant_id.clone().unwrap(),
                    self.oid.clone().unwrap()
                )
            }
            ProviderKind::Google => {
                format!("gip/*/{}", self.subject)
            }
            ProviderKind::Oidc => {
                format!("oidc/*/{}", self.subject)
            }
        }
    }
}

/// Canonical provider kinds used by the server. This is required and there
/// is no longer any runtime string-parsing fallback — the provider kind must
/// be set at resolution time and carried into sessions.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum ProviderKind {
    Microsoft,
    Google,
    Oidc,
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
    /// The canonical provider kind (set during resolution).
    pub provider_kind: ProviderKind,
    /// Optional group configuration for role-based access control.
    pub groups: Option<crate::config::models::Groups>,
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
                // Validate required fields
                if config.client_id.trim().is_empty() {
                    return Err(anyhow!("Microsoft provider client_id cannot be empty"));
                }
                if config.authority.trim().is_empty() {
                    return Err(anyhow!("Microsoft provider authority cannot be empty"));
                }
                // Validate authority URL format
                if !config.authority.starts_with("http://")
                    && !config.authority.starts_with("https://")
                {
                    return Err(anyhow!(
                        "Microsoft provider authority must be a valid HTTP/HTTPS URL: {}",
                        config.authority
                    ));
                }

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
                    provider_kind: ProviderKind::Microsoft,
                    groups: config.groups.clone(),
                })
            }
            IdentityProvider::Google(config) => {
                // Validate required fields
                if config.client_id.trim().is_empty() {
                    return Err(anyhow!("Google provider client_id cannot be empty"));
                }

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
                    provider_kind: ProviderKind::Google,
                    groups: config.groups.clone(),
                })
            }
            IdentityProvider::Oidc(config) => {
                // Validate required fields
                if config.client_id.trim().is_empty() {
                    return Err(anyhow!("OIDC provider client_id cannot be empty"));
                }
                if config.authority.trim().is_empty() {
                    return Err(anyhow!("OIDC provider authority cannot be empty"));
                }
                // Validate authority URL format
                if !config.authority.starts_with("http://")
                    && !config.authority.starts_with("https://")
                {
                    return Err(anyhow!(
                        "OIDC provider authority must be a valid HTTP/HTTPS URL: {}",
                        config.authority
                    ));
                }

                let scopes = config
                    .scopes
                    .clone()
                    .unwrap_or_else(|| "openid profile".to_string());
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
                    provider_kind: ProviderKind::Oidc,
                    groups: config.groups.clone(),
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
    /// Original OAuth request query string for resuming after login.
    pub oauth_query: Option<String>,
}

/// Core authentication state and session management.
///
/// Manages user sessions, pending authentications, and provider configurations.
/// This is the central state object shared across all authentication operations.
/// Type alias for the value stored in the access_tokens map.
type AccessTokenEntry = (Principal, SystemTime, Option<String>);

/// Type alias for the value stored in the auth_codes map.
///
/// We store: (client_id, principal, expires_at, code_challenge, redirect_uri)
/// The `code_challenge` is optional (PKCE) and `redirect_uri` may be present
/// when the original authorize request provided it.
type AuthCodeEntry = (
    String,
    Principal,
    SystemTime,
    Option<String>,
    Option<String>,
);

#[derive(Clone)]
pub struct AuthState {
    /// Whether authentication is enabled.
    pub enabled: bool,
    /// HTTP client for making external requests (OIDC discovery, token exchange).
    pub http: Client,
    /// Active resolved provider configuration.
    pub active: Arc<RwLock<Option<ResolvedProvider>>>,
    /// Reference to the application state for database access.
    pub app_state: Arc<crate::state::ArkState>,
    /// Pending OAuth authentications (state -> PendingAuth).
    pub pending: Arc<RwLock<HashMap<String, PendingAuth>>>,
    /// OAuth authorization codes (code -> AuthCodeEntry).
    pub auth_codes: Arc<RwLock<HashMap<String, AuthCodeEntry>>>,
    /// OAuth access tokens (token -> (principal, expires_at, scope)).
    pub access_tokens: Arc<RwLock<HashMap<String, AccessTokenEntry>>>,
    /// Optional signer for issuing ID tokens (JWKS endpoint)
    pub signer: Option<DynSigner>,
}

impl std::fmt::Debug for AuthState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthState")
            .field("enabled", &self.enabled)
            .field("http", &"reqwest::Client")
            .field("active", &self.active)
            .field("app_state", &self.app_state)
            .field("pending", &self.pending)
            .field("auth_codes", &self.auth_codes)
            .field("access_tokens", &self.access_tokens)
            .finish()
    }
}

impl AuthState {
    /// Creates a new AuthState with explicit app state for database support.
    pub async fn new_with_state(
        config: &Option<AuthConfig>,
        app_state: Arc<crate::state::ArkState>,
        signer_override: Option<crate::server::signing::DynSigner>,
    ) -> Result<Self> {
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
            app_state,
            pending: Arc::new(RwLock::new(HashMap::new())),
            auth_codes: Arc::new(RwLock::new(HashMap::new())),
            access_tokens: Arc::new(RwLock::new(HashMap::new())),
            signer: signer_override.or_else(|| {
                // If override not provided, try env/config as before (best-effort)
                let key_path = std::env::var("ARK_TOKEN_SIGNING_KEY").ok();
                let cert_path = std::env::var("ARK_TOKEN_SIGNING_CERT").ok();
                if let Some(k) = key_path {
                    match load_pem_signer_from_paths(&k, cert_path.as_deref()) {
                        Ok(s) => Some(s),
                        Err(e) => {
                            tracing::warn!("Failed to initialize PEM signer from env: {}", e);
                            None
                        }
                    }
                } else {
                    None
                }
            }),
        })
    }

    /// Return JWKS if signer is configured
    pub fn jwks(&self) -> Option<serde_json::Value> {
        self.signer.as_ref().map(|s| s.jwks())
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
        // Get database reference without holding the guard across await
        let database = {
            if let Ok(database_guard) = self.app_state.database.read() {
                database_guard.as_ref().cloned()
            } else {
                None
            }
        };

        if let Some(database) = database {
            match database
                .get_session_record_async(session_id.to_string())
                .await
            {
                Ok(Some(session_record)) => {
                    // Check if session is still valid using chrono UTC timestamp
                    if chrono::Utc::now() < session_record.expiry_utc {
                        tracing::debug!("Session found in database: {}", session_id);
                        return Some(session_record.principal);
                    } else {
                        // Session expired, remove it immediately
                        tracing::info!("Session {} expired, removing from database", session_id);
                        if let Err(e) = database.delete_session_async(session_id.to_string()).await
                        {
                            tracing::warn!(
                                "Failed to delete expired session {}: {}",
                                session_id,
                                e
                            );
                        }
                        return None;
                    }
                }
                Ok(None) => {
                    tracing::trace!("Session not found in database: {}", session_id);
                }
                Err(e) => {
                    tracing::warn!("Database error retrieving session {}: {}", session_id, e);
                }
            }
        }

        None
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
        let session_id = random_urlsafe(32);

        // Get database reference without holding the guard across await
        let database = {
            if let Ok(database_guard) = self.app_state.database.read() {
                database_guard.as_ref().cloned()
            } else {
                None
            }
        };

        if let Some(database) = database {
            // Build a SessionRecord and persist via the model-based writer
            let expiry_system_time = SystemTime::now()
                .checked_add(ttl)
                .unwrap_or(SystemTime::now());
            let expiry_epoch = expiry_system_time
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;
            let expiry_utc = chrono::DateTime::<chrono::Utc>::from(expiry_system_time);

            let session_record = crate::server::persist::SessionRecord {
                session_id: session_id.clone(),
                principal: principal.clone(),
                expiry_utc,
                expiry_epoch,
                is_admin: principal.is_admin,
            };

            match database.save_session_record_async(session_record).await {
                Ok(()) => tracing::debug!("Session saved to database: {}", session_id),
                Err(e) => tracing::warn!("Failed to save session to database: {}", e),
            }
        }

        session_id
    }

    /// Cleans up expired sessions and pending authentications.
    ///
    /// Should be called periodically to maintain the state size.
    pub async fn cleanup(&self) {
        // Get database reference without holding the guard across await
        let database = {
            if let Ok(database_guard) = self.app_state.database.read() {
                database_guard.as_ref().cloned()
            } else {
                None
            }
        };

        // Clean up database sessions
        if let Some(database) = database {
            match database.cleanup_expired_sessions_async().await {
                Ok(count) => {
                    if count > 0 {
                        tracing::debug!("Cleaned up {} expired sessions from database", count);
                    }
                }
                Err(e) => {
                    tracing::warn!("Failed to cleanup expired sessions: {}", e);
                }
            }
        }

        // Clean up in-memory state
        let mut pending = self.pending.write().await;
        pending.retain(|_, auth| auth.created_at.elapsed() < Duration::from_secs(300)); // 5 minutes
        drop(pending);

        let system_now = SystemTime::now();
        let mut auth_codes = self.auth_codes.write().await;
        // AuthCodeEntry is (client_id, principal, expires_at, code_challenge, redirect_uri)
        auth_codes.retain(|_, (_, _, expires_at, _, _)| *expires_at > system_now);
        drop(auth_codes);

        let mut access_tokens = self.access_tokens.write().await;
        access_tokens.retain(|_, (_, expires_at, _)| *expires_at > system_now);
    }

    /// Removes a user session.
    pub async fn delete_session(&self, session_id: &str) -> bool {
        // Get database reference without holding the guard across await
        let database = {
            if let Ok(database_guard) = self.app_state.database.read() {
                database_guard.as_ref().cloned()
            } else {
                None
            }
        };

        if let Some(database) = database {
            match database.delete_session_async(session_id.to_string()).await {
                Ok(was_deleted) => {
                    if was_deleted {
                        tracing::debug!("Session deleted from database: {}", session_id);
                    }
                    return was_deleted;
                }
                Err(e) => {
                    tracing::warn!("Failed to delete session from database: {}", e);
                }
            }
        }

        false
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

    /// Fetches a user's profile photo from Microsoft Graph API.
    ///
    /// Uses the access token to retrieve the user's profile picture from Microsoft Graph API
    /// and returns it as a base64-encoded data URL.
    ///
    /// # Arguments
    ///
    /// * `access_token` - The OAuth access token for Microsoft Graph API.
    ///
    /// # Returns
    ///
    /// `Some(String)` containing the base64 data URL of the profile photo, or `None` if fetching fails.
    pub async fn fetch_microsoft_profile_photo(&self, access_token: &str) -> Option<String> {
        let url = "https://graph.microsoft.com/v1.0/me/photo/$value";

        let response = match self
            .http
            .get(url)
            .header("Authorization", format!("Bearer {}", access_token))
            .send()
            .await
        {
            Ok(resp) => resp,
            Err(e) => {
                tracing::warn!("Failed to fetch Microsoft profile photo: {}", e);
                return None;
            }
        };

        if !response.status().is_success() {
            tracing::warn!("Microsoft Graph API returned error: {}", response.status());
            return None;
        }

        let image_bytes = match response.bytes().await {
            Ok(bytes) => bytes,
            Err(e) => {
                tracing::warn!("Failed to read Microsoft profile photo bytes: {}", e);
                return None;
            }
        };

        // Convert to base64 data URL
        let base64_data =
            base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &image_bytes);
        Some(format!("data:image/jpeg;base64,{}", base64_data))
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

        // Check if this path requires admin privileges
        if path_requires_admin(path) && !principal.is_admin {
            tracing::warn!(
                "Access denied: user {} is not an admin",
                principal.global_id()
            );
            return (StatusCode::FORBIDDEN, "Admin privileges required").into_response();
        }

        let mut req = req;
        // Make principal available to downstream handlers
        req.extensions_mut().insert(principal);
        return next.run(req).await;
    }

    // Try OAuth bearer token for /mcp endpoints
    if path.starts_with("/mcp")
        && let Some(auth_header) = req.headers().get(header::AUTHORIZATION)
        && let Ok(auth_str) = auth_header.to_str()
        && let Some(token) = auth_str.strip_prefix("Bearer ")
        && let Some((principal, expires_at, _scope)) =
            auth.access_tokens.read().await.get(token).cloned()
    {
        // Check if token is still valid
        if SystemTime::now() <= expires_at {
            tracing::debug!(
                "Authenticated user via OAuth token: {}",
                principal.global_id()
            );

            // Check if this path requires admin privileges
            if path_requires_admin(path) && !principal.is_admin {
                tracing::warn!(
                    "Access denied: user {} is not an admin (OAuth token)",
                    principal.global_id()
                );
                return (StatusCode::FORBIDDEN, "Admin privileges required").into_response();
            }

            let mut req = req;
            req.extensions_mut().insert(principal);
            return next.run(req).await;
        } else {
            // Token expired, remove it
            auth.access_tokens.write().await.remove(token);
            tracing::debug!("OAuth token expired for path: {}", path);
        }
    }

    // Allow /token endpoint - OAuth clients authenticate via client credentials in request body
    if path == "/token" {
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
        "/authorize",
        "/.well-known/openid-configuration",
        "/.well-known/jwks.json",
        "/.well-known/oauth-authorization-server",
        "/.well-known/oauth-authorization-server/mcp",
        "/mcp/.well-known/openid-configuration",
        "/auth/status",
        "/auth/login",
        "/auth/logout",
        "/auth/callback",
        "/health",
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

/// Determines if a request path requires admin privileges.
///
/// Checks the path against a list of admin-only routes.
///
/// # Arguments
///
/// * `path` - The request path.
///
/// # Returns
///
/// `true` if admin privileges are required, `false` otherwise.
pub fn path_requires_admin(path: &str) -> bool {
    // Admin-only paths
    let admin_paths = ["/metrics"];

    // Check exact match
    admin_paths.contains(&path)
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
fn random_urlsafe(bytes: usize) -> String {
    let mut rng = OsRng;
    let mut buf = vec![0u8; bytes];
    rng.try_fill_bytes(&mut buf)
        .expect("OsRng failed to produce random bytes");
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
