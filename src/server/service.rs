//! HTTP service implementation - starts HTTP(S) servers for management and MCP endpoints.

use anyhow::{Context, bail};
use axum::{
    Extension, Json, Router,
    body::Body,
    http::Request,
    middleware::{self, Next},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
};
use hyper::server::conn::http1;
use hyper_util::rt::TokioIo;
use hyper_util::service::TowerToHyperService;
use rmcp::service::serve_server;
use rmcp::transport::sse_server::{SseServer, SseServerConfig};
use rmcp::transport::stdio;
use rmcp::transport::streamable_http_server::StreamableHttpServerConfig;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::tower::StreamableHttpService;
use serde::Deserialize;
use serde_json::{Value, to_value};
use std::{fs, net::SocketAddr, path::PathBuf, sync::Arc, time::Duration};
use tokio_rustls::TlsAcceptor;
use tokio_util::sync::CancellationToken;
use tower_http::{cors::CorsLayer, services::ServeDir, trace::TraceLayer};
use tracing::info;

use crate::{
    config::{ArkConfig, McpTransport},
    server::{
        handlers::{
            api::{
                create_plugin, delete_plugin, execute_plugin_tool, get_plugin_by_id, get_plugins,
            },
            health::{livez, readyz},
            oauth,
        },
        mcp::McpHandler,
    },
    state::{ApplicationState, ArkState},
};

/// CORS configuration for HTTP servers.
///
/// Allows specifying allowed origins for cross-origin requests.
/// Supports "*" for all origins or comma-separated list of specific origins.
#[derive(Debug, Deserialize, Clone, Default)]
pub struct Cors {
    /// Comma-separated list of allowed origins, or "*" for all origins.
    pub origins: String,
    /// Optional list of allowed headers (if None, allows any)
    #[serde(skip)]
    pub allowed_headers: Option<Vec<axum::http::HeaderName>>,
    /// Optional list of allowed methods (if None, allows any)
    #[serde(skip)]
    pub allowed_methods: Option<Vec<axum::http::Method>>,
    /// Whether to allow credentials
    #[serde(skip)]
    pub allow_credentials: bool,
}

impl Cors {
    /// Creates a CorsLayer from the configuration.
    ///
    /// Parses the origins string and configures the layer appropriately:
    /// - "*" allows all origins
    /// - Comma-separated list allows specific origins
    ///
    /// # Returns
    /// A configured CorsLayer with permissive methods and headers
    pub fn into_layer(self) -> CorsLayer {
        use tower_http::cors::{AllowHeaders, AllowMethods, AllowOrigin, ExposeHeaders};

        let allow_origin = if self.origins.trim() == "*" {
            AllowOrigin::any()
        } else {
            // Parse comma-separated origins
            let origin_list: Vec<_> = self
                .origins
                .split(',')
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .filter_map(|s| s.parse().ok())
                .collect();

            tracing::debug!(
                "Parsed CORS origins: {:?} from {:?}",
                origin_list,
                self.origins
            );

            if origin_list.is_empty() {
                tracing::warn!("No valid CORS origins specified, defaulting to allow all");
                AllowOrigin::any()
            } else if origin_list.len() == 1 {
                // For single origin, use exact matching
                AllowOrigin::exact(origin_list.into_iter().next().unwrap())
            } else {
                AllowOrigin::list(origin_list)
            }
        };

        let mut layer = CorsLayer::new().allow_origin(allow_origin);

        // Apply configured headers
        if let Some(headers) = self.allowed_headers {
            layer = layer.allow_headers(AllowHeaders::list(headers));
        } else {
            layer = layer.allow_headers(AllowHeaders::any());
        }

        // Apply configured methods
        if let Some(methods) = self.allowed_methods {
            layer = layer.allow_methods(AllowMethods::list(methods));
        } else {
            layer = layer.allow_methods(AllowMethods::any());
        }

        // Expose MCP headers so browser can read them from responses
        layer = layer.expose_headers(ExposeHeaders::list(vec![
            axum::http::HeaderName::from_static("mcp-session-id"),
            axum::http::HeaderName::from_static("mcp-protocol-version"),
        ]));

        // Apply credentials setting
        layer.allow_credentials(self.allow_credentials)
    }
}

/// TLS certificate and key material.
///
/// Holds the raw bytes for TLS certificate and private key,
/// loaded from PEM-encoded files.
struct TlsMaterial {
    /// PEM-encoded certificate chain.
    certs: Vec<u8>,
    /// PEM-encoded private key.
    key: Vec<u8>,
}

/// Standardized JSON response format for API endpoints.
///
/// Provides consistent error and success response structures
/// across all management API endpoints.
#[derive(serde::Serialize)]
pub struct StandardizedResponse {
    /// Error message if this is an error response.
    pub error: Option<String>,
    /// Success message if this is a success response.
    pub message: Option<String>,
    /// Additional context or details.
    pub additional: Option<String>,
    /// Whether this response represents an error.
    pub is_error: Option<bool>,
}

impl StandardizedResponse {
    /// Creates a standardized error response.
    ///
    /// # Arguments
    /// * `error` - The error message
    /// * `additional` - Optional additional context
    ///
    /// # Returns
    /// A JSON response with error formatting
    pub fn as_error(error: &str, additional: Option<&str>) -> Json<Value> {
        let response = StandardizedResponse {
            error: Some(error.to_string()),
            message: None,
            additional: additional.map(|s| s.to_string()),
            is_error: Some(true),
        };
        Json(to_value(response).unwrap())
    }
}

/// Checks if a file exists and is a regular file.
///
/// # Arguments
/// * `path` - Path to check
///
/// # Returns
/// `true` if the path exists and is a file, `false` otherwise
fn is_existing_file(path: &str) -> bool {
    let path = std::path::Path::new(path);
    path.exists() && path.is_file()
}

/// Loads TLS certificate and key material from configuration.
///
/// Reads PEM-encoded certificate and private key files specified
/// in the TLS configuration. Both files must exist and be non-empty.
///
/// # Arguments
/// * `config` - Application configuration containing TLS settings
///
/// # Returns
/// `Ok(TlsMaterial)` with loaded certificates and keys, or an error
///
/// # Errors
/// Returns an error if TLS is not configured, files don't exist,
/// or reading fails
async fn get_tls_key_material(config: &ArkConfig) -> anyhow::Result<Arc<TlsMaterial>> {
    let tls_cert = config
        .clone()
        .tls
        .unwrap_or_default()
        .cert
        .unwrap_or_default();
    let tls_key = config
        .clone()
        .tls
        .unwrap_or_default()
        .key
        .unwrap_or_default();
    let use_tls = !(tls_key.is_empty() || tls_cert.is_empty());

    if !use_tls {
        bail!("No TLS configuration");
    }
    if !tls_key.is_empty() && !is_existing_file(tls_key.as_str()) {
        tracing::debug!("TLS key file {} could not be found", tls_key);
        bail!("Missing or empty key file");
    }

    if !tls_cert.is_empty() && !is_existing_file(tls_cert.as_str()) {
        tracing::debug!("TLS cert file {} could not be found", tls_cert);
        bail!("Missing or empty cert file");
    }

    let cert_bytes =
        fs::read(&tls_cert).context(format!("Failed to read cert file {}", tls_cert))?;

    let key_bytes = fs::read(&tls_key).context(format!("Failed to read key file {}", tls_key))?;

    if cert_bytes.is_empty() || key_bytes.is_empty() {
        tracing::debug!(
            "Empty key or cert (certs={}, key={})",
            !cert_bytes.is_empty(),
            !key_bytes.is_empty()
        );
        bail!("Key or cert is empty")
    }

    Ok(Arc::new(TlsMaterial {
        certs: cert_bytes,
        key: key_bytes,
    }))
}

/// Handler for Prometheus metrics endpoint.
///
/// Returns metrics in Prometheus format when the `prometheus` feature is enabled.
/// This endpoint is only available when metrics collection is configured
/// and otherwise not disabled in configuration file.
#[cfg(feature = "prometheus")]
pub async fn metrics_handler() -> axum::response::Response {
    use axum::response::Response;
    use http_body_util::BodyExt;

    let hyper_response = crate::metrics::handler::make_metrics_response();

    // Convert hyper response to axum response
    let (parts, body) = hyper_response.into_parts();
    let body_bytes = body.collect().await.unwrap().to_bytes();

    Response::builder()
        .status(parts.status)
        .header(
            "content-type",
            parts
                .headers
                .get("content-type")
                .unwrap_or(&"text/plain".parse().unwrap()),
        )
        .body(axum::body::Body::from(body_bytes))
        .unwrap()
}

/// Builds authentication state and starts cleanup tasks if auth is enabled.
///
/// # Arguments
/// * `config` - Application configuration
/// * `state` - Shared application state
///
/// # Returns
/// AuthState instance
async fn build_auth_state_and_cleanup(
    config: &ArkConfig,
    state: std::sync::Arc<ArkState>,
) -> anyhow::Result<std::sync::Arc<crate::server::auth::AuthState>> {
    // Build signer if configured
    let signer_override = if let Some(ts) = &config.token_signing {
        if ts.source.as_deref() == Some("local") {
            let key_path = std::env::var("ARK_TOKEN_SIGNING_KEY")
                .ok()
                .or_else(|| ts.key.clone());
            let cert_path = std::env::var("ARK_TOKEN_SIGNING_CERT")
                .ok()
                .or_else(|| ts.cert.clone());
            if let Some(k) = key_path {
                match crate::server::signing::load_pem_signer_from_paths(&k, cert_path.as_deref()) {
                    Ok(s) => Some(s),
                    Err(e) => {
                        tracing::error!("Failed to initialize PEM signer at startup: {}", e);
                        return Err(e);
                    }
                }
            } else {
                None
            }
        } else {
            None
        }
    } else {
        None
    };

    // Build auth state
    let auth_state = std::sync::Arc::new(
        crate::server::auth::AuthState::new_with_state(
            &config.auth,
            state.clone(),
            signer_override,
        )
        .await?,
    );

    // Start cleanup tasks if auth enabled
    if auth_state.enabled {
        start_auth_cleanup_tasks(auth_state.clone());
    }

    Ok(auth_state)
}

/// Starts periodic cleanup tasks for authentication state.
///
/// # Arguments
/// * `auth_state` - Auth state to clean up
fn start_auth_cleanup_tasks(auth_state: std::sync::Arc<crate::server::auth::AuthState>) {
    // General cleanup every 5 minutes
    let auth_state_cleanup = auth_state.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(300));
        loop {
            interval.tick().await;
            auth_state_cleanup.cleanup().await;
            tracing::debug!("Performed auth state cleanup");
        }
    });

    // Session cleanup every minute
    let auth_state_session_cleanup = auth_state.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        loop {
            interval.tick().await;
            // Get database reference without holding guard across await
            let database = {
                if let Ok(database_guard) = auth_state_session_cleanup.app_state.database.read() {
                    database_guard.as_ref().cloned()
                } else {
                    None
                }
            };

            if let Some(database) = database {
                match database.cleanup_expired_sessions_async().await {
                    Ok(count) => {
                        if count > 0 {
                            tracing::info!("Cleaned up {} expired sessions", count);
                        }
                    }
                    Err(e) => tracing::warn!("Session cleanup failed: {}", e),
                }
            }
        }
    });
}

/// Builds the management router with conditional endpoints.
///
/// # Arguments
/// * `state` - Shared application state
/// * `config` - Application configuration
/// * `auth_state` - Auth state for middleware
///
/// # Returns
/// (Router, bool) - Router and whether API server is enabled
fn build_management_router(
    state: std::sync::Arc<ArkState>,
    config: &ArkConfig,
    auth_state: std::sync::Arc<crate::server::auth::AuthState>,
) -> (Router, bool) {
    let mut router = Router::new();
    let mut enable_api_server = false;
    let transport = state.get_transport();

    // Add conditional routes
    if state.is_plugin_api_enabled() {
        router = router.nest("/api", create_api_router(state.clone()));
        enable_api_server = true;
    }

    if state.is_health_api_enabled() {
        let livez_path = config
            .management_server
            .as_ref()
            .and_then(|m| m.livez.path.clone());
        let readyz_path = config
            .management_server
            .as_ref()
            .and_then(|m| m.readyz.path.clone());
        router = router.merge(create_health_router(
            state.clone(),
            &livez_path,
            &readyz_path,
        ));
        enable_api_server = true;
    }

    #[cfg(feature = "prometheus")]
    if state.is_prometheus_api_enabled() {
        router = router.route("/metrics", get(metrics_handler));
        enable_api_server = true;
    }

    if state.is_console_enabled() {
        router = router.nest("/admin", create_console_router(state.clone()));
        let transport_str = match transport {
            McpTransport::Stdio => "stdio",
            McpTransport::Sse => "sse",
            McpTransport::StreamableHTTP => "streamable-http",
        };
        router = router.route(
            "/",
            get(move || async move {
                Redirect::temporary(&format!("/admin?transport={}", transport_str))
            }),
        );
        enable_api_server = true;
    }

    // Add auth routes if enabled
    if auth_state.enabled && transport != McpTransport::Stdio {
        let auth_router = Router::new()
            .nest(
                "/auth",
                crate::server::handlers::session::router(auth_state.clone()),
            )
            .merge(oauth::router(auth_state.clone()));
        router = router.merge(auth_router);
        enable_api_server = true;
    }

    // Apply middleware if API server enabled
    if enable_api_server {
        let auth_state_clone = auth_state.clone();
        router = router
            .layer(middleware::from_fn(
                move |req: Request<Body>, next: Next| {
                    let auth_state = auth_state_clone.clone();
                    async move {
                        crate::server::auth::check_auth(req, next, Extension(auth_state)).await
                    }
                },
            ))
            .layer(middleware::from_fn(log_requests));
    }

    (router, enable_api_server)
}

/// Builds the MCP server task based on transport.
///
/// # Arguments
/// * `transport` - MCP transport type
/// * `state` - Shared application state
/// * `auth_state` - Auth state
/// * `mcp_bind_address` - Bind address for MCP server
/// * `rustls_config` - Optional TLS config
/// * `mcp_cors` - Optional CORS config
///
/// # Returns
/// Optional server task handle
async fn build_mcp_server_task(
    transport: McpTransport,
    state: std::sync::Arc<ArkState>,
    auth_state: std::sync::Arc<crate::server::auth::AuthState>,
    mcp_bind_address: String,
    rustls_config: Option<Arc<TlsAcceptor>>,
    mcp_cors: Option<Cors>,
) -> Option<tokio::task::JoinHandle<()>> {
    match transport {
        McpTransport::StreamableHTTP => {
            let mut router = create_mcp_router(state.clone());
            if auth_state.enabled {
                router = router
                    .merge(oauth::router(auth_state.clone()))
                    .nest(
                        "/auth",
                        crate::server::handlers::session::router(auth_state.clone()),
                    )
                    .layer(middleware::from_fn(
                        move |req: Request<Body>, next: Next| {
                            let auth_state = auth_state.clone();
                            async move {
                                crate::server::auth::check_auth(req, next, Extension(auth_state))
                                    .await
                            }
                        },
                    ));
            }
            Some(tokio::spawn(async move {
                if let Err(e) =
                    run_server(router, mcp_bind_address, rustls_config, mcp_cors, state).await
                {
                    tracing::error!("MCP server error: {:?}", e);
                }
            }))
        }
        McpTransport::Sse => {
            let addr: SocketAddr = resolve_bind_addr(&mcp_bind_address).await.ok()?;
            let sse_config = SseServerConfig {
                bind: addr,
                sse_path: "/sse".to_string(),
                post_path: "/message".to_string(),
                ct: CancellationToken::new(),
                sse_keep_alive: None,
            };
            let (sse_server, sse_router) = SseServer::new(sse_config);
            let state_for_closure = state.clone();
            let _ct = sse_server.with_service(move || McpHandler {
                state: state_for_closure.clone(),
            });
            let mut router = sse_router;
            if auth_state.enabled {
                router = router
                    .merge(oauth::router(auth_state.clone()))
                    .nest(
                        "/auth",
                        crate::server::handlers::session::router(auth_state.clone()),
                    )
                    .layer(middleware::from_fn(
                        move |req: Request<Body>, next: Next| {
                            let auth_state = auth_state.clone();
                            async move {
                                crate::server::auth::check_auth(req, next, Extension(auth_state))
                                    .await
                            }
                        },
                    ));
            }
            Some(tokio::spawn(async move {
                if let Err(e) =
                    run_server(router, mcp_bind_address, rustls_config, mcp_cors, state).await
                {
                    tracing::error!("MCP server error: {:?}", e);
                }
            }))
        }
        McpTransport::Stdio => Some(tokio::spawn(async move {
            info!("Starting MCP stdio server");
            let service = McpHandler {
                state: state.clone(),
            };
            let io = stdio();
            let running = match serve_server(service, io).await {
                Ok(r) => r,
                Err(_) => return,
            };
            state.set_state(ApplicationState::Ready);
            let ct = running.cancellation_token();
            let waiting_fut = running.waiting();
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {
                    info!("Shutting down (Ctrl+C)");
                    ct.cancel();
                },
                res = waiting_fut => {
                    if let Ok(reason) = res {
                        info!(?reason, "Stdio server stopped");
                    }
                }
            }
        })),
    }
}

/// Main entry point for starting all servers.
///
/// Orchestrates server setup and shutdown.
///
/// # Arguments
/// * `config` - Application configuration
/// * `state` - Shared application state
///
/// # Returns
/// `Ok(())` on success, or an error
///
/// # Errors
/// Returns an error if server initialization or TLS setup fails
///
/// # Panics
/// May panic if critical state transitions fail
pub async fn start(config: &ArkConfig, state: std::sync::Arc<ArkState>) -> anyhow::Result<()> {
    state.set_state(ApplicationState::StartingNetwork);

    // Build auth state and cleanup
    let auth_state = build_auth_state_and_cleanup(config, state.clone()).await?;

    // Build management router
    let (management_router, enable_api_server) =
        build_management_router(state.clone(), config, auth_state.clone());

    // TLS and CORS setup
    let tls_key_material = get_tls_key_material(config).await;
    let rustls_config = tls_key_material.ok().and_then(|material| {
        let certs = rustls_pemfile::certs(&mut material.certs.as_slice())
            .collect::<Result<Vec<_>, _>>()
            .context("Failed to parse certificates")
            .ok()?;
        let key = rustls_pemfile::private_key(&mut material.key.as_slice())
            .context("Failed to parse private key")
            .ok()??;
        let config = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(certs, key)
            .ok()?;
        Some(Arc::new(TlsAcceptor::from(Arc::new(config))))
    });

    let management_bind_address = config
        .management_server
        .as_ref()
        .and_then(|m| m.bind_address.as_ref())
        .unwrap_or(&"127.0.0.1:8000".to_string())
        .clone();
    let mcp_bind_address = config
        .mcp_server
        .as_ref()
        .and_then(|m| m.bind_address.as_ref())
        .unwrap_or(&"127.0.0.1:3000".to_string())
        .clone();

    let management_cors = config
        .management_server
        .as_ref()
        .and_then(|m| m.cors.as_ref())
        .map(|origins| Cors {
            origins: origins.clone(),
            allowed_headers: Some(vec![
                axum::http::HeaderName::from_static("content-type"),
                axum::http::HeaderName::from_static("authorization"),
                axum::http::HeaderName::from_static("x-requested-with"),
                axum::http::HeaderName::from_static("mcp-session-id"),
            ]),
            allowed_methods: Some(vec![
                axum::http::Method::POST,
                axum::http::Method::OPTIONS,
                axum::http::Method::GET,
                axum::http::Method::DELETE,
            ]),
            allow_credentials: true,
        });

    let mcp_cors = config
        .mcp_server
        .as_ref()
        .and_then(|m| m.cors.as_ref())
        .map(|origins| Cors {
            origins: origins.clone(),
            allowed_headers: Some(vec![
                axum::http::HeaderName::from_static("content-type"),
                axum::http::HeaderName::from_static("authorization"),
                axum::http::HeaderName::from_static("x-requested-with"),
                axum::http::HeaderName::from_static("mcp-protocol-version"),
                axum::http::HeaderName::from_static("mcp-session-id"),
            ]),
            allowed_methods: Some(vec![axum::http::Method::POST, axum::http::Method::OPTIONS]),
            allow_credentials: true,
        });

    // Spawn servers
    let state_clone = state.clone();
    let rustls_clone = rustls_config.clone();
    let mut management_handle = if enable_api_server {
        Some(tokio::spawn(async move {
            if let Err(e) = run_server(
                management_router,
                management_bind_address,
                rustls_clone,
                management_cors,
                state_clone,
            )
            .await
            {
                tracing::error!("Management server error: {:?}", e);
            }
        }))
    } else {
        None
    };

    let state_for_mcp = state.clone();
    let rustls_for_mcp = rustls_config.clone();
    let mut mcp_handle = build_mcp_server_task(
        state.get_transport(),
        state_for_mcp,
        auth_state,
        mcp_bind_address,
        rustls_for_mcp,
        mcp_cors,
    )
    .await;

    // Shutdown handling (unchanged)
    let mut mcp_result = None;
    let mut management_result = None;

    tokio::select! {
        _ = tokio::signal::ctrl_c() => tracing::info!("Shutdown signal received"),
        res = async {
            match &mut management_handle {
                Some(handle) => handle.await,
                None => std::future::pending().await,
            }
        } => management_result = Some(res),
        res = async {
            match &mut mcp_handle {
                Some(handle) => handle.await,
                None => std::future::pending().await,
            }
        } => mcp_result = Some(res),
    }

    // Handle results and abort remaining tasks
    if let Some(ref res) = management_result {
        match res {
            Ok(()) => tracing::debug!("Management server exited normally"),
            Err(e) => tracing::error!("Management server task panicked: {:?}", e),
        }
    }
    if let Some(ref res) = mcp_result {
        match res {
            Ok(()) => tracing::debug!("MCP server exited normally"),
            Err(e) => tracing::error!("MCP server task panicked: {:?}", e),
        }
    }

    if management_result.is_none()
        && let Some(h) = management_handle
    {
        h.abort();
        let _ = h.await;
    }
    if mcp_result.is_none()
        && let Some(h) = mcp_handle
    {
        h.abort();
        let _ = h.await;
    }

    Ok(())
}

/// Runs a single server instance with the given configuration.
///
/// Binds to the specified address and serves the router, with optional TLS.
/// Sets application state to Ready when server starts successfully.
///
/// # Arguments
/// * `router` - The Axum router to serve
/// * `addr` - Bind address as string (e.g., "127.0.0.1:8000")
/// * `tls_config` - Optional TLS configuration
/// * `cors_config` - Optional CORS configuration (currently unused)
/// * `state` - Shared application state
///
/// # Returns
/// `Ok(())` on successful server operation, or an error
///
/// # Errors
/// Returns an error if binding fails or server encounters issues
async fn run_server(
    router: Router,
    addr: String,
    tls_acceptor: Option<Arc<TlsAcceptor>>,
    cors_config: Option<Cors>,
    state: std::sync::Arc<ArkState>,
) -> anyhow::Result<()> {
    let sock_addr: SocketAddr = addr.parse()?;

    // Apply CORS layer if configured
    let app = if let Some(cors) = cors_config {
        router.layer(cors.into_layer())
    } else {
        router
    };

    // Add tracing layer for request logging
    let app = app.layer(TraceLayer::new_for_http());

    tracing::debug!("Listening on {}", sock_addr);

    let listener = tokio::net::TcpListener::bind(sock_addr).await?;

    if let Some(acceptor) = tls_acceptor {
        state.clone().set_state(ApplicationState::Ready);
        tracing::info!("Starting TLS server on https://{}", sock_addr);

        loop {
            let (stream, _) = listener.accept().await?;
            let acceptor = acceptor.clone();
            let app = app.clone();

            tokio::spawn(async move {
                let tls_stream = match acceptor.accept(stream).await {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::warn!("TLS accept failed: {}", e);
                        return;
                    }
                };
                let service = TowerToHyperService::new(app);
                let _ = http1::Builder::new()
                    .serve_connection(TokioIo::new(tls_stream), service)
                    .await;
            });
        }
    } else {
        state.clone().set_state(ApplicationState::Ready);
        tracing::info!("Starting plain HTTP server on http://{}", sock_addr);
        axum::serve(listener, app).await?;
    }

    Ok(())
}

/// Middleware to log incoming requests and outgoing responses.
///
/// Logs request method and URI on entry, response status on exit.
/// Useful for debugging and monitoring server activity.
async fn log_requests(req: Request<Body>, next: Next) -> Response {
    tracing::debug!(
        "Received request: {} {} from {:?}",
        req.method(),
        req.uri(),
        req.headers().get("origin")
    );

    let is_admin = req.uri().path().starts_with("/admin");

    // Log request body if trace level (skip for /admin)
    let req = if tracing::level_enabled!(tracing::Level::TRACE) && !is_admin {
        let (parts, body) = req.into_parts();
        let body_bytes = match axum::body::to_bytes(body, usize::MAX).await {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!("Failed to read request body: {}", e);
                return Response::builder().status(400).body(Body::empty()).unwrap();
            }
        };
        if let Ok(body_str) = std::str::from_utf8(&body_bytes) {
            tracing::trace!("Request body: {}", body_str);
        } else {
            tracing::trace!("Request body: <binary data, {} bytes>", body_bytes.len());
        }
        Request::from_parts(parts, Body::from(body_bytes))
    } else {
        req
    };

    let response = next.run(req).await;

    // Log response body if trace level (skip for /admin)
    let response = if tracing::level_enabled!(tracing::Level::TRACE) && !is_admin {
        let (parts, body) = response.into_parts();
        let body_bytes = match axum::body::to_bytes(body, usize::MAX).await {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!("Failed to read response body: {}", e);
                return Response::from_parts(parts, Body::empty());
            }
        };
        if let Ok(body_str) = std::str::from_utf8(&body_bytes) {
            tracing::trace!("Response body: {}", body_str);
        } else {
            tracing::trace!("Response body: <binary data, {} bytes>", body_bytes.len());
        }
        Response::from_parts(parts, Body::from(body_bytes))
    } else {
        response
    };

    tracing::debug!("Sending response: {} for request", response.status());
    response
}

/// Creates the router for plugin management API endpoints.
///
/// Includes routes for listing, creating, deleting, and executing plugins.
/// All routes are prefixed with `/api`.
///
/// # Arguments
/// * `state` - Shared application state
///
/// # Returns
/// Configured router with API routes
pub fn create_api_router(state: std::sync::Arc<ArkState>) -> Router {
    tracing::debug!("Creating plugin API router");
    Router::new()
        .route("/plugins", get(get_plugins).post(create_plugin))
        .route("/plugins/{id}", get(get_plugin_by_id).delete(delete_plugin))
        .route("/plugins/{id}/tools", post(execute_plugin_tool))
        .with_state(state)
}

/// Creates the router for health check endpoints.
///
/// Includes liveness and readiness probes at `/livez` and `/readyz`.
///
/// # Arguments
/// * `state` - Shared application state
/// * `livez_path` - Optional custom liveness path
/// * `readyz_path` - Optional custom readiness path
///
/// # Returns
/// Configured router with health check routes
pub fn create_health_router(
    state: std::sync::Arc<ArkState>,
    livez_path: &Option<String>,
    readyz_path: &Option<String>,
) -> Router {
    tracing::debug!("Creating health API router");
    Router::new()
        .route(
            &livez_path.clone().unwrap_or("/livez".to_string()),
            get(livez),
        )
        .route(
            &readyz_path.clone().unwrap_or("/readyz".to_string()),
            get(readyz),
        )
        .with_state(state)
}

/// Creates the router for the admin console SPA.
///
/// Serves static assets and provides client-side routing fallback.
/// Assumes static files are built to `www/dist/`.
///
/// # Arguments
/// * `state` - Shared application state
///
/// # Returns
/// Configured router for admin console
pub fn create_console_router(state: std::sync::Arc<ArkState>) -> Router {
    tracing::debug!("Creating console router");
    let transport = state.get_transport();
    let transport_str = match transport {
        McpTransport::Stdio => "stdio",
        McpTransport::Sse => "sse",
        McpTransport::StreamableHTTP => "streamable-http",
    };
    Router::new()
        // Serve static assets like JS, CSS, images
        .nest_service("/assets", ServeDir::new("www/dist/assets"))
        // Serve fonts from public/fonts (copied to dist/fonts during build)
        .nest_service("/fonts", ServeDir::new("www/dist/fonts"))
        // Serve index.html at admin root, with transport parameter redirect if missing
        .route(
            "/",
            get({
                let transport_str_copy = transport_str;
                move |query: axum::extract::Query<std::collections::HashMap<String, String>>| async move {
                    // Check if transport parameter is already present
                    if query.contains_key("transport") {
                        // Transport parameter exists, serve the SPA
                        let index_path = PathBuf::from("www/dist/index.html");
                        Html(
                            std::fs::read_to_string(index_path)
                                .unwrap_or_else(|_| "<h1>index.html not found</h1>".to_string()),
                        ).into_response()
                    } else {
                        // No transport parameter, redirect to include it
                        Redirect::temporary(&format!("/admin?transport={}", transport_str_copy)).into_response()
                    }
                }
            }),
        )
        // Catch-all route for SPA client-side routing (includes transport param handling)
        .route(
            "/{*path}",
            get(|| async {
                let index_path = PathBuf::from("www/dist/index.html");
                Html(
                    std::fs::read_to_string(index_path)
                        .unwrap_or_else(|_| "<h1>index.html not found</h1>".to_string()),
                )
            }),
        )
        .with_state(state)
}

/// Creates the router for the MCP server.
///
/// Uses the rmcp library to provide Model Context Protocol endpoints
/// at the `/mcp` path.
///
/// # Arguments
/// * `state` - Shared application state
///
/// # Returns
/// Configured router with MCP service
fn create_mcp_router(state: std::sync::Arc<ArkState>) -> Router {
    tracing::debug!("Creating MCP router");
    // Build the rmcp streamable HTTP tower service and mount it at /mcp.
    let app_state = state.clone();
    let handler_factory = move || -> Result<McpHandler, std::io::Error> {
        Ok(McpHandler {
            state: app_state.clone(),
        })
    };
    let session_mgr = LocalSessionManager::default();
    let cfg = StreamableHttpServerConfig::default();
    let svc = StreamableHttpService::new(handler_factory, Arc::new(session_mgr), cfg);

    Router::new().nest_service("/mcp", svc)
}

/// Resolve a "host:port" string to a SocketAddr, allowing hostnames like "localhost:9999".
async fn resolve_bind_addr(addr: &str) -> anyhow::Result<SocketAddr> {
    use std::net::ToSocketAddrs;
    addr.to_socket_addrs()?
        .next()
        .ok_or_else(|| anyhow::anyhow!("No address found for {}", addr))
}
