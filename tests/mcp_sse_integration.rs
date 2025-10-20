/// Integration tests for MCP and SSE endpoints with authentication and ownership filtering.
use std::{sync::Arc, time::Duration};
use tempfile::TempDir;

use ark::server::roles::Role;
use ark::{
    config::{
        models::{AuthConfig, IdentityProviderConfig, SessionConfig},
        plugins::ArkPlugin,
    },
    plugins::ToolSet,
    server::{auth, mcp::McpHandler},
    state::{ApplicationState, ArkState},
};
use axum::{
    Router,
    body::Body,
    extract::Request,
    http::{Method, StatusCode, header},
    middleware,
    response::IntoResponse,
};
use rmcp::model::Tool;
use serde_json::Map;
use std::borrow::Cow;
use tower::ServiceExt;
use url::Url;

/// Helper to create test AuthState with database backing
async fn create_test_auth_state(
    auth_cfg: Option<AuthConfig>,
) -> (auth::AuthState, Option<TempDir>) {
    if let Some(config) = auth_cfg {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let app_state = Arc::new(ArkState::default());

        // Initialize database using the persist module
        let database = ark::server::persist::Database::with_path(&db_path).unwrap();
        {
            let mut db_guard = app_state.database.write().unwrap();
            *db_guard = Some(database);
        }

        let auth_state = auth::AuthState::new_with_state(&Some(config), app_state, None)
            .await
            .unwrap();

        (auth_state, Some(temp_dir))
    } else {
        // For disabled auth, we can use the simple constructor
        let app_state = Arc::new(ArkState::default());
        let auth_state = auth::AuthState::new_with_state(&None, app_state, None)
            .await
            .unwrap();
        (auth_state, None)
    }
}

/// Create auth state with enabled authentication for testing
async fn create_enabled_auth_state() -> Arc<auth::AuthState> {
    let provider = IdentityProviderConfig {
        name: "test-provider".to_string(),
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

    let auth_cfg = AuthConfig {
        enabled: true,
        provider: Some("test-provider".to_string()),
        providers: vec![provider],
        session: Some(SessionConfig::default()),
    };

    let (auth_state, _temp_dir) = create_test_auth_state(Some(auth_cfg)).await;
    Arc::new(auth_state)
}

/// Create application state with test plugins and tools
async fn create_test_app_state_with_plugins() -> Arc<ArkState> {
    let state = Arc::new(ArkState::default());
    state.set_state(ApplicationState::Ready);

    // Create tools for each plugin
    let user1_tool = Tool {
        name: Cow::Borrowed("user1_tool"),
        title: Some("User1 Tool".to_string()),
        description: Some(Cow::Borrowed("A tool owned by user1")),
        input_schema: Arc::new(Map::new()),
        output_schema: None,
        annotations: None,
        icons: None,
    };

    let user2_tool = Tool {
        name: Cow::Borrowed("user2_tool"),
        title: Some("User2 Tool".to_string()),
        description: Some(Cow::Borrowed("A tool owned by user2")),
        input_schema: Arc::new(Map::new()),
        output_schema: None,
        annotations: None,
        icons: None,
    };

    let public_tool = Tool {
        name: Cow::Borrowed("public_tool"),
        title: Some("Public Tool".to_string()),
        description: Some(Cow::Borrowed("A public tool accessible to all")),
        input_schema: Arc::new(Map::new()),
        output_schema: None,
        annotations: None,
        icons: None,
    };

    // Plugin owned by user1
    let user1_plugin = ArkPlugin {
        name: "user1_plugin".to_string(),
        url: Some(Url::parse("file:///user1").unwrap()),
        owner: Some("test-provider:domain:user1".to_string()),
        ..Default::default()
    };

    // Plugin owned by user2
    let user2_plugin = ArkPlugin {
        name: "user2_plugin".to_string(),
        url: Some(Url::parse("file:///user2").unwrap()),
        owner: Some("test-provider:domain:user2".to_string()),
        ..Default::default()
    };

    // Public plugin
    let public_plugin = ArkPlugin {
        name: "public_plugin".to_string(),
        url: Some(Url::parse("file:///public").unwrap()),
        owner: Some("*/*/*".to_string()),
        ..Default::default()
    };

    // Register plugins with their tools
    let user1_toolset = ToolSet {
        name: "user1_tool".to_string(),
        tools: vec![user1_tool],
    };

    let user2_toolset = ToolSet {
        name: "user2_tool".to_string(),
        tools: vec![user2_tool],
    };

    let public_toolset = ToolSet {
        name: "public_tool".to_string(),
        tools: vec![public_tool],
    };

    state
        .register_plugin_with_executors(user1_plugin, user1_toolset, vec![])
        .await
        .unwrap();

    state
        .register_plugin_with_executors(user2_plugin, user2_toolset, vec![])
        .await
        .unwrap();

    state
        .register_plugin_with_executors(public_plugin, public_toolset, vec![])
        .await
        .unwrap();

    state
}

/// Create test users
fn create_test_user(user_id: &str) -> auth::Principal {
    auth::Principal {
        subject: user_id.to_string(),
        email: Some(format!("{}@example.com", user_id)),
        name: Some(format!("Test User {}", user_id)),
        provider: "test-provider".to_string(),
        provider_kind: auth::ProviderKind::Oidc,
        tenant_id: None,
        oid: None,
        picture: None,
        roles: vec![Role::User],
        is_admin: false,
        groups: vec![],
    }
}

/// Test MCP tool listing with different authenticated users
#[tokio::test]
async fn test_mcp_list_tools_ownership_filtering() {
    let app_state = create_test_app_state_with_plugins().await;
    let auth_state = create_enabled_auth_state().await;
    app_state.set_auth_state(auth_state.clone());

    let mcp_handler = McpHandler {
        state: app_state.clone(),
    };

    // Test user1 - should see user1_tool and public_tool
    let user1_tools = get_accessible_tools(&mcp_handler, "user1").await;
    assert_eq!(user1_tools.len(), 2);
    assert!(user1_tools.contains(&"user1_tool".to_string()));
    assert!(user1_tools.contains(&"public_tool".to_string()));
    assert!(!user1_tools.contains(&"user2_tool".to_string()));

    // Test user2 - should see user2_tool and public_tool
    let user2_tools = get_accessible_tools(&mcp_handler, "user2").await;
    assert_eq!(user2_tools.len(), 2);
    assert!(user2_tools.contains(&"user2_tool".to_string()));
    assert!(user2_tools.contains(&"public_tool".to_string()));
    assert!(!user2_tools.contains(&"user1_tool".to_string()));

    // Test unauthenticated user - should only see public_tool
    let public_tools = get_accessible_tools(&mcp_handler, "").await;
    assert_eq!(public_tools.len(), 1);
    assert!(public_tools.contains(&"public_tool".to_string()));
    assert!(!public_tools.contains(&"user1_tool".to_string()));
    assert!(!public_tools.contains(&"user2_tool".to_string()));
}

/// Test MCP tool execution with ownership filtering
#[tokio::test]
async fn test_mcp_call_tool_ownership_filtering() {
    let app_state = create_test_app_state_with_plugins().await;
    let auth_state = create_enabled_auth_state().await;
    app_state.set_auth_state(auth_state.clone());

    let mcp_handler = McpHandler {
        state: app_state.clone(),
    };

    // Test user1 can access their own tool
    assert!(
        mcp_handler
            .is_tool_accessible("user1_tool", Some("test-provider:domain:user1"))
            .await
    );

    // Test user1 can access public tool
    assert!(
        mcp_handler
            .is_tool_accessible("public_tool", Some("test-provider:domain:user1"))
            .await
    );

    // Test user1 cannot access user2's tool
    assert!(
        !mcp_handler
            .is_tool_accessible("user2_tool", Some("test-provider:domain:user1"))
            .await
    );

    // Test user2 can access their own tool
    assert!(
        mcp_handler
            .is_tool_accessible("user2_tool", Some("test-provider:domain:user2"))
            .await
    );

    // Test user2 can access public tool
    assert!(
        mcp_handler
            .is_tool_accessible("public_tool", Some("test-provider:domain:user2"))
            .await
    );

    // Test user2 cannot access user1's tool
    assert!(
        !mcp_handler
            .is_tool_accessible("user1_tool", Some("test-provider:domain:user2"))
            .await
    );

    // Test unauthenticated user can only access public tool
    assert!(mcp_handler.is_tool_accessible("public_tool", None).await);
    assert!(!mcp_handler.is_tool_accessible("user1_tool", None).await);
    assert!(!mcp_handler.is_tool_accessible("user2_tool", None).await);
}

/// Test MCP endpoint authentication protection
#[tokio::test]
async fn test_mcp_endpoint_authentication_protection() {
    let app_state = create_test_app_state_with_plugins().await;

    // Create auth state with database backing and keep temp dir alive
    let provider = IdentityProviderConfig {
        name: "test-provider".to_string(),
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

    let auth_cfg = AuthConfig {
        enabled: true,
        provider: Some("test-provider".to_string()),
        providers: vec![provider],
        session: Some(SessionConfig::default()),
    };

    let (auth_state, _temp_dir) = create_test_auth_state(Some(auth_cfg)).await;
    let auth_state = Arc::new(auth_state);
    app_state.set_auth_state(auth_state.clone());

    let router = create_test_router_with_mcp(auth_state.clone(), app_state.clone()).await;

    // Test unauthenticated access to MCP endpoint - should be rejected
    let req = Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header("content-type", "application/json")
        .body(Body::from(
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
        ))
        .unwrap();

    let response = router.clone().oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    // Test authenticated access to MCP endpoint - should be allowed
    let user1 = create_test_user("user1");
    let session_id = auth_state
        .put_session(user1, Duration::from_secs(3600))
        .await;
    let cookie = format!("ark_session={}", session_id);

    let req = Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header("content-type", "application/json")
        .header("cookie", cookie)
        .body(Body::from(
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
        ))
        .unwrap();

    let response = router.clone().oneshot(req).await.unwrap();
    // Should not be UNAUTHORIZED (might be other status due to MCP protocol handling)
    assert_ne!(response.status(), StatusCode::UNAUTHORIZED);
}

/// Test SSE endpoint authentication protection
#[tokio::test]
async fn test_sse_endpoint_authentication_protection() {
    let app_state = create_test_app_state_with_plugins().await;

    // Create auth state with database backing and keep temp dir alive
    let provider = IdentityProviderConfig {
        name: "test-provider".to_string(),
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

    let auth_cfg = AuthConfig {
        enabled: true,
        provider: Some("test-provider".to_string()),
        providers: vec![provider],
        session: Some(SessionConfig::default()),
    };

    let (auth_state, _temp_dir) = create_test_auth_state(Some(auth_cfg)).await;
    let auth_state = Arc::new(auth_state);
    app_state.set_auth_state(auth_state.clone());

    let router = create_test_router_with_sse(auth_state.clone(), app_state.clone()).await;

    // Test unauthenticated access to SSE endpoint - should be rejected
    let req = Request::builder()
        .method(Method::GET)
        .uri("/sse")
        .header("accept", "text/event-stream")
        .body(Body::empty())
        .unwrap();

    let response = router.clone().oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    // Test authenticated access to SSE endpoint - should be allowed
    let user1 = create_test_user("user1");
    let session_id = auth_state
        .put_session(user1, Duration::from_secs(3600))
        .await;
    let cookie = format!("ark_session={}", session_id);

    let req = Request::builder()
        .method(Method::GET)
        .uri("/sse")
        .header("accept", "text/event-stream")
        .header("cookie", cookie)
        .body(Body::empty())
        .unwrap();

    let response = router.clone().oneshot(req).await.unwrap();
    // Should not be UNAUTHORIZED (might be other status due to SSE protocol handling)
    assert_ne!(response.status(), StatusCode::UNAUTHORIZED);
}

/// Test auth disabled - all tools should be accessible
#[tokio::test]
async fn test_auth_disabled_all_tools_accessible() {
    let app_state = create_test_app_state_with_plugins().await;
    let (auth_state, _temp_dir) = create_test_auth_state(None).await; // Disabled auth
    let auth_state = Arc::new(auth_state);
    app_state.set_auth_state(auth_state.clone());

    let mcp_handler = McpHandler {
        state: app_state.clone(),
    };

    // All tools should be accessible regardless of user
    assert!(
        mcp_handler
            .is_tool_accessible("user1_tool", Some("test-provider:domain:user1"))
            .await
    );
    assert!(
        mcp_handler
            .is_tool_accessible("user2_tool", Some("test-provider:domain:user1"))
            .await
    );
    assert!(
        mcp_handler
            .is_tool_accessible("public_tool", Some("test-provider:domain:user1"))
            .await
    );

    assert!(
        mcp_handler
            .is_tool_accessible("user1_tool", Some("test-provider:domain:user2"))
            .await
    );
    assert!(
        mcp_handler
            .is_tool_accessible("user2_tool", Some("test-provider:domain:user2"))
            .await
    );
    assert!(
        mcp_handler
            .is_tool_accessible("public_tool", Some("test-provider:domain:user2"))
            .await
    );

    assert!(mcp_handler.is_tool_accessible("user1_tool", None).await);
    assert!(mcp_handler.is_tool_accessible("user2_tool", None).await);
    assert!(mcp_handler.is_tool_accessible("public_tool", None).await);
}

// Helper functions

async fn get_accessible_tools(mcp_handler: &McpHandler, user_id: &str) -> Vec<String> {
    let user_gid = if user_id.is_empty() {
        None
    } else {
        Some(format!("test-provider:domain:{}", user_id))
    };

    let all_tools = ["user1_tool", "user2_tool", "public_tool"];
    let mut accessible_tools = Vec::new();

    for tool in all_tools {
        if mcp_handler
            .is_tool_accessible(tool, user_gid.as_deref())
            .await
        {
            accessible_tools.push(tool.to_string());
        }
    }

    accessible_tools
}

async fn create_test_router_with_mcp(
    auth_state: Arc<auth::AuthState>,
    _app_state: Arc<ArkState>,
) -> Router {
    let router = Router::new().route("/mcp", axum::routing::post(|| async { StatusCode::OK }));

    apply_auth_middleware(router, auth_state)
}

async fn create_test_router_with_sse(
    auth_state: Arc<auth::AuthState>,
    _app_state: Arc<ArkState>,
) -> Router {
    let router = Router::new().route("/sse", axum::routing::get(|| async { StatusCode::OK }));

    apply_auth_middleware(router, auth_state)
}

fn apply_auth_middleware(router: Router, auth_state: Arc<auth::AuthState>) -> Router {
    let auth_clone = auth_state.clone();
    router.layer(middleware::from_fn(
        move |req: Request<Body>, next: axum::middleware::Next| {
            let auth = auth_clone.clone();
            async move {
                if !auth.enabled {
                    return next.run(req).await;
                }

                let path = req.uri().path();
                let protected = auth::path_requires_auth(path);
                if !protected {
                    return next.run(req).await;
                }

                // Extract headers
                let cookie_str = req
                    .headers()
                    .get(header::COOKIE)
                    .and_then(|h| h.to_str().ok());

                // Check session cookie
                if let Some(cookie) = cookie_str
                    && let Some(principal) =
                        auth::extract_session_user_from_cookie(&auth, cookie).await
                {
                    let mut req = req;
                    req.extensions_mut().insert(principal);
                    return next.run(req).await;
                }

                // Return unauthorized
                let body = axum::Json(serde_json::json!({
                    "error": "unauthorized",
                }));
                let mut resp = body.into_response();
                *resp.status_mut() = StatusCode::UNAUTHORIZED;
                resp
            }
        },
    ))
}
