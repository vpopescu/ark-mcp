//! HTTP service implementation - starts HTTP(S) servers for management and MCP endpoints.
//!
//! This module provides the core server functionality for Ark, including:
//! - Management server (health checks, plugin API, admin console)
//! - MCP server (Model Context Protocol endpoints)
//! - TLS configuration and dual-protocol support
//! - Graceful shutdown handling
//!
//! The service supports both plain HTTP and HTTPS, with configurable TLS certificates.
//! Servers are started concurrently and can be shut down gracefully via signals.

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

/// Main entry point for starting all servers.
///
/// Initializes and starts both management and MCP servers based on configuration.
/// Handles TLS setup, router creation, and graceful shutdown.
///
/// # Arguments
/// * `config` - Application configuration
/// * `state` - Shared application state
///
/// # Returns
/// `Ok(())` on successful startup and shutdown, or an error
///
/// # Errors
/// Returns an error if server initialization or TLS setup fails
///
/// # Panics
/// May panic if critical state transitions fail
pub async fn start(config: &ArkConfig, state: std::sync::Arc<ArkState>) -> anyhow::Result<()> {
    state.set_state(ApplicationState::StartingNetwork);

    // Track if any management routes are enabled
    let mut enable_api_server = false;

    // Build auth state (lightweight if disabled)
    let auth_state = std::sync::Arc::new(crate::server::auth::AuthState::new(&config.auth).await?);

    // Start periodic cleanup task for auth state if authentication is enabled
    if auth_state.enabled {
        let auth_state_cleanup = auth_state.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(300)); // Every 5 minutes
            loop {
                interval.tick().await;
                auth_state_cleanup.cleanup().await;
                tracing::debug!(
                    "Performed auth state cleanup (expired sessions and pending auths)"
                );
            }
        });
    }

    let mut management_router = Router::new();

    // Enable configured management endpoints

    if state.is_plugin_api_enabled() {
        management_router = management_router.nest("/api", create_api_router(state.clone()));
        enable_api_server = true;
    }

    if state.is_health_api_enabled() {
        management_router = management_router.merge(create_health_router(
            state.clone(),
            &config
                .management_server
                .clone()
                .unwrap_or_default()
                .livez
                .path,
            &config
                .management_server
                .clone()
                .unwrap_or_default()
                .readyz
                .path,
        ));
        enable_api_server = true;
    }

    #[cfg(feature = "prometheus")]
    if state.is_prometheus_api_enabled() {
        management_router = management_router.route("/metrics", get(metrics_handler));
        enable_api_server = true;
    }

    if state.is_console_enabled() {
        management_router = management_router.nest("/admin", create_console_router(state.clone()));
        // Root path should redirect to /admin with transport parameter
        let transport = state.get_transport();
        let transport_str = match transport {
            McpTransport::Stdio => "stdio",
            McpTransport::Sse => "sse",
            McpTransport::StreamableHTTP => "streamable-http",
        };
        management_router = management_router.route(
            "/",
            get(move || async move {
                Redirect::temporary(&format!("/admin?transport={}", transport_str))
            }),
        );
        enable_api_server = true;
    }

    // Mount /auth routes if auth enabled (on separate router to avoid middleware)
    let mut auth_router = Router::new();
    if auth_state.enabled {
        auth_router = auth_router.nest(
            "/auth",
            crate::server::handlers::auth::router(auth_state.clone()),
        );
        enable_api_server = true;
    }

    if enable_api_server {
        // Apply auth middleware (will no-op for non-protected paths or if disabled)
        let auth_state_clone = auth_state.clone();
        management_router =
            management_router.layer(middleware::from_fn(
                move |req: Request<Body>, next: Next| {
                    let auth_state = auth_state_clone.clone();
                    async move {
                        crate::server::auth::check_auth(req, next, Extension(auth_state)).await
                    }
                },
            ));
        management_router = management_router.layer(middleware::from_fn(log_requests));
    }

    // Merge auth router (without middleware)
    management_router = management_router.merge(auth_router);

    // Load TLS material if configured
    let tls_key_material = get_tls_key_material(config).await;

    // Build Rustls config from material
    let rustls_config = match tls_key_material {
        Ok(material) => {
            let certs = rustls_pemfile::certs(&mut material.certs.as_slice())
                .collect::<Result<Vec<_>, _>>()
                .context("Failed to parse certificates")?;
            let key = rustls_pemfile::private_key(&mut material.key.as_slice())
                .context("Failed to parse private key")?
                .ok_or_else(|| anyhow::anyhow!("No private key found"))?;

            let config = rustls::ServerConfig::builder()
                .with_no_client_auth()
                .with_single_cert(certs, key)
                .context("Failed to create TLS config")?;

            Some(Arc::new(TlsAcceptor::from(Arc::new(config))))
        }
        Err(e) => {
            tracing::debug!("Failed to get TLS keys: {}", e);
            None
        }
    };

    if rustls_config.is_none() && !config.clone().tls.unwrap_or_default().silent_insecure {
        tracing::warn!("TLS keys could not be loaded, server is starting without TLS");
    }

    // Get bind addresses from config
    let management_bind_address = config
        .management_server
        .clone()
        .unwrap_or_default()
        .bind_address
        .unwrap_or_default();

    let mcp_bind_address = config
        .mcp_server
        .clone()
        .unwrap_or_default()
        .bind_address
        .unwrap_or_default();

    // CORS configuration
    let management_cors = if config
        .management_server
        .as_ref()
        .and_then(|m| m.cors.as_ref())
        .is_some()
    {
        Some(Cors {
            origins: config
                .management_server
                .as_ref()
                .and_then(|m| m.cors.clone())
                .unwrap(),
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
        })
    } else {
        None
    };

    let mcp_cors = if config
        .mcp_server
        .as_ref()
        .and_then(|m| m.cors.as_ref())
        .is_some()
    {
        Some(Cors {
            origins: config
                .mcp_server
                .as_ref()
                .and_then(|m| m.cors.clone())
                .unwrap(),
            allowed_headers: Some(vec![
                axum::http::HeaderName::from_static("content-type"),
                axum::http::HeaderName::from_static("authorization"),
                axum::http::HeaderName::from_static("x-requested-with"),
                axum::http::HeaderName::from_static("mcp-protocol-version"),
                axum::http::HeaderName::from_static("mcp-session-id"),
            ]),
            allowed_methods: Some(vec![axum::http::Method::POST, axum::http::Method::OPTIONS]),
            allow_credentials: true,
        })
    } else {
        None
    };

    tracing::debug!("Management endpoint CORS: {:?}", management_cors);
    tracing::debug!("MCP endpoint CORS: {:?}", mcp_cors);

    // CORS configuration
    tracing::debug!(
        "Management CORS from config: {:?}",
        config.management_server.as_ref().map(|m| &m.cors)
    );
    tracing::debug!(
        "MCP CORS from config: {:?}",
        config.mcp_server.as_ref().map(|m| &m.cors)
    );

    // Spawn server tasks
    let mut management_handle = None;

    let transport = state.get_transport();
    let state_for_match = state.clone();
    let rustls_for_match = rustls_config.clone();

    if enable_api_server {
        tracing::info!("Starting management server on {}", management_bind_address);
        management_handle = Some(tokio::spawn(async move {
            if let Err(e) = run_server(
                management_router,
                management_bind_address,
                rustls_config,
                management_cors,
                state,
            )
            .await
            {
                tracing::error!("Management server error: {:?}", e);
            }
        }));
    }

    // Spawn MCP server based on transport
    let mut mcp_handle = match transport {
        McpTransport::StreamableHTTP => {
            let mut mcp_router = create_mcp_router(state_for_match.clone());
            if auth_state.enabled {
                let auth_state_clone = auth_state.clone();
                mcp_router = mcp_router.layer(middleware::from_fn(
                    move |req: Request<Body>, next: Next| {
                        let auth_state = auth_state_clone.clone();
                        async move {
                            crate::server::auth::check_auth(req, next, Extension(auth_state)).await
                        }
                    },
                ));
            }
            Some(tokio::spawn(async move {
                if let Err(e) = run_server(
                    mcp_router,
                    mcp_bind_address,
                    rustls_for_match,
                    mcp_cors,
                    state_for_match,
                )
                .await
                {
                    tracing::error!("MCP server error: {:?}", e);
                }
            }))
        }
        McpTransport::Sse => {
            let addr: SocketAddr = resolve_bind_addr(&mcp_bind_address).await?;
            let sse_config = SseServerConfig {
                bind: addr,
                sse_path: "/sse".to_string(),
                post_path: "/message".to_string(),
                ct: CancellationToken::new(),
                sse_keep_alive: None,
            };
            let (sse_server, sse_router) = SseServer::new(sse_config);
            let state_for_closure = state_for_match.clone();
            let _ct = sse_server.with_service({
                move || McpHandler {
                    state: state_for_closure.clone(),
                }
            });
            let mut sse_router_with_auth = sse_router;
            if auth_state.enabled {
                let auth_state_clone = auth_state.clone();
                sse_router_with_auth = sse_router_with_auth.layer(middleware::from_fn(
                    move |req: Request<Body>, next: Next| {
                        let auth_state = auth_state_clone.clone();
                        async move {
                            crate::server::auth::check_auth(req, next, Extension(auth_state)).await
                        }
                    },
                ));
            }
            Some(tokio::spawn(async move {
                if let Err(e) = run_server(
                    sse_router_with_auth,
                    mcp_bind_address,
                    rustls_for_match,
                    mcp_cors,
                    state_for_match,
                )
                .await
                {
                    tracing::error!("MCP server error: {:?}", e);
                }
            }))
        }
        McpTransport::Stdio => Some(tokio::spawn(async move {
            info!("Starting MCP stdio server (stdin/stdout)");

            let service = McpHandler {
                state: state_for_match.clone(),
            };
            let io = stdio();
            let running = match serve_server(service, io).await {
                Ok(r) => r,
                Err(_e) => {
                    // Treat connection errors as normal shutdown
                    return;
                }
            };
            state_for_match.set_state(ApplicationState::Ready);
            let ct = running.cancellation_token();
            let waiting_fut = running.waiting();

            tokio::select! {
                _ = tokio::signal::ctrl_c() => {
                    info!("shutting down (Ctrl+C)");
                    ct.cancel();
                },
                res = waiting_fut => {
                    match res {
                        Ok(reason) => info!(?reason, "stdio server stopped"),
                        Err(_e) => {
                            // Suppress error logging for stdio shutdown
                        }
                    }
                }
            }
        })),
    };

    // Wait for shutdown signal or server errors
    let mut mcp_result = None;
    let mut management_result = None;

    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("Shutdown signal received");
        },

        res = async {
            match &mut management_handle {
                Some(handle) => handle.await,
                None => std::future::pending().await,
            }
        } => {
            management_result = Some(res);
        },

        res = async {
            match &mut mcp_handle {
                Some(handle) => handle.await,
                None => std::future::pending().await,
            }
        } => {
            mcp_result = Some(res);
        }
    }

    // Handle results from completed tasks
    if let Some(ref res) = management_result {
        match res {
            Ok(()) => tracing::debug!("Management server exited normally"),
            Err(join_err) => tracing::error!("Management server task panicked: {:?}", join_err),
        }
    }

    if let Some(ref res) = mcp_result {
        match res {
            Ok(()) => tracing::debug!("MCP server exited normally"),
            Err(join_err) => tracing::error!("MCP server task panicked: {:?}", join_err),
        }
    }

    // Graceful shutdown: abort tasks that are still running
    if let Some(handle) = management_handle
        && management_result.is_none()
    {
        handle.abort();
        let _ = handle.await;
    }
    if mcp_result.is_none()
        && let Some(handle) = mcp_handle
    {
        handle.abort();
        let _ = handle.await;
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
