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

use crate::{
    server::service::start,
    state::{ApplicationState, ArkState},
};
use clap::{CommandFactory, FromArgMatches, Parser};
use config::{ArkConfig, components::McpTransport};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{Layer, fmt};

/// Layer that filters out specific error messages
struct FilteringLayer<L> {
    inner: L,
}

impl<L, S> Layer<S> for FilteringLayer<L>
where
    L: Layer<S>,
    S: tracing::Subscriber,
{
    fn on_event(&self, event: &tracing::Event<'_>, ctx: tracing_subscriber::layer::Context<'_, S>) {
        let mut visitor = MessageVisitor::new();
        event.record(&mut visitor);
        if event.metadata().level() == &tracing::Level::ERROR
            && visitor.message.contains("Error reading from stream")
        {
            return; // suppress this specific error
        }
        self.inner.on_event(event, ctx);
    }

    fn enabled(
        &self,
        metadata: &tracing::Metadata<'_>,
        ctx: tracing_subscriber::layer::Context<'_, S>,
    ) -> bool {
        self.inner.enabled(metadata, ctx)
    }

    fn on_new_span(
        &self,
        attrs: &tracing::span::Attributes<'_>,
        id: &tracing::Id,
        ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        self.inner.on_new_span(attrs, id, ctx);
    }

    fn on_record(
        &self,
        span: &tracing::Id,
        values: &tracing::span::Record<'_>,
        ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        self.inner.on_record(span, values, ctx);
    }

    fn on_enter(&self, id: &tracing::Id, ctx: tracing_subscriber::layer::Context<'_, S>) {
        self.inner.on_enter(id, ctx);
    }

    fn on_exit(&self, id: &tracing::Id, ctx: tracing_subscriber::layer::Context<'_, S>) {
        self.inner.on_exit(id, ctx);
    }

    fn on_close(&self, id: tracing::Id, ctx: tracing_subscriber::layer::Context<'_, S>) {
        self.inner.on_close(id, ctx);
    }
}

struct MessageVisitor {
    message: String,
}

impl MessageVisitor {
    fn new() -> Self {
        Self {
            message: String::new(),
        }
    }
}

impl tracing::field::Visit for MessageVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.message = format!("{:?}", value);
        }
    }
}

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

    // Initialize logging with filtering
    let env_filter = if let Ok(v) = std::env::var("RUST_LOG") {
        format!("{},log=warn", v)
    } else {
        "info,log=warn".to_string()
    };
    let fmt_layer = fmt::layer().with_target(false).compact();
    let filtering_layer = FilteringLayer { inner: fmt_layer };
    tracing_subscriber::registry()
        .with(filtering_layer)
        .with(tracing_subscriber::filter::EnvFilter::new(env_filter))
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
    tracing::debug!("Early init completed");

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
    match start(&config, app_state).await {
        Ok(_) => tracing::debug!("Server has exited"),
        Err(e) => tracing::error!("Server execution failed: {:?}", e),
    }

    Ok(())
}
