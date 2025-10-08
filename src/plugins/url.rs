//! URL-based plugin loader (file://, http://, https://).
//!
//! Loads a WASM plugin from a local file or over HTTP(S), initializes it via
//! `WasmHandler`, and returns the plugin-described `ToolSet`.
//!
//! Local files must be provided as file:/// URLs.

use super::sanitized_url;
use super::wasm::WasmHandler;
use super::{PluginLoadResult, UriHandler};
use crate::config::plugins::ArkPlugin;
use crate::server;
use anyhow::{Context, anyhow, bail};
use reqwest::Client;
use std::{
    sync::OnceLock,
    time::{Duration, Instant},
};
use tokio::fs;
use tracing::debug;

/// Handles loading WASM plugins from file://, http://, and https:// URLs.
pub struct UrlHandler;
const LOCAL_LOG_PREFIX: &str = "[URL-REPO]";

impl UriHandler for UrlHandler {
    /// Fetches and initializes a WASM plugin from the URL in `plugin_config`.
    /// Supports file://, http://, and https:// schemes with appropriate security checks.    
    async fn get(&self, plugin_config: &ArkPlugin) -> anyhow::Result<PluginLoadResult> {
        if plugin_config.url.is_none() {
            bail!("Missing plugin path");
        }
        let url = plugin_config.url.clone().unwrap();
        let start = Instant::now(); // Measure load time for diagnostics

        let wasm = match url.scheme() {
            "file" => {
                let path = url
                    .to_file_path()
                    .map_err(|_| anyhow!("{LOCAL_LOG_PREFIX} Unsupported file URL '{}'", url))?;

                let bytes = fs::read(&path).await.with_context(|| {
                    format!(
                        "{LOCAL_LOG_PREFIX} Failed to read file '{}'",
                        path.display()
                    )
                })?;

                let wasm = WasmHandler::new(bytes, &plugin_config.manifest)?;

                debug!(
                    repo = LOCAL_LOG_PREFIX,
                    "Plugin [{}] loaded from file in {:.2?}",
                    path.display(),
                    start.elapsed()
                );
                wasm
            }
            "http" | "https" => {
                let safe = sanitized_url(&url);
                if url.scheme() == "http" && !plugin_config.insecure {
                    bail!(
                        "{LOCAL_LOG_PREFIX} Plain HTTP is disabled for '{}'; set plugin.insecure=true to allow",
                        safe
                    );
                }
                debug!(
                    repo = LOCAL_LOG_PREFIX,
                    "Retrieving plugin from URL: {}", safe
                );

                let resp = http_client()
                    .get(url.as_str())
                    .send()
                    .await
                    .with_context(|| format!("{LOCAL_LOG_PREFIX} Failed to fetch '{}'", safe))?
                    .error_for_status()
                    .with_context(|| format!("{LOCAL_LOG_PREFIX} HTTP error for '{}'", safe))?;

                let bytes = resp.bytes().await.with_context(|| {
                    format!(
                        "{LOCAL_LOG_PREFIX} Failed to read response body for '{}'",
                        safe
                    )
                })?;

                let wasm = WasmHandler::new(bytes.to_vec(), &plugin_config.manifest)?;

                debug!(
                    repo = LOCAL_LOG_PREFIX,
                    "Plugin [{}] loaded in {:.2?}",
                    safe,
                    start.elapsed()
                );
                wasm
            }
            other => {
                bail!(
                    "{LOCAL_LOG_PREFIX} Unsupported scheme '{}' for '{}'",
                    other,
                    sanitized_url(&url)
                );
            }
        };

        // Execute plugin
        let exec_start = Instant::now();
        let result = wasm.describe(plugin_config).await?;
        debug!(
            repo = LOCAL_LOG_PREFIX,
            "Plugin [{}] execution completed in {:.2?}",
            sanitized_url(&url),
            exec_start.elapsed()
        );

        // Build executors for all tools using this wasm module
        let mut execs = Vec::new();
        for t in &result.tools {
            let exec = wasm.build_executor(t.name.as_ref());
            execs.push((t.name.to_string(), exec));
        }
        Ok(PluginLoadResult {
            toolset: result,
            executors: execs,
        })
    }
}

/// Returns a reused HTTP client with a 30-second timeout and user agent.
/// The client is lazily initialized and reused across requests.
fn http_client() -> &'static Client {
    static CLIENT: OnceLock<Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent(server::constants::REQUEST_USER_AGENT)
            .build()
            .expect("reqwest client")
    })
}
