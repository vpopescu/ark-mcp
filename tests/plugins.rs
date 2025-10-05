use std::sync::Arc;

use ark::config::ArkConfig;
use ark::config::plugins::{MemoryLimits, PluginManifest};
use ark::plugins;
use ark::plugins::builtin::BUILTIN_PLUGIN_ID;
use ark::state::{ApplicationState, ArkState};
use extism::Manifest;
use std::collections::BTreeMap;
use std::path::PathBuf;

fn testdata_sample_path() -> std::path::PathBuf {
    let mut p = std::env::current_dir().unwrap();
    p.push("tests");
    p.push("testdata");
    p.push("sample.wasm");
    p
}

fn base_manifest() -> Manifest {
    let mut manifest = Manifest::new([PathBuf::from("base.wasm")]);
    manifest.memory.max_pages = Some(1);
    manifest.config = BTreeMap::from([("A".to_string(), "1".to_string())]);
    manifest.allowed_hosts = Some(vec!["base.com".to_string()]);
    manifest.allowed_paths = Some(BTreeMap::from([("/base".to_string(), "/base/path".into())]));
    manifest
}

fn overlay_manifest() -> PluginManifest {
    PluginManifest {
        wasm: Some(vec!["overlay.wasm".to_string()]),
        memory: Some(MemoryLimits { max_pages: Some(2) }),
        config: Some(BTreeMap::from([("B".to_string(), "2".to_string())])),
        allowed_hosts: Some(vec!["overlay.com".to_string()]),
        allowed_paths: Some(BTreeMap::from([(
            "/overlay".to_string(),
            PathBuf::from("/overlay/path"),
        )])),
    }
}

#[tokio::test]
/// Tests that builtin plugin is loaded when no external plugins are configured
async fn load_no_plugins_registers_builtin() {
    // Test that when no plugins are configured, the builtin plugin is loaded
    let cfg = ArkConfig {
        plugins: vec![],
        ..Default::default()
    };
    let app = Arc::new(ArkState::default());
    app.set_state(ApplicationState::StartingNetwork);
    plugins::load_plugins(&cfg, app.clone())
        .await
        .expect("plugin load");
    let defs = app.plugin_registry.catalog.read().await;
    assert!(
        defs.plugin_to_config.contains_key(BUILTIN_PLUGIN_ID),
        "expected builtin plugin '{}' to be registered when no plugins are loaded",
        BUILTIN_PLUGIN_ID
    );
}

#[cfg(target_os = "windows")]
#[tokio::test]
/// Tests loading a WASM plugin from a Windows file path and verifies builtin plugin is not loaded
async fn load_wasm_plugin_from_windows_file_path() {
    // Build config JSON using a Windows-style absolute file path
    use ark::config::plugins::file_path_to_url;

    let sample = testdata_sample_path();
    assert!(
        sample.exists(),
        "sample.wasm not found at {}",
        sample.display()
    );
    let abs = std::fs::canonicalize(&sample).unwrap();
    let path_str = abs.to_string_lossy().to_string();
    let file_url = file_path_to_url(&path_str[..]).expect("file url");
    let cfg_val = serde_json::json!({
        "plugins": [
            {
                "name": "sample",
                "url": file_url.as_str()
            }
        ]
    });
    let cfg: ArkConfig = serde_json::from_value(cfg_val).expect("config parse");
    let app = Arc::new(ArkState::default());
    app.set_state(ApplicationState::StartingNetwork);
    plugins::load_plugins(&cfg, app.clone())
        .await
        .expect("plugin load");
    let defs = app.plugin_registry.catalog.read().await;
    assert!(
        defs.plugin_to_config.contains_key("sample"),
        "expected plugin 'sample' to be registered"
    );
    // When plugins are loaded, builtin should not be present
    assert!(
        !defs.plugin_to_config.contains_key(BUILTIN_PLUGIN_ID),
        "builtin plugin '{}' should not be present when external plugins are loaded",
        BUILTIN_PLUGIN_ID
    );
}

#[cfg(not(target_os = "windows"))]
#[tokio::test]
/// Tests loading a WASM plugin from a Linux file path and verifies builtin plugin is not loaded
async fn load_wasm_plugin_from_linux_file_path() {
    // Build config JSON using a Linux-style absolute file path

    use ark::config::plugins::file_path_to_url;
    let sample = testdata_sample_path();
    assert!(
        sample.exists(),
        "sample.wasm not found at {}",
        sample.display()
    );
    let abs = std::fs::canonicalize(&sample).unwrap();
    let path_str = abs.to_string_lossy().to_string();
    let file_url = file_path_to_url(&path_str[..]).expect("file url");
    let cfg_val = serde_json::json!({
        "plugins": [
            {
                "name": "sample",
                "url": file_url.as_str()
            }
        ]
    });
    let cfg: ArkConfig = serde_json::from_value(cfg_val).expect("config parse");
    let app = Arc::new(ArkState::default());
    app.set_state(ApplicationState::StartingNetwork);
    plugins::load_plugins(&cfg, app.clone())
        .await
        .expect("plugin load");
    let defs = app.plugin_registry.catalog.read().await;
    assert!(
        defs.plugin_to_config.contains_key("sample"),
        "expected plugin 'sample' to be registered"
    );
    // When plugins are loaded, builtin should not be present
    assert!(
        !defs.plugin_to_config.contains_key(BUILTIN_PLUGIN_ID),
        "builtin plugin '{}' should not be present when external plugins are loaded",
        BUILTIN_PLUGIN_ID
    );
}

#[tokio::test]
/// Tests loading a WASM plugin from an HTTP URL with a local test server and verifies builtin plugin is not loaded
async fn load_wasm_plugin_from_http_url() {
    // Start a tiny HTTP file server that serves the sample.wasm
    use http_body_util::Full;
    use hyper::body::Bytes;
    use hyper::server::conn::http1;
    use hyper::service::service_fn;
    use hyper::{Request, Response, StatusCode};
    use std::convert::Infallible;
    use tokio::net::TcpListener;

    let sample = std::fs::read(testdata_sample_path()).expect("read sample.wasm");

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let serve = tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let io = hyper_util::rt::TokioIo::new(stream);
            let bytes = sample.clone();
            tokio::spawn(async move {
                let svc = service_fn(move |req: Request<hyper::body::Incoming>| {
                    let bytes = bytes.clone();
                    async move {
                        let path = req.uri().path();
                        if path.ends_with("/sample.wasm") || path == "/" {
                            let mut resp = Response::new(Full::new(Bytes::from(bytes)));
                            *resp.status_mut() = StatusCode::OK;
                            Ok::<_, Infallible>(resp)
                        } else {
                            let mut resp =
                                Response::new(Full::new(Bytes::from_static(b"not found")));
                            *resp.status_mut() = StatusCode::NOT_FOUND;
                            Ok::<_, Infallible>(resp)
                        }
                    }
                });
                let _ = http1::Builder::new().serve_connection(io, svc).await;
            });
        }
    });

    // Build config pointing to our HTTP server; allow plain HTTP (insecure)
    let url = format!("http://{}/sample.wasm", addr);
    let cfg_text = format!(
        r#"{{
            "plugins": [
                {{
                    "name": "sample-http",
                    "url": "{}",
                    "insecure": true
                }}
            ]
        }}"#,
        url
    );
    let cfg: ArkConfig = serde_json::from_str(&cfg_text).expect("config parse");
    let app = Arc::new(ArkState::default());
    app.set_state(ApplicationState::StartingNetwork);
    plugins::load_plugins(&cfg, app.clone())
        .await
        .expect("plugin load");
    let defs = app.plugin_registry.catalog.read().await;
    assert!(
        defs.plugin_to_config.contains_key("sample-http"),
        "expected plugin 'sample-http' to be registered"
    );
    // When plugins are loaded, builtin should not be present
    assert!(
        !defs.plugin_to_config.contains_key(BUILTIN_PLUGIN_ID),
        "builtin plugin '{}' should not be present when external plugins are loaded",
        BUILTIN_PLUGIN_ID
    );

    // Stop server task
    serve.abort();
}

#[tokio::test]
/// Tests loading a WASM plugin from an but it ase registry and verifies builtin plugin is not loaded
async fn load_wasm_plugin_from_oci_registry() {
    // Pull a real plugin from GHCR (network dependent)
    let cfg_text = r#"{
        "plugins": [
            {
                "name": "time",
                "url": "oci://ghcr.io/vpopescu/ark-mcp-plugin-time:v0.0.1"
            }
        ]
    }"#;

    let cfg: ArkConfig = serde_json::from_str(cfg_text).expect("config parse");
    let app = Arc::new(ArkState::default());
    app.set_state(ApplicationState::StartingNetwork);
    plugins::load_plugins(&cfg, app.clone())
        .await
        .expect("plugin load");
    let defs = app.plugin_registry.catalog.read().await;
    assert!(
        defs.plugin_to_config.contains_key("time"),
        "expected plugin 'time' to be registered"
    );
    // When plugins are loaded, builtin should not be present
    assert!(
        !defs.plugin_to_config.contains_key(BUILTIN_PLUGIN_ID),
        "builtin plugin '{}' should not be present when external plugins are loaded",
        BUILTIN_PLUGIN_ID
    );
}

fn apply_manifest_overlay(manifest: &mut Manifest, overlay: &PluginManifest) {
    if let Some(max_pages) = overlay.memory.as_ref().and_then(|memory| memory.max_pages) {
        manifest.memory.max_pages = Some(max_pages);
    }
    if let Some(config) = &overlay.config {
        manifest.config = config.clone();
    }
    if let Some(hosts) = &overlay.allowed_hosts {
        manifest.allowed_hosts = Some(hosts.clone());
    }
    if let Some(paths) = &overlay.allowed_paths {
        manifest.allowed_paths = Some(paths.clone());
    }
}

/// Test that plugin manifest overlay completely replaces base manifest fields when all overlay fields are present.
#[test]
fn test_full_overlay_replaces_all_fields() {
    let mut manifest = base_manifest();
    let overlay = overlay_manifest();
    apply_manifest_overlay(&mut manifest, &overlay);

    // Check that overlay values replaced base values
    assert_eq!(manifest.memory.max_pages, Some(2));
    assert_eq!(manifest.config.get("B"), Some(&"2".to_string()));
    assert_eq!(
        manifest.allowed_hosts,
        Some(vec!["overlay.com".to_string()])
    );
    assert_eq!(
        manifest.allowed_paths.as_ref().unwrap().get("/overlay"),
        Some(&PathBuf::from("/overlay/path"))
    );
    // Check that base values are gone
    assert!(!manifest.config.contains_key("A"));
    assert!(
        manifest
            .allowed_paths
            .as_ref()
            .unwrap()
            .get("/base")
            .is_none()
    );
}

/// Test that plugin manifest overlay with all None values preserves the base manifest unchanged.
#[test]
fn test_empty_overlay_preserves_base() {
    let mut manifest = base_manifest();
    let overlay = PluginManifest {
        wasm: None,
        memory: None,
        config: None,
        allowed_hosts: None,
        allowed_paths: None,
    };
    apply_manifest_overlay(&mut manifest, &overlay);

    // All base values should remain
    assert_eq!(manifest.memory.max_pages, Some(1));
    assert_eq!(manifest.config.get("A"), Some(&"1".to_string()));
    assert_eq!(manifest.allowed_hosts, Some(vec!["base.com".to_string()]));
    assert_eq!(
        manifest.allowed_paths.as_ref().unwrap().get("/base"),
        Some(&PathBuf::from("/base/path"))
    );
}

/// Test that plugin manifest overlay partially replaces base manifest, only affecting specified fields.
#[test]
fn test_partial_overlay_replaces_only_specified_fields() {
    let mut manifest = base_manifest();
    let overlay = PluginManifest {
        wasm: None,
        memory: None,
        config: Some(BTreeMap::from([("D".to_string(), "4".to_string())])),
        allowed_hosts: None,
        allowed_paths: None,
    };
    apply_manifest_overlay(&mut manifest, &overlay);

    // Only config should be replaced
    assert_eq!(manifest.memory.max_pages, Some(1));
    assert_eq!(manifest.config.get("D"), Some(&"4".to_string()));
    assert!(!manifest.config.contains_key("A"));
    assert_eq!(manifest.allowed_hosts, Some(vec!["base.com".to_string()]));
    assert_eq!(
        manifest.allowed_paths.as_ref().unwrap().get("/base"),
        Some(&PathBuf::from("/base/path"))
    );
}
