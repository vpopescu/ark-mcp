use ark::config::{ArkConfig, McpTransport};
use std::fs;
use std::path::PathBuf;

fn write_temp_config(contents: &str, ext: &str) -> tempfile::NamedTempFile {
    let f = tempfile::Builder::new()
        .suffix(&format!(".{ext}"))
        .tempfile()
        .unwrap();
    fs::write(f.path(), contents).unwrap();
    f
}

/// Test that default bind addresses are used when no configuration file exists
/// and no CLI overrides are provided for either MCP or management servers.
#[test]
fn default_addresses_when_unset_anywhere() {
    // No file on disk: expect defaults for both mcp and management binds
    let cfg = ArkConfig::load_with_overrides(
        Some(PathBuf::from("__does_not_exist__")),
        //None,
        McpTransport::StreamableHTTP,
        None, // no MCP CLI override
        false,
        true,
        None, // no management disable_api override
        None, // no management bind CLI override
    )
    .unwrap();

    // Defaults come from server::constants
    assert_eq!(
        cfg.mcp_server.clone().unwrap().bind_address.unwrap(),
        ark::server::constants::DEFAULT_MCP_BIND_ADDRESS
    );
    assert_eq!(
        cfg.management_server
            .clone()
            .unwrap()
            .bind_address
            .as_deref(),
        Some(ark::server::constants::DEFAULT_MGMT_BIND_ADDRESS)
    );
}

/// Test that bind addresses specified in the configuration file are used
/// when no CLI overrides are provided for either MCP or management servers.
#[test]
fn config_file_addresses_when_no_cli_overrides() {
    let tf = write_temp_config(
        r#"
        mcp_server:
          bind_address: "127.0.0.1:4555"
          cors: "*"
          add_cors_headers: true

        management_server:
          bind_address: "127.0.0.1:4666"
          livez:
            enabled: true
            path: "/livez"
          readyz:
            enabled: true
            path: "/readyz"
        "#,
        "yaml",
    );

    let cfg = ArkConfig::load_with_overrides(
        Some(tf.path().to_path_buf()),
        //None,
        McpTransport::StreamableHTTP,
        None, // no MCP CLI override
        false,
        true,
        None,
        None, // no management CLI override
    )
    .unwrap();

    assert_eq!(
        cfg.mcp_server.clone().unwrap().bind_address.unwrap(),
        "127.0.0.1:4555"
    );
    assert_eq!(
        cfg.management_server
            .clone()
            .unwrap()
            .bind_address
            .as_deref(),
        Some("127.0.0.1:4666")
    );
}

/// Test that CLI-provided bind addresses take precedence over
/// configuration file settings for both MCP and management servers.
#[test]
fn cli_overrides_take_precedence_over_config_file() {
    let tf = write_temp_config(
        r#"
        mcp_server:
          bind_address: "127.0.0.1:5001"

        management_server:
          bind_address: "127.0.0.1:5002"
        "#,
        "yaml",
    );

    let cfg = ArkConfig::load_with_overrides(
        Some(tf.path().to_path_buf()),
        //None,
        McpTransport::StreamableHTTP,
        Some("127.0.0.1:6001".to_string()), // MCP CLI override
        false,
        true,
        None,
        Some("127.0.0.1:6002".to_string()), // management CLI override
    )
    .unwrap();

    assert_eq!(
        cfg.mcp_server.unwrap().bind_address.unwrap(),
        "127.0.0.1:6001"
    );
    assert_eq!(
        cfg.management_server.unwrap().bind_address.as_deref(),
        Some("127.0.0.1:6002")
    );
}
