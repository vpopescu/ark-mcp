//! Ark MCP server entry point.
//!
//! This module contains the main entry point for the Ark MCP (Model Context Protocol) server.
//! It handles command-line argument parsing, configuration loading, plugin initialization,
//! and server startup.
//!
//! # Responsibilities
//!
//! - Parse CLI arguments and environment variables (via Clap)
//! - Load configuration from file, environment, and CLI overrides
//! - Initialize logging and application state
//! - Apply configuration to runtime state
//! - Load and register plugins
//! - Start MCP and management servers
//!
//! # Application Lifecycle
//!
//! The server follows a structured initialization sequence:
//! 1. **Unknown** → Parse CLI args and initialize state
//! 2. **Initializing** → Load configuration and apply to state
//! 3. **LoadingPlugins** → Load configured plugins and register tools
//! 4. **StartingNetwork** → Initialize network services and servers
//! 5. **Ready** → Server is fully operational
//! 6. **Terminating** → Server is shutting down

mod config;
mod metrics;
mod plugins;
mod server;
mod state;
mod utility;

use crate::{
    server::service::start,
    state::{ApplicationState, ArkState},
};
use clap::{CommandFactory, FromArgMatches, Parser};
use config::{ArkConfig, models::McpTransport};
use tracing_subscriber::fmt;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// CLI arguments definition for the Ark MCP server.
///
/// This struct defines all command-line arguments and environment variables
/// supported by the server. Field documentation is used by Clap to generate
/// help text, so keep them in rustdoc format.
#[derive(Parser, Debug, Clone)]
#[command(name = "ark", version, about = "Ark MCP server", long_about = None)]
struct Args {
    /// Config file path (overrides default path and ARK_CONFIG_PATH)
    #[arg(long = "config-file", value_name = "FILE", env = "ARK_CONFIG_PATH")]
    config_file: Option<std::path::PathBuf>,

    /// Transport protocol for MCP communication
    #[arg(
        long = "transport",
        value_name = "TRANSPORT",
        env = "ARK_TRANSPORT",
        value_enum,
        default_value_t = McpTransport::Stdio
    )]
    transport: McpTransport,

    /// MCP server bind address override (optional)
    #[arg(
        long = "mcp-bind-address",
        value_name = "MCP_BIND_ADDRESS",
        env = "ARK_MCP_BIND_ADDRESS",
        required = false
    )]
    mcp_bind_address: Option<String>,

    /// Management API server bind address override (optional)
    #[arg(
        long = "management-bind-address",
        value_name = "MANAGEMENT_BIND_ADDRESS",
        env = "ARK_MANAGEMENT_BIND_ADDRESS",
        required = false
    )]
    management_bind_address: Option<String>,

    /// Skip OCI image signature verification (insecure)
    #[arg(
        long = "insecure-skip-signature",
        help = "Skip OCI image signature verification",
        env = "ARK_INSECURE_SKIP_SIGNATURE",
        default_value = "false"
    )]
    insecure_skip_signature: bool,

    /// Use Sigstore TUF data for signature verification
    #[arg(
        long = "use-sigstore-tuf-data",
        help = "Use Sigstore TUF data for verification",
        env = "ARK_USE_SIGSTORE_TUF_DATA",
        default_value = "true"
    )]
    use_sigstore_tuf_data: bool,

    /// Disable the built-in management API server (CLI override)
    #[arg(
        long = "disable-api",
        value_name = "MANAGEMENT_API_DISABLE",
        env = "ARK_DISABLE_API",
        required = false
    )]
    disable_api: Option<bool>,
}

/// Main entry point for the Ark MCP server.
///
/// This function orchestrates the complete server initialization sequence:
/// argument parsing, configuration loading, state initialization, plugin loading,
/// and server startup. It follows a structured lifecycle with proper error handling
/// and logging throughout.
///
/// # Returns
/// - `Ok(())` if the server starts and runs successfully
/// - `Err(anyhow::Error)` if initialization or execution fails
///
/// # Panics
/// This function may panic if critical initialization steps fail (e.g., argument parsing).
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Parse command-line arguments
    let matches = Args::command().get_matches();
    let args = Args::from_arg_matches(&matches).expect("invalid args");

    // Initialize application state with default values
    let app_state = std::sync::Arc::new(ArkState::default());

    let fmt_layer = fmt::layer().with_target(false).compact();
    tracing_subscriber::registry()
        .with(fmt_layer)
        .with(tracing_subscriber::EnvFilter::from_default_env().add_directive("log=warn".parse()?))
        .init();

    // Transition to initializing state
    app_state.set_state(ApplicationState::Initializing);

    // Load configuration from file, environment, and CLI overrides
    let config = ArkConfig::load_with_overrides(
        args.config_file.clone(),
        args.transport,
        args.mcp_bind_address.clone(),
        args.insecure_skip_signature,
        args.use_sigstore_tuf_data,
        args.disable_api,
        args.management_bind_address,
    )?;

    // Apply configuration-derived settings to application state
    config.apply_to_state(app_state.clone()).await;

    // Startup-time validation: if token_signing is configured to use local keys,
    // ensure the key file is present and readable. Fail fast if misconfigured.
    if let Some(ts) = &config.token_signing
        && let Some(src) = &ts.source
        && src == "local"
    {
        // Check ENV override first
        let key_path = std::env::var("ARK_TOKEN_SIGNING_KEY")
            .ok()
            .or_else(|| ts.key.clone());
        if let Some(k) = key_path {
            match std::fs::read(&k) {
                Ok(_) => tracing::info!("Token signing configured with local key: {}", k),
                Err(e) => {
                    tracing::error!("Token signing key '{}' not readable: {}", k, e);
                    return Err(anyhow::anyhow!(
                        "Token signing misconfigured: key '{}' not readable",
                        k
                    ));
                }
            }
        } else {
            tracing::error!(
                "Token signing configured for 'local' but no key path provided (config.token_signing.key or ARK_TOKEN_SIGNING_KEY)"
            );
            return Err(anyhow::anyhow!(
                "Token signing misconfigured: missing key path"
            ));
        }
    }
    // Initialize database for persistent storage
    match crate::server::persist::Database::new() {
        Ok(database) => {
            app_state.set_database(database);
            tracing::info!("Database initialized successfully");
        }
        Err(e) => {
            tracing::warn!("Failed to initialize database: {:?}", e);
            tracing::warn!("Continuing without persistent storage");
        }
    }

    // Initialize metrics collection if enabled
    crate::metrics::init();

    // Transition to plugin loading phase
    app_state.set_state(ApplicationState::LoadingPlugins);

    // Load configured plugins and register their tools
    plugins::load_plugins(&config, app_state.clone()).await?;
    tracing::debug!("Plugin load completed");

    // Transition to network startup phase
    app_state.set_state(ApplicationState::StartingNetwork);

    // Initialize AWS-LC cryptographic provider for TLS
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .expect("Failed to install AWS-LC provider");

    // Start MCP and management servers
    // Start servers and map errors to friendly exit codes
    match start(&config, app_state).await {
        Ok(_) => {
            tracing::debug!("Server has exited");
            Ok(())
        }
        Err(e) => {
            tracing::error!("Server execution failed: {:?}", e);
            // Use string-based classification to avoid type import issues at the binary boundary.
            let msg = format!("{:?}", e);
            let code = if msg.contains("Failed to parse") || msg.contains("Configuration") {
                2
            } else if msg.contains("Token signing misconfigured")
                || msg.contains("Failed to initialize PEM signer")
            {
                3
            } else if msg.contains("KeyCertMismatch")
                || msg.contains("Certificate public key does not match")
            {
                4
            } else {
                1
            };

            // Flush logs then exit with code
            std::process::exit(code);
        }
    }
}
