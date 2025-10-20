//! OCI-based plugin loader.
//!
//! Pulls an OCI artifact/image, locates a WASM payload (raw layer or .wasm inside a tar
//! layer, optionally gzip/zstd compressed), verifies descriptor size and digest, and
//! initializes the plugin via `WasmHandler`.

use super::UriHandler;
use anyhow::{Context, bail};
use flate2::read::GzDecoder;
use sha2::Digest;
use std::{
    io::{self, Cursor, Read},
    pin::Pin,
    task::{Context as TaskContext, Poll},
    time::Instant,
};
use tar::Archive;
use tokio::io::AsyncWrite;
use zstd::stream::read::Decoder as ZstdDecoder;

use super::PluginLoadResult;
use crate::config::{models::OciAuthentication, plugins::ArkPlugin};
use crate::plugins::wasm::WasmHandler;
use oci_client::{
    Reference,
    client::{Client as OciClient, ClientConfig, ClientProtocol},
    manifest::{self, OciManifest},
    secrets::RegistryAuth,
};
use tracing::{debug, warn};

const LOCAL_LOG_PREFIX: &str = "[OCI-REPO]";

const RAW_WASM_MEDIA_TYPE: &str = "application/vnd.wasm.content.layer.v1+wasm";
const TAR_MEDIA_TYPES: &[&str] = &[
    "application/vnd.oci.image.layer.v1.tar",
    manifest::IMAGE_LAYER_MEDIA_TYPE,
];
const TAR_GZIP_MEDIA_TYPES: &[&str] = &[
    "application/vnd.oci.image.layer.v1.tar+gzip",
    "application/vnd.oci.image.layer.nondistributable.v1.tar+gzip",
    manifest::IMAGE_LAYER_GZIP_MEDIA_TYPE,
    manifest::IMAGE_DOCKER_LAYER_GZIP_MEDIA_TYPE,
];
const TAR_ZSTD_MEDIA_TYPES: &[&str] = &[
    "application/vnd.oci.image.layer.v1.tar+zstd",
    "application/vnd.oci.image.layer.nondistributable.v1.tar+zstd",
];

const MAX_BLOB_SIZE: u64 = 128 * 1024 * 1024; // 128 MiB, compressed as stored
const MAX_WASM_SIZE: u64 = 128 * 1024 * 1024; // 128 MiB, after extraction

/// A bounded `AsyncWrite` sink that caps the number of bytes accepted to prevent unbounded growth.
struct LimitedAsyncWriter {
    buf: Vec<u8>, // collected bytes
    max: u64,     // hard cap
}

impl LimitedAsyncWriter {
    /// Creates a new `LimitedAsyncWriter` with the specified maximum capacity.
    ///
    /// # Arguments
    /// * `max` - The maximum number of bytes to accept.
    /// * `cap_hint` - A hint for the initial capacity of the internal buffer.
    fn with_capacity(max: u64, cap_hint: usize) -> Self {
        Self {
            buf: Vec::with_capacity(cap_hint.min(max as usize)),
            max,
        }
    }

    /// Consumes the writer and returns the collected bytes.
    fn into_inner(self) -> Vec<u8> {
        self.buf
    }
}

/// Implements `AsyncWrite` with a hard cap on total bytes written.
/// Rejects writes that would exceed the maximum size.
impl AsyncWrite for LimitedAsyncWriter {
    fn poll_write(
        mut self: Pin<&mut Self>,
        _cx: &mut TaskContext<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        let written = self.buf.len() as u64;
        if written >= self.max {
            return Poll::Ready(Err(io::Error::other("blob exceeds maximum allowed size")));
        }
        let remaining = (self.max - written) as usize;

        // All-or-nothing: refuse this chunk if it would exceed the cap.
        if data.len() > remaining {
            return Poll::Ready(Err(io::Error::other("blob exceeds maximum allowed size")));
        }

        self.buf.extend_from_slice(data);
        Poll::Ready(Ok(data.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

// Resolve `RegistryAuth` from `PluginConfig`. Defaults to 'anonymous' if nothing specified, or if
// the authentication is misconfigured
///
/// Resolves `RegistryAuth` from the plugin configuration.
/// Defaults to anonymous access if no authentication is specified or if the configuration is invalid.
fn build_auth(config: &ArkPlugin) -> anyhow::Result<RegistryAuth> {
    match config.auth.clone().unwrap_or(OciAuthentication::Anonymous) {
        OciAuthentication::Anonymous => Ok(RegistryAuth::Anonymous),
        OciAuthentication::Bearer { token } => {
            if token.is_empty() {
                warn!(
                    repo = LOCAL_LOG_PREFIX,
                    "Bearer token is empty; proceeding with anonymous access"
                );
                Ok(RegistryAuth::Anonymous)
            } else {
                Ok(RegistryAuth::Bearer(token))
            }
        }
        OciAuthentication::Basic { username, password } => {
            if username.is_empty() || password.is_empty() {
                warn!(
                    repo = LOCAL_LOG_PREFIX,
                    "Basic auth with empty username or password; proceeding with anonymous access"
                );
                Ok(RegistryAuth::Anonymous)
            } else {
                Ok(RegistryAuth::Basic(username, password))
            }
        }
    }
}

/// Builds an `oci_client::ClientConfig` based on the plugin configuration.
/// Sets the protocol to HTTP if insecure is true, otherwise HTTPS.
fn prepare_connection(config: &ArkPlugin) -> ClientConfig {
    ClientConfig {
        protocol: if config.insecure {
            ClientProtocol::Http
        } else {
            ClientProtocol::Https
        },
        ..Default::default()
    }
}

/// Pulls an OCI artifact/image and returns the WASM layer bytes.
/// Verifies the manifest, selects the best layer containing WASM, downloads and verifies the blob.
pub async fn download_and_verify_image(config: &ArkPlugin) -> anyhow::Result<Vec<u8>> {
    if config.url.is_none() {
        bail!("Missing plugin path");
    }
    let url = config.url.clone().unwrap();
    let reference_text = strip_scheme(&url);
    let reference = Reference::try_from(reference_text.as_str())
        .with_context(|| format!("{LOCAL_LOG_PREFIX} Invalid reference: {}", reference_text))?;

    let auth = build_auth(config)?;
    if config.insecure && !matches!(config.auth, None | Some(OciAuthentication::Anonymous)) {
        warn!(
            repo = LOCAL_LOG_PREFIX,
            "Using insecure HTTP with credentials; this may leak secrets"
        );
    }
    let client = OciClient::new(prepare_connection(config));

    // Pull manifest and resolve layers
    let (manifest, _digest) = client
        .pull_manifest(&reference, &auth)
        .await
        .with_context(|| {
            format!(
                "{LOCAL_LOG_PREFIX} Failed to pull manifest for {}",
                reference_text
            )
        })?;
    let layers = layers_from_manifest(&manifest)?;

    if layers.is_empty() {
        bail!("{LOCAL_LOG_PREFIX} No layers in manifest");
    }

    // Try candidates in priority order until one yields a WASM module.
    let mut last_err: Option<anyhow::Error> = None;
    for idx in build_candidates(&layers) {
        let desc = &layers[idx];

        let blob = match fetch_blob_checked(&client, &reference, desc).await {
            Ok(b) => b,
            Err(e) => {
                last_err = Some(e);
                continue;
            }
        };

        match decode_wasm_from_layer(desc, &blob) {
            Ok(wasm) => {
                debug!(
                    repo = LOCAL_LOG_PREFIX,
                    "Selected layer {} with mediaType {}", idx, desc.media_type
                );
                return Ok(wasm);
            }
            Err(e) => {
                last_err = Some(e);
                continue;
            }
        }
    }

    if let Some(e) = last_err {
        Err(e)
    } else {
        bail!("{LOCAL_LOG_PREFIX} No supported layers found for WASM content")
    }
}

/// Maps a media type to a priority value for layer selection (lower is better).
/// Returns `None` for unsupported media types.
fn candidate_priority(mt: &str) -> Option<u8> {
    if mt == RAW_WASM_MEDIA_TYPE {
        Some(0)
    } else if TAR_GZIP_MEDIA_TYPES.contains(&mt) || TAR_ZSTD_MEDIA_TYPES.contains(&mt) {
        Some(1)
    } else if TAR_MEDIA_TYPES.contains(&mt) {
        Some(2)
    } else {
        None
    }
}

// Build a list of candidate layer indices ordered by (priority, original index).
/// Prioritizes raw WASM, then compressed tar, then uncompressed tar.
fn build_candidates(layers: &[manifest::OciDescriptor]) -> Vec<usize> {
    let mut tmp: Vec<(u8, usize)> = Vec::new();
    for (idx, d) in layers.iter().enumerate() {
        let mt = d.media_type.as_str();
        if let Some(p) = candidate_priority(mt) {
            tmp.push((p, idx));
        }
    }
    tmp.sort_by_key(|(p, idx)| (*p, *idx));
    tmp.into_iter().map(|(_, idx)| idx).collect()
}

/// Extracts layers from an OCI image manifest.
/// Bails if the manifest is not an image manifest.
fn layers_from_manifest(manifest: &OciManifest) -> anyhow::Result<Vec<manifest::OciDescriptor>> {
    match manifest {
        OciManifest::Image(m) => Ok(m.layers.clone()),
        _ => bail!("{LOCAL_LOG_PREFIX} Expected image manifest"),
    }
}

/// Downloads the blob for the given descriptor, verifies size and digest.
/// Uses the session/auth established during manifest pull for authentication.
async fn fetch_blob_checked(
    client: &OciClient,
    reference: &Reference,
    desc: &manifest::OciDescriptor,
) -> anyhow::Result<Vec<u8>> {
    if desc.size < 0 {
        bail!("{LOCAL_LOG_PREFIX} Negative layer size in descriptor");
    }
    let expected_size = desc.size as u64;
    if expected_size > MAX_BLOB_SIZE {
        bail!(
            "{LOCAL_LOG_PREFIX} Layer size {} exceeds maximum {} bytes",
            expected_size,
            MAX_BLOB_SIZE
        );
    }

    let mut sink = LimitedAsyncWriter::with_capacity(expected_size, expected_size as usize);

    // Stream blob into bounded sink; auth/session is managed by the client.
    client
        .pull_blob(reference, desc, &mut sink)
        .await
        .with_context(|| {
            format!(
                "{LOCAL_LOG_PREFIX} Failed to pull blob {} (mediaType {})",
                desc.digest, desc.media_type
            )
        })?;

    let buf = sink.into_inner();

    if buf.len() as u64 != expected_size {
        bail!(
            "{LOCAL_LOG_PREFIX} Layer size mismatch: expected {}, got {}",
            expected_size,
            buf.len()
        );
    }

    match parse_digest(&desc.digest) {
        Some(("sha256", expected_hex)) => {
            let computed_hex = hex::encode(sha2::Sha256::digest(&buf));
            if computed_hex != expected_hex {
                bail!(
                    "{LOCAL_LOG_PREFIX} Layer digest mismatch: expected sha256:{}, got sha256:{}",
                    expected_hex,
                    computed_hex
                );
            }
        }
        Some((algo, _)) => bail!("{LOCAL_LOG_PREFIX} Unsupported digest algorithm: {}", algo),
        None => bail!(
            "{LOCAL_LOG_PREFIX} Invalid layer digest format: {}",
            desc.digest
        ),
    }

    Ok(buf)
}

fn decode_wasm_from_layer(desc: &manifest::OciDescriptor, blob: &[u8]) -> anyhow::Result<Vec<u8>> {
    let mt = desc.media_type.as_str();
    if mt == RAW_WASM_MEDIA_TYPE {
        if blob.len() as u64 > MAX_WASM_SIZE {
            bail!(
                "{LOCAL_LOG_PREFIX} WASM layer exceeds maximum {} bytes",
                MAX_WASM_SIZE
            );
        }
        return Ok(blob.to_vec());
    }

    if TAR_GZIP_MEDIA_TYPES.contains(&mt) {
        return extract_wasm_from_tar_gz(blob)
            .with_context(|| format!("{LOCAL_LOG_PREFIX} No .wasm found in gzip tar layer"));
    }
    if TAR_ZSTD_MEDIA_TYPES.contains(&mt) {
        return extract_wasm_from_tar_zstd(blob)
            .with_context(|| format!("{LOCAL_LOG_PREFIX} No .wasm found in zstd tar layer"));
    }
    if TAR_MEDIA_TYPES.contains(&mt) {
        // Try standard tar extraction first
        match extract_wasm_from_tar(Cursor::new(blob)) {
            Ok(result) => return Ok(result),
            Err(_) => {
                // Fallback: check if blob is raw WASM (handles mislabeled layers)
                if blob.len() >= 8 && &blob[0..4] == b"\0asm" {
                    if blob.len() as u64 > MAX_WASM_SIZE {
                        bail!(
                            "{LOCAL_LOG_PREFIX} WASM layer exceeds maximum {} bytes",
                            MAX_WASM_SIZE
                        );
                    }
                    return Ok(blob.to_vec());
                }
                return Err(anyhow::anyhow!(
                    "{LOCAL_LOG_PREFIX} No .wasm found in tar layer"
                ));
            }
        }
    }

    bail!(
        "{LOCAL_LOG_PREFIX} Unsupported layer media type for WASM: {}",
        mt
    )
}

/// Parses a digest string in the format "<algo>:<hex>" into algorithm and hex parts.
/// Returns `None` for malformed input.
fn parse_digest(d: &str) -> Option<(&str, &str)> {
    d.split_once(':')
}

// Drop the URL scheme prefix used by higher-level config (e.g., "oci://").
///
/// Strips the URL scheme prefix (e.g., "oci://") from the URL for OCI client compatibility.
fn strip_scheme(url: &url::Url) -> String {
    let s = url.as_str();
    if let Some(i) = s.find(':') {
        s[i + 1..].trim_start_matches('/').to_string()
    } else {
        s.to_string()
    }
}

fn extract_wasm_from_tar<R: Read>(reader: R) -> anyhow::Result<Vec<u8>> {
    let mut archive = Archive::new(reader);
    let entries = archive.entries()?;
    for entry in entries {
        match entry {
            Ok(e) => {
                let path = e.path()?;
                let path_str = path.to_string_lossy();

                // Check for .wasm extension
                let is_wasm = path
                    .extension()
                    .and_then(|s| s.to_str())
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("wasm"));

                if is_wasm {
                    debug!(
                        repo = LOCAL_LOG_PREFIX,
                        "Found WASM file in tar: {}", path_str
                    );
                    let mut buf = Vec::new();
                    let mut limited = e.take(MAX_WASM_SIZE + 1);
                    let n = limited.read_to_end(&mut buf)?;
                    if n as u64 > MAX_WASM_SIZE {
                        bail!("{LOCAL_LOG_PREFIX} wasm file exceeds maximum allowed size");
                    }
                    return Ok(buf);
                } else {
                    debug!(
                        repo = LOCAL_LOG_PREFIX,
                        "Skipping non-WASM file in tar: {}", path_str
                    );
                }
            }
            Err(_) => continue,
        }
    }
    anyhow::bail!("{LOCAL_LOG_PREFIX} no .wasm found in tar layer")
}

/// Extracts WASM from a gzip-compressed tar archive.
fn extract_wasm_from_tar_gz(input: &[u8]) -> anyhow::Result<Vec<u8>> {
    let gz = GzDecoder::new(Cursor::new(input));
    extract_wasm_from_tar(gz)
}

/// Extracts WASM from a Zstd-compressed tar archive.
fn extract_wasm_from_tar_zstd(input: &[u8]) -> anyhow::Result<Vec<u8>> {
    let z = ZstdDecoder::new(Cursor::new(input))?;
    extract_wasm_from_tar(z)
}

/// Handler for loading WASM plugins from OCI references (e.g., `oci://` or `oci+https://` URLs).
/// Downloads the OCI image, extracts the WASM payload, and initializes the plugin.
pub struct OciHandler;

impl UriHandler for OciHandler {
    /// Fetches and initializes the plugin described by `plugin_config`.
    /// Measures load and execution time for diagnostics.    
    async fn get(&self, plugin_config: &ArkPlugin) -> anyhow::Result<PluginLoadResult> {
        if plugin_config.url.is_none() {
            bail!("Missing plugin path");
        }
        let url = plugin_config.url.clone().unwrap();
        let start = Instant::now(); // Measure load + init time for diagnostics
        let wasm_bytes = download_and_verify_image(plugin_config).await?;

        // Initialize WASM plugin
        let wasm = WasmHandler::new(wasm_bytes.clone(), &plugin_config.manifest)?; // Validates module and required exports

        // Execute plugin
        let exec_start = Instant::now();
        let mut result = wasm.get(plugin_config).await?;
        debug!(
            repo = LOCAL_LOG_PREFIX,
            "Plugin [{}] execution completed in {:.2?} (total {:.2?})",
            super::sanitized_url(&url),
            exec_start.elapsed(),
            start.elapsed()
        );

        // Attach raw bytes and source url so callers can persist the payload
        result.raw_bytes = Some(wasm_bytes);
        result.source_url = Some(url.to_string());
        Ok(result)
    }
}
