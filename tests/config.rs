use ark::config::{ArkConfig, McpTransport};
use ark::config::{
    components::ManagementEndpointConfig, components::ManagementPathConfig,
    components::McpEndpointConfig,
};
use ark::server::service;
use ark::state::{ApplicationState, ArkState};
use std::fs;
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::Arc;

fn write_temp_config(contents: &str, ext: &str) -> tempfile::NamedTempFile {
    let f = tempfile::Builder::new()
        .suffix(&format!(".{ext}"))
        .tempfile()
        .unwrap();
    fs::write(f.path(), contents).unwrap();
    f
}

fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

/// Test that loading configuration with no file present uses explicit defaults
/// instead of relying on struct Default for nested blocks.
#[test]
fn load_empty_uses_explicit_defaults() {
    // No file on disk: ArkConfig::load_with_overrides should construct explicit defaults
    // and not rely on struct Default for nested blocks.
    let cfg = ArkConfig::load_with_overrides(
        Some(PathBuf::from("__does_not_exist__")),
        //None,
        McpTransport::StreamableHTTP,
        Some("127.0.0.1:3001".to_string()),
        false,
        true,
        None,
        None,
    )
    .unwrap();

    assert_eq!(cfg.transport, Some(McpTransport::StreamableHTTP));
    assert_eq!(
        cfg.mcp_server.clone().unwrap().bind_address.unwrap(),
        "127.0.0.1:3001"
    );

    // management defaults present
    let mgmt = cfg.management_server.clone().unwrap();
    assert_eq!(mgmt.livez.path.as_deref(), Some("/livez"));
    assert_eq!(mgmt.readyz.path.as_deref(), Some("/readyz"));
    assert_eq!(mgmt.response_type.to_lowercase(), "json");

    // mcp defaults present
    let mcp = cfg.mcp_server.clone().unwrap();
    assert_eq!(mcp.cors, None);
}

/// Test loading YAML config without MCP block, verifying defaults are applied
/// and CLI overrides work for transport and bind address.
#[test]
fn load_yaml_without_mcp_block_applies_defaults() {
    let tf = write_temp_config(
        r#"
        management_server:
          livez:
            enabled: true
            path: "/livez"
          readyz:
            enabled: true
            path: "/readyz"
          bind_address: "127.0.0.1:4000"
        "#,
        "yaml",
    );

    let cfg = ArkConfig::load_with_overrides(
        Some(tf.path().to_path_buf()),
        //None,
        McpTransport::StreamableHTTP,
        Some("127.0.0.1:3001".to_string()),
        false,
        true,
        None,
        None,
    )
    .unwrap();

    assert_eq!(cfg.transport, Some(McpTransport::StreamableHTTP));
    assert_eq!(
        cfg.mcp_server.clone().unwrap().bind_address.unwrap(),
        "127.0.0.1:3001"
    ); // CLI override applied

    // mcp_server should be Some with explicit defaults even if absent in file
    let mcp = cfg.mcp_server.clone().unwrap();
    assert_eq!(mcp.cors, None);

    // mgmt present and parsed
    let mgmt = cfg.management_server.clone().unwrap();
    assert_eq!(mgmt.livez.path.as_deref(), Some("/livez"));
    assert_eq!(mgmt.readyz.path.as_deref(), Some("/readyz"));
    assert_eq!(mgmt.bind_address, Some("127.0.0.1:4000".to_string()));
}

/// Test loading YAML config with specific CORS settings and custom management paths,
/// ensuring CLI overrides and parsed values are correctly applied.
#[test]
fn load_yaml_with_specific_cors() {
    let tf = write_temp_config(
        r#"
        mcp_server:
          cors: "http://localhost:8000"

        management_server:
          livez:
            enabled: true
            path: "/healthz"
          readyz:
            enabled: false
            path: "/ready"
        "#,
        "yaml",
    );

    let cfg = ArkConfig::load_with_overrides(
        Some(tf.path().to_path_buf()),
        McpTransport::StreamableHTTP,
        Some("127.0.0.1:3666".to_string()),
        false,
        true,
        Some(false),
        None,
    )
    .unwrap();

    // CLI overrides
    assert_eq!(cfg.transport, Some(McpTransport::StreamableHTTP));
    assert_eq!(
        cfg.mcp_server.clone().unwrap().bind_address.unwrap(),
        "127.0.0.1:3666"
    );

    // mcp settings parsed
    let mcp = cfg.mcp_server.clone().unwrap();
    assert_eq!(mcp.cors, Some("http://localhost:8000".to_string()));

    // mgmt settings parsed and response flags applied
    let mgmt = cfg.management_server.clone().unwrap();
    assert_eq!(mgmt.livez.path.as_deref(), Some("/healthz"));
    assert_eq!(mgmt.readyz.path.as_deref(), Some("/ready"));
}

/// Test loading YAML config with multilevel health endpoint paths,
/// verifying custom paths are parsed and applied correctly.
#[test]
fn load_yaml_with_multilevel_health_paths() {
    let tf = write_temp_config(
        r#"
        management_server:
          livez:
            enabled: true
            path: "/api/livez"
          readyz:
            enabled: true
            path: "/api/readyz"
        "#,
        "yaml",
    );

    let cfg = ArkConfig::load_with_overrides(
        Some(tf.path().to_path_buf()),
        McpTransport::StreamableHTTP,
        Some("127.0.0.1:3001".to_string()),
        false,
        true,
        None,
        None,
    )
    .unwrap();

    let mgmt = cfg.management_server.clone().unwrap();
    assert!(mgmt.livez.enabled);
    assert!(mgmt.readyz.enabled);
    assert_eq!(mgmt.livez.path.as_deref(), Some("/api/livez"));
    assert_eq!(mgmt.readyz.path.as_deref(), Some("/api/readyz"));
}

/// Test loading YAML config with plugins and TLS configuration,
/// ensuring plugins are parsed with manifests and TLS settings are applied.
#[test]
fn load_yaml_with_plugins_and_tls() {
    let tf = write_temp_config(
        r#"
        management_server:
          livez:
            enabled: true
            path: "/livez"
          readyz:
            enabled: true
            path: "/readyz"

        mcp_server:
          cors: "https://localhost:8000"
          bind_address: "127.0.0.1:3001"

        plugins:
        - name: time
          url: "file:///path/to/time_plugin.wasm"
          manifest:
            config:
              timeout_ms: "2000"
            memory:
              max_pages: 32
        - name: hash
          url: "https://example.com/hash_plugin.wasm"
          insecure: true

        tls:
          key: "assets/dev_server.key"
          cert: "assets/dev_server.pem"
        "#,
        "yaml",
    );

    let cfg = ArkConfig::load_with_overrides(
        Some(tf.path().to_path_buf()),
        McpTransport::StreamableHTTP,
        Some("127.0.0.1:3002".to_string()),
        false,
        true,
        None,
        None,
    )
    .unwrap();

    // Check transport override
    assert_eq!(cfg.transport, Some(McpTransport::StreamableHTTP));

    // Check MCP server config
    let mcp = cfg.mcp_server.clone().unwrap();
    assert_eq!(mcp.cors, Some("https://localhost:8000".to_string()));
    assert_eq!(mcp.bind_address, Some("127.0.0.1:3002".to_string())); // CLI override

    // Check management server defaults
    let mgmt = cfg.management_server.clone().unwrap();
    assert_eq!(mgmt.livez.path.as_deref(), Some("/livez"));
    assert_eq!(mgmt.readyz.path.as_deref(), Some("/readyz"));

    // Check plugins
    assert_eq!(cfg.plugins.len(), 2);

    let time_plugin = &cfg.plugins[0];
    assert_eq!(time_plugin.name, "time");
    assert_eq!(
        time_plugin.url.as_ref().unwrap().as_str(),
        "file:///path/to/time_plugin.wasm"
    );
    assert!(!time_plugin.insecure);
    assert!(time_plugin.manifest.is_some());
    let manifest = time_plugin.manifest.as_ref().unwrap();
    assert!(manifest.memory.is_some());
    assert_eq!(manifest.memory.as_ref().unwrap().max_pages, Some(32));
    assert!(manifest.config.is_some());
    assert_eq!(
        manifest.config.as_ref().unwrap().get("timeout_ms"),
        Some(&"2000".to_string())
    );

    let hash_plugin = &cfg.plugins[1];
    assert_eq!(hash_plugin.name, "hash");
    assert_eq!(
        hash_plugin.url.as_ref().unwrap().as_str(),
        "https://example.com/hash_plugin.wasm"
    );
    assert!(hash_plugin.insecure);
    assert!(hash_plugin.manifest.is_none());

    // Check TLS config
    let tls = cfg.tls.unwrap();
    assert_eq!(tls.key, Some("assets/dev_server.key".to_string()));
    assert_eq!(tls.cert, Some("assets/dev_server.pem".to_string()));
    assert!(!tls.silent_insecure);
}

/// Test that when plugin API is disabled and console is disabled, management server serves only health endpoints,
/// and MCP server serves only MCP and SSE endpoints, with proper CORS handling.
#[tokio::test]
async fn mgmt_api_disabled_console_and_health_enabled_mcp_only_serves_mcp_and_health() {
    let port = free_port();
    let bind = format!("127.0.0.1:{port}");
    // Separate management bind
    let mgmt_port = free_port();
    let mgmt_bind = format!("127.0.0.1:{mgmt_port}");

    // Build config with disable_api = true
    let cfg = ArkConfig {
        transport: Some(McpTransport::StreamableHTTP),
        insecure_skip_signature: false,
        use_sigstore_tuf_data: true,
        cert_issuer: None,
        cert_email: None,
        cert_url: None,
        tls: None,
        management_server: Some(ManagementEndpointConfig {
            livez: ManagementPathConfig {
                path: Some("/livez".into()),
                enabled: true,
            },
            readyz: ManagementPathConfig {
                path: Some("/readyz".into()),
                enabled: true,
            },
            bind_address: Some(mgmt_bind.clone()),
            response_type: "json".into(),
            disable_plugin_api: true,
            disable_console: true,
            disable_health_api: false,
            cors: Some("http://localhost:3000".to_string()),
            disable_prometheus_api: false,
            disable_emit_otel: true,
        }),
        mcp_server: Some(McpEndpointConfig {
            cors: Some("http://localhost:3000".to_string()),
            bind_address: Some(bind.clone()),
        }),
        plugins: vec![],
    };

    let state = Arc::new(ArkState::default());
    state.set_state(ApplicationState::Ready);
    cfg.apply_to_state(state.clone()).await;
    if let Some(t) = cfg.transport {
        state.set_transport(t);
    }

    // Run server in background and abort after assertions
    let srv_state = state.clone();
    let cfg_clone = cfg.clone();
    let handle = tokio::spawn(async move {
        let _ = service::start(&cfg_clone, srv_state).await;
    });

    // Give it a moment to bind
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let base = format!("http://{bind}");
    let mgmt_base = format!("http://{mgmt_bind}");
    let client = reqwest::Client::new();

    // Management: /admin should be enabled
    let admin_resp = client
        .get(format!("{}/admin", mgmt_base))
        .send()
        .await
        .unwrap();
    assert!(admin_resp.status().is_client_error());

    // Management: health endpoints enabled
    assert_eq!(
        client
            .get(format!("{}/livez", mgmt_base))
            .send()
            .await
            .unwrap()
            .status()
            .as_u16(),
        200
    );
    assert_eq!(
        client
            .get(format!("{}/readyz", mgmt_base))
            .send()
            .await
            .unwrap()
            .status()
            .as_u16(),
        200
    );

    // Management: API disabled
    assert!(
        client
            .get(format!("{}/api/plugins", mgmt_base))
            .send()
            .await
            .unwrap()
            .status()
            .is_client_error()
    );

    // MCP listener should NOT serve /api or health or /admin
    assert!(
        client
            .get(format!("{}/api/plugins", base))
            .send()
            .await
            .unwrap()
            .status()
            .is_client_error()
    );

    assert!(
        client
            .get(format!("{}/admin", base))
            .send()
            .await
            .unwrap()
            .status()
            .is_client_error()
    );

    // MCP preflight should be handled (200)
    let opt_resp = client
        .request(reqwest::Method::OPTIONS, format!("{}/mcp", base))
        .header(reqwest::header::ORIGIN, "http://localhost:8000")
        .header("Access-Control-Request-Method", "POST")
        .send()
        .await
        .unwrap();
    assert_eq!(opt_resp.status().as_u16(), 200);

    handle.abort();
}

/// Test that when console is disabled but API and health are enabled, management server serves API and health,
/// and MCP server serves only MCP endpoints, with /admin properly disabled.
#[tokio::test]
async fn console_disabled_api_and_health_enabled_mcp_only_serves_mcp() {
    let main_port = free_port();
    let mgmt_port = free_port();
    let main_bind = format!("127.0.0.1:{main_port}");
    let mgmt_bind = format!("127.0.0.1:{mgmt_port}");

    let cfg = ArkConfig {
        transport: Some(McpTransport::StreamableHTTP),
        insecure_skip_signature: false,
        use_sigstore_tuf_data: true,
        cert_issuer: None,
        cert_email: None,
        cert_url: None,
        tls: None,
        management_server: Some(ManagementEndpointConfig {
            livez: ManagementPathConfig {
                path: Some("/livez".into()),
                enabled: false,
            },
            readyz: ManagementPathConfig {
                path: Some("/readyz".into()),
                enabled: false,
            },
            bind_address: Some(mgmt_bind.clone()),
            response_type: "json".into(),
            disable_plugin_api: false,
            disable_console: true,
            disable_health_api: false,
            cors: Some("http://localhost:3000".to_string()),
            disable_prometheus_api: false,
            disable_emit_otel: true,
        }),
        mcp_server: Some(McpEndpointConfig {
            cors: Some("http://localhost:3000".to_string()),
            bind_address: Some(main_bind.clone()),
        }),
        plugins: vec![],
    };

    let state = Arc::new(ArkState::default());
    state.set_state(ApplicationState::StartingNetwork);
    cfg.apply_to_state(state.clone()).await;
    if let Some(t) = cfg.transport {
        state.set_transport(t);
    }

    let srv_state = state.clone();
    let cfg_clone = cfg.clone();
    let handle = tokio::spawn(async move {
        let _ = service::start(&cfg_clone, srv_state).await;
    });

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    let admin_url = format!("http://{}/admin", mgmt_bind);
    let client = reqwest::Client::new();
    match client.get(&admin_url).send().await {
        Ok(resp) => {
            // When console is disabled, /admin should not be served -> usually 404.
            assert_eq!(
                resp.status().as_u16(),
                404,
                "expected 404 for /admin when console disabled, got {}",
                resp.status()
            );
        }
        Err(e) => {
            // Connection refused is also acceptable per requirements
            assert!(e.is_connect(), "unexpected error for /admin: {e}");
        }
    }

    // Management: API should be enabled on management endpoint, not on MCP endpoint
    let client = reqwest::Client::new();
    assert_eq!(
        client
            .get(format!("http://{}/api/plugins", mgmt_bind))
            .send()
            .await
            .unwrap()
            .status()
            .as_u16(),
        200
    );
    assert!(
        client
            .get(format!("http://{}/api/plugins", main_bind))
            .send()
            .await
            .unwrap()
            .status()
            .is_client_error()
    );

    handle.abort();
}
