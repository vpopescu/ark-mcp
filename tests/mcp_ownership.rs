/// Test MCP ownership filtering functionality.
use ark::{
    config::plugins::ArkPlugin,
    plugins::ToolSet,
    server::auth::AuthState,
    state::{ApplicationState, ArkState},
};
use rmcp::model::Tool;
use serde_json::Map;
use std::{borrow::Cow, sync::Arc};
use url::Url;

#[tokio::test]
async fn test_mcp_ownership_filtering() {
    // Create application state
    let state = Arc::new(ArkState::default());
    state.set_state(ApplicationState::Ready);

    // Create auth state (enabled for this test)
    let provider = ark::config::models::IdentityProviderConfig {
        name: "test".to_string(),
        client_id: "test-client".to_string(),
        client_secret: Some("test-secret".to_string()),
        authority: "https://example.com".to_string(),
        scopes: Some("openid profile email".to_string()),
        discovery: false,
        jwks_uri: Some("https://example.com/.well-known/jwks.json".to_string()),
        authorization_endpoint: Some("https://example.com/auth".to_string()),
        token_endpoint: Some("https://example.com/token".to_string()),
        ..Default::default()
    };

    let auth_cfg = ark::config::models::AuthConfig {
        enabled: true,
        provider: Some("test".to_string()),
        providers: vec![provider],
        session: Some(ark::config::models::SessionConfig::default()),
    };

    let app_state = Arc::new(ArkState::default());
    let auth_state = Arc::new(
        AuthState::new_with_state(&Some(auth_cfg), app_state, None)
            .await
            .unwrap(),
    );
    state.set_auth_state(auth_state);

    // Create test plugin owned by user1
    let owned_plugin = ArkPlugin {
        name: "owned_plugin".to_string(),
        url: Some(Url::parse("file:///test").unwrap()),
        owner: Some("provider1:domain:user1".to_string()),
        ..Default::default()
    };

    // Create test plugin that's public
    let public_plugin = ArkPlugin {
        name: "public_plugin".to_string(),
        url: Some(Url::parse("file:///test").unwrap()),
        owner: Some("*/*/*".to_string()),
        ..Default::default()
    };

    // Create test tools
    let owned_tool = Tool {
        name: Cow::Borrowed("owned_tool"),
        title: Some("Owned Tool".to_string()),
        description: Some(Cow::Borrowed("A tool owned by user1")),
        input_schema: Arc::new(Map::new()),
        output_schema: None,
        annotations: None,
        icons: None,
    };

    let public_tool = Tool {
        name: Cow::Borrowed("public_tool"),
        title: Some("Public Tool".to_string()),
        description: Some(Cow::Borrowed("A public tool")),
        input_schema: Arc::new(Map::new()),
        output_schema: None,
        annotations: None,
        icons: None,
    };

    // Register plugins and tools
    let owned_toolset = ToolSet {
        name: "owned_tool".to_string(),
        tools: vec![owned_tool],
    };

    let public_toolset = ToolSet {
        name: "public_tool".to_string(),
        tools: vec![public_tool],
    };

    state
        .register_plugin_with_executors(owned_plugin, owned_toolset, vec![])
        .await
        .unwrap();

    state
        .register_plugin_with_executors(public_plugin, public_toolset, vec![])
        .await
        .unwrap();

    // Create MCP handler
    let mcp_handler = ark::server::mcp::McpHandler {
        state: state.clone(),
    };

    // Test ownership filtering - user1 should see both tools
    assert!(
        mcp_handler
            .is_tool_accessible("owned_tool", Some("provider1:domain:user1"))
            .await
    );
    assert!(
        mcp_handler
            .is_tool_accessible("public_tool", Some("provider1:domain:user1"))
            .await
    );

    // Test ownership filtering - user2 should only see public tool
    assert!(
        !mcp_handler
            .is_tool_accessible("owned_tool", Some("provider1:domain:user2"))
            .await
    );
    assert!(
        mcp_handler
            .is_tool_accessible("public_tool", Some("provider1:domain:user2"))
            .await
    );

    // Test ownership filtering - no user (unauthenticated) should only see public tool
    assert!(!mcp_handler.is_tool_accessible("owned_tool", None).await);
    assert!(mcp_handler.is_tool_accessible("public_tool", None).await);
}
