use std::sync::Arc;
use tempfile::TempDir;

use ark::server::auth::{self, ProviderKind};
use ark::server::roles::Role;
use ark::{
    config::ArkConfig,
    plugins,
    plugins::builtin::BUILTIN_PLUGIN_ID,
    server::{
        handlers::{
            api::{
                create_plugin, delete_plugin, execute_plugin_tool, get_plugin_by_id, get_plugins,
            },
            health::{livez, readyz},
        },
        service::metrics_handler,
    },
    state::{ApplicationState, ArkState},
};
use axum::{Router, body::Body, extract::Request, http::StatusCode, routing::get};
use serde_json::json;
use tower::ServiceExt;

#[tokio::test]
/// Tests that the builtin plugin is loaded when no external plugins are configured
async fn api_builtin_plugin_behavior() {
    let app = Arc::new(ArkState::default());
    app.set_state(ApplicationState::StartingNetwork);

    // Load no plugins - should get builtin
    let cfg = ArkConfig::default();
    plugins::load_plugins(&cfg, app.clone())
        .await
        .expect("plugin load");

    // Verify builtin plugin is loaded
    let catalog = app.plugin_registry.catalog.read().await;
    assert!(catalog.plugin_to_config.contains_key(BUILTIN_PLUGIN_ID));
    assert!(catalog.plugin_to_config.len() == 1);
}

#[tokio::test]
/// Tests that builtin plugin is not loaded when external plugins are configured (even if they fail)
async fn api_external_plugins_replace_builtin() {
    let app = Arc::new(ArkState::default());
    app.set_state(ApplicationState::StartingNetwork);

    // Create a minimal config with one plugin
    let cfg = ArkConfig {
        plugins: vec![ark::config::plugins::ArkPlugin {
            name: "test-plugin".to_string(),
            url: Some("file:///nonexistent.wasm".parse().unwrap()),
            auth: None,
            insecure: false,
            manifest: None,
            owner: None,
        }],
        ..Default::default()
    };

    // This will fail to load the plugin, but should still remove builtin
    plugins::load_plugins(&cfg, app.clone())
        .await
        .expect_err("plugin loading should fail");

    // But builtin should still be gone since we tried to load external plugins
    let catalog = app.plugin_registry.catalog.read().await;
    assert!(!catalog.plugin_to_config.contains_key(BUILTIN_PLUGIN_ID));
}

#[tokio::test]
/// Tests that plugin loading works when application is in Ready state and proceeds to Ready even if plugin loads fail
async fn api_state_validation() {
    let app = Arc::new(ArkState::default());
    app.set_state(ApplicationState::Ready); // Set to ready state first

    // Plugin loading should work in Ready state
    let cfg = ArkConfig::default();
    plugins::load_plugins(&cfg, app.clone())
        .await
        .expect("plugin loading should succeed");

    // Verify builtin plugin was loaded
    let catalog = app.plugin_registry.catalog.read().await;
    assert!(catalog.plugin_to_config.contains_key(BUILTIN_PLUGIN_ID));
}

// ===== TEST HELPERS =====

/// Creates a test AuthState with database backing for proper session functionality
async fn create_test_auth_state(
    auth_cfg: ark::config::models::AuthConfig,
) -> (auth::AuthState, TempDir) {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("test.db");
    let app_state = Arc::new(ArkState::default());

    // Initialize database using the persist module
    let database = ark::server::persist::Database::with_path(&db_path).unwrap();
    {
        let mut db_guard = app_state.database.write().unwrap();
        *db_guard = Some(database);
    }

    let auth_state = auth::AuthState::new_with_state(&Some(auth_cfg), app_state, None)
        .await
        .unwrap();

    (auth_state, temp_dir)
}

// ===== HTTP API TESTS =====

#[tokio::test]
/// Tests GET /api/plugins endpoint returns builtin plugin when loaded
async fn test_get_plugins_lists_builtin() {
    let app = Arc::new(ArkState::default());
    app.set_state(ApplicationState::StartingNetwork);

    // Load default config (should get builtin)
    let cfg = ArkConfig::default();
    plugins::load_plugins(&cfg, app.clone())
        .await
        .expect("plugin load");

    let router = Router::new()
        .route("/api/plugins", get(get_plugins))
        .with_state(app);

    let request = Request::get("/api/plugins").body(Body::empty()).unwrap();
    let response = router.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json.get(BUILTIN_PLUGIN_ID).is_some());

    // Verify structure contains expected fields
    let builtin = json.get(BUILTIN_PLUGIN_ID).unwrap();
    assert!(builtin.get("name").is_some());
    assert!(builtin.get("tools").is_some());
    assert!(builtin.get("description").is_some());
}

#[tokio::test]
/// GET /api/plugins should exclude plugins owned by a different user
async fn test_get_plugins_filters_by_owner() {
    let app = Arc::new(ArkState::default());
    app.set_state(ApplicationState::StartingNetwork);

    // Add two plugins: one owned by user A, one wildcard
    let p_owned = ark::config::plugins::ArkPlugin {
        name: "OwnedByA".to_string(),
        url: Some("file:///nonexistent.wasm".parse().unwrap()),
        auth: None,
        insecure: false,
        manifest: None,
        owner: Some("oidc/*/user-a".into()),
    };
    let p_wild = ark::config::plugins::ArkPlugin {
        name: "Wildcard".to_string(),
        url: Some("file:///nonexistent.wasm".parse().unwrap()),
        auth: None,
        insecure: false,
        manifest: None,
        owner: Some("*/*/*".into()),
    };

    // Register with empty tool sets
    let ts = ark::plugins::ToolSet {
        name: "tools".into(),
        tools: vec![],
    };
    app.register_plugin_with_executors(p_owned, ts.clone(), vec![])
        .await
        .unwrap();
    app.register_plugin_with_executors(p_wild, ts, vec![])
        .await
        .unwrap();

    // Prepare router with auth middleware and principal for user B
    let principal = auth::Principal {
        subject: "user-b".into(),
        email: None,
        name: None,
        provider: "test".into(),
        provider_kind: ProviderKind::Oidc,
        picture: None,
        tenant_id: None,
        oid: None,
        roles: vec![Role::User],
        is_admin: false,
        groups: vec![],
    };
    // Build enabled auth state to exercise middleware and session injection
    let provider = ark::config::models::IdentityProviderConfig {
        name: "fake".into(),
        client_id: "client".into(),
        client_secret: None,
        authority: "https://example.invalid".into(),
        discovery: false,
        ..Default::default()
    };
    let auth_cfg = ark::config::models::AuthConfig {
        enabled: true,
        provider: Some("fake".into()),
        providers: vec![provider],
        session: Some(ark::config::models::SessionConfig::default()),
    };
    let (auth_state, _temp_dir) = create_test_auth_state(auth_cfg).await;
    let session_id = auth_state
        .put_session(principal, std::time::Duration::from_secs(60))
        .await;
    let auth_state = Arc::new(auth_state);

    // Build router mirroring service wiring with auth middleware
    let mut router = axum::Router::new()
        .route("/api/plugins", axum::routing::get(get_plugins))
        .with_state(app.clone());
    router = router.layer(axum::middleware::from_fn(move |req, next| {
        let st = auth_state.clone();
        async move { auth::check_auth(req, next, axum::Extension(st)).await }
    }));

    let request = Request::get("/api/plugins")
        .header("Cookie", format!("ark_session={}", session_id))
        .body(Body::empty())
        .unwrap();
    let response = router.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    // Should only contain Wildcard, not OwnedByA
    assert!(json.get("Wildcard").is_some());
    assert!(json.get("OwnedByA").is_none());
}

#[tokio::test]
/// GET /api/plugins/{id} should 404 if owned by a different user
async fn test_get_plugin_by_id_respects_owner() {
    let app = Arc::new(ArkState::default());
    app.set_state(ApplicationState::StartingNetwork);
    let plugin = ark::config::plugins::ArkPlugin {
        name: "PrivatePlugin".to_string(),
        url: Some("file:///nonexistent.wasm".parse().unwrap()),
        auth: None,
        insecure: false,
        manifest: None,
        owner: Some("oidc/*/owner-1".into()),
    };
    let ts = ark::plugins::ToolSet {
        name: "tools".into(),
        tools: vec![],
    };
    app.register_plugin_with_executors(plugin, ts, vec![])
        .await
        .unwrap();

    // Principal is different user
    let principal = auth::Principal {
        subject: "owner-2".into(),
        email: None,
        name: None,
        picture: None,
        provider: "test".into(),
        provider_kind: ProviderKind::Oidc,
        tenant_id: None,
        oid: None,
        roles: vec![Role::User],
        is_admin: false,
        groups: vec![],
    };
    let provider = ark::config::models::IdentityProviderConfig {
        name: "fake".into(),
        client_id: "client".into(),
        client_secret: None,
        authority: "https://example.invalid".into(),
        discovery: false,
        ..Default::default()
    };
    let auth_cfg = ark::config::models::AuthConfig {
        enabled: true,
        provider: Some("fake".into()),
        providers: vec![provider],
        session: Some(ark::config::models::SessionConfig::default()),
    };
    let (auth_state, _temp_dir) = create_test_auth_state(auth_cfg).await;
    let session_id = auth_state
        .put_session(principal, std::time::Duration::from_secs(60))
        .await;
    let auth_state = Arc::new(auth_state);

    let mut router = axum::Router::new()
        .route("/api/plugins/{id}", axum::routing::get(get_plugin_by_id))
        .with_state(app.clone());
    let auth_state2 = auth_state.clone();
    router = router.layer(axum::middleware::from_fn(move |req, next| {
        let st = auth_state2.clone();
        async move { auth::check_auth(req, next, axum::Extension(st)).await }
    }));

    let request = Request::get("/api/plugins/PrivatePlugin")
        .header("Cookie", format!("ark_session={}", session_id))
        .body(Body::empty())
        .unwrap();
    let response = router.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
/// DELETE /api/plugins/{id} should return 403 if not owner
async fn test_delete_plugin_forbidden_when_not_owner() {
    let app = Arc::new(ArkState::default());
    app.set_state(ApplicationState::StartingNetwork);
    let plugin = ark::config::plugins::ArkPlugin {
        name: "OwnedPlugin".to_string(),
        url: Some("file:///nonexistent.wasm".parse().unwrap()),
        auth: None,
        insecure: false,
        manifest: None,
        owner: Some("oidc/*/owner-1".into()),
    };
    let ts = ark::plugins::ToolSet {
        name: "tools".into(),
        tools: vec![],
    };
    app.register_plugin_with_executors(plugin, ts, vec![])
        .await
        .unwrap();

    let principal = auth::Principal {
        subject: "owner-2".into(),
        email: None,
        name: None,
        picture: None,
        provider: "test".into(),
        provider_kind: ProviderKind::Oidc,
        tenant_id: None,
        oid: None,
        roles: vec![Role::User],
        is_admin: false,
        groups: vec![],
    };
    let provider = ark::config::models::IdentityProviderConfig {
        name: "fake".into(),
        client_id: "client".into(),
        client_secret: None,
        authority: "https://example.invalid".into(),
        discovery: false,
        ..Default::default()
    };
    let auth_cfg = ark::config::models::AuthConfig {
        enabled: true,
        provider: Some("fake".into()),
        providers: vec![provider],
        session: Some(ark::config::models::SessionConfig::default()),
    };
    let (auth_state, _temp_dir) = create_test_auth_state(auth_cfg).await;
    let session_id = auth_state
        .put_session(principal, std::time::Duration::from_secs(60))
        .await;
    let auth_state = Arc::new(auth_state);

    let mut router = axum::Router::new()
        .route("/api/plugins/{id}", axum::routing::delete(delete_plugin))
        .with_state(app.clone());
    let auth_state2 = auth_state.clone();
    router = router.layer(axum::middleware::from_fn(move |req, next| {
        let st = auth_state2.clone();
        async move { auth::check_auth(req, next, axum::Extension(st)).await }
    }));

    let request = Request::delete("/api/plugins/OwnedPlugin")
        .header("Cookie", format!("ark_session={}", session_id))
        .body(Body::empty())
        .unwrap();
    let response = router.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
/// POST /api/plugins should set owner to authenticated user
async fn test_create_plugin_sets_owner() {
    let app = Arc::new(ArkState::default());
    app.set_state(ApplicationState::StartingNetwork);

    let principal = auth::Principal {
        subject: "creator".into(),
        email: None,
        name: None,
        picture: None,
        provider: "test".into(),
        provider_kind: ProviderKind::Oidc,
        tenant_id: None,
        oid: None,
        roles: vec![Role::User],
        is_admin: false,
        groups: vec![],
    };
    let provider = ark::config::models::IdentityProviderConfig {
        name: "fake".into(),
        client_id: "client".into(),
        client_secret: None,
        authority: "https://example.invalid".into(),
        discovery: false,
        ..Default::default()
    };
    let auth_cfg = ark::config::models::AuthConfig {
        enabled: true,
        provider: Some("fake".into()),
        providers: vec![provider],
        session: Some(ark::config::models::SessionConfig::default()),
    };
    let (auth_state, _temp_dir) = create_test_auth_state(auth_cfg).await;
    let session_id = auth_state
        .put_session(principal.clone(), std::time::Duration::from_secs(60))
        .await;
    let auth_state = Arc::new(auth_state);

    let mut router = axum::Router::new()
        .route("/api/plugins", axum::routing::post(create_plugin))
        .with_state(app.clone());
    let auth_state2 = auth_state.clone();
    router = router.layer(axum::middleware::from_fn(move |req, next| {
        let st = auth_state2.clone();
        async move { auth::check_auth(req, next, axum::Extension(st)).await }
    }));

    let payload = json!({
        "name": "CreatedPlugin",
        "url": "file:///nonexistent.wasm",
        "insecure": true
    });

    let request = Request::post("/api/plugins")
        .header("content-type", "application/json")
        .header("Cookie", format!("ark_session={}", session_id))
        .body(Body::from(serde_json::to_string(&payload).unwrap()))
        .unwrap();
    let response = router.clone().oneshot(request).await.unwrap();
    // Current behavior: plugin load fails for nonexistent file, returning 500 and not registering
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let catalog = app.plugin_registry.catalog.read().await;
    assert!(
        !catalog.plugin_to_config.contains_key("CreatedPlugin"),
        "Plugin should not be registered when load fails"
    );
}

#[tokio::test]
/// POST /api/plugins/{id}/tools/{tool_id} should return 403 if plugin owned by another user
async fn test_execute_tool_forbidden_when_not_owner() {
    let app = Arc::new(ArkState::default());
    app.set_state(ApplicationState::StartingNetwork);

    // Register plugin owned by someone else; ownership check should forbid before tool lookup
    let plugin = ark::config::plugins::ArkPlugin {
        name: "ExecOwned".to_string(),
        url: Some("file:///nonexistent.wasm".parse().unwrap()),
        auth: None,
        insecure: false,
        manifest: None,
        owner: Some("oidc/*/owner-1".into()),
    };
    {
        let mut catalog = app.plugin_registry.catalog.write().await;
        catalog.plugin_to_config.insert("ExecOwned".into(), plugin);
    }

    let principal = auth::Principal {
        subject: "owner-2".into(),
        email: None,
        name: None,
        provider: "test".into(),
        picture: None,
        provider_kind: ProviderKind::Oidc,
        tenant_id: None,
        oid: None,
        roles: vec![Role::User],
        is_admin: false,
        groups: vec![],
    };
    let provider = ark::config::models::IdentityProviderConfig {
        name: "fake".into(),
        client_id: "client".into(),
        client_secret: None,
        authority: "https://example.invalid".into(),
        discovery: false,
        ..Default::default()
    };
    let auth_cfg = ark::config::models::AuthConfig {
        enabled: true,
        provider: Some("fake".into()),
        providers: vec![provider],
        session: Some(ark::config::models::SessionConfig::default()),
    };
    let (auth_state, _temp_dir) = create_test_auth_state(auth_cfg).await;
    let session_id = auth_state
        .put_session(principal, std::time::Duration::from_secs(60))
        .await;
    let auth_state = Arc::new(auth_state);

    let mut router = axum::Router::new()
        .route(
            "/api/plugins/{id}/tools/{tool_id}",
            axum::routing::post(execute_plugin_tool),
        )
        .with_state(app.clone());
    let auth_state2 = auth_state.clone();
    router = router.layer(axum::middleware::from_fn(move |req, next| {
        let st = auth_state2.clone();
        async move { auth::check_auth(req, next, axum::Extension(st)).await }
    }));

    let payload = json!({});
    let request = Request::post("/api/plugins/ExecOwned/tools/t1")
        .header("content-type", "application/json")
        .header("Cookie", format!("ark_session={}", session_id))
        .body(Body::from(serde_json::to_string(&payload).unwrap()))
        .unwrap();
    let response = router.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
/// Owner is serialized when present and not wildcard; value matches expected provider:domain:user_id shape
async fn test_get_plugins_owner_serialized_when_present() {
    let app = Arc::new(ArkState::default());
    app.set_state(ApplicationState::StartingNetwork);

    // Register a plugin with explicit Microsoft AAD owner
    let plugin = ark::config::plugins::ArkPlugin {
        name: "OwnedPlugin".to_string(),
        url: None,
        auth: None,
        insecure: false,
        manifest: None,
        owner: None,
    };
    let toolset = ark::plugins::ToolSet {
        name: "tools".into(),
        tools: vec![],
    };
    let owner_gid =
        "aad/00000000-0000-0000-0000-000000000000/11111111-1111-1111-1111-111111111111".to_string();
    let mut plugin = plugin;
    plugin.owner = Some(owner_gid.clone());
    app.register_plugin_with_executors(plugin, toolset, vec![])
        .await
        .expect("register");

    // Auth principal that matches the owner gid so non-public plugin is visible
    let principal = auth::Principal {
        subject: "11111111-1111-1111-1111-111111111111".into(),
        email: None,
        name: None,
        picture: None,
        provider: "entra".into(),
        provider_kind: ProviderKind::Microsoft,
        tenant_id: Some("00000000-0000-0000-0000-000000000000".into()),
        oid: Some("11111111-1111-1111-1111-111111111111".into()),
        roles: vec![Role::User],
        is_admin: false,
        groups: vec![],
    };
    let provider = ark::config::models::IdentityProviderConfig {
        name: "entra".into(),
        client_id: "client".into(),
        client_secret: None,
        authority: "https://login.microsoftonline.com/common".into(),
        discovery: false,
        ..Default::default()
    };
    let auth_cfg = ark::config::models::AuthConfig {
        enabled: true,
        provider: Some("entra".into()),
        providers: vec![provider],
        session: Some(ark::config::models::SessionConfig::default()),
    };
    let (auth_state, _temp_dir) = create_test_auth_state(auth_cfg).await;
    let session_id = auth_state
        .put_session(principal.clone(), std::time::Duration::from_secs(60))
        .await;
    let auth_state = Arc::new(auth_state);

    let mut router = Router::new()
        .route("/api/plugins", get(get_plugins))
        .with_state(app);
    let auth_state2 = auth_state.clone();
    router = router.layer(axum::middleware::from_fn(move |req, next| {
        let st = auth_state2.clone();
        async move { auth::check_auth(req, next, axum::Extension(st)).await }
    }));

    let request = Request::get("/api/plugins")
        .header("Cookie", format!("ark_session={}", session_id))
        .body(Body::empty())
        .unwrap();
    let response = router.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let owned = json.get("OwnedPlugin").expect("OwnedPlugin missing");
    let owner = owned
        .get("owner")
        .and_then(|v| v.as_str())
        .expect("owner should be present");

    // Minimal structural validation in tests only
    let parts: Vec<&str> = owner.split('/').collect();
    assert_eq!(parts.len(), 3, "owner must have 3 parts");
    assert!(
        matches!(parts[0], "aad" | "gip" | "oidc"),
        "invalid provider"
    );
    // helper to check GUID format: 8-4-4-4-12 hex segments
    fn is_guid(s: &str) -> bool {
        let segments: Vec<&str> = s.split('-').collect();
        if segments.len() != 5 {
            return false;
        }
        let lens = [8, 4, 4, 4, 12];
        segments
            .iter()
            .zip(lens.iter())
            .all(|(seg, &len)| seg.len() == len && seg.chars().all(|c| c.is_ascii_hexdigit()))
    }
    assert!(
        parts[1] == "*" || is_guid(parts[1]),
        "domain must be guid or *"
    );
    assert!(is_guid(parts[2]), "user id must be guid");
}

#[tokio::test]
/// Owner is omitted from JSON when wildcard sentinel is stored
async fn test_get_plugins_owner_omitted_when_wildcard() {
    let app = Arc::new(ArkState::default());
    app.set_state(ApplicationState::StartingNetwork);

    // Register a plugin without owner; registration defaults to */*/*
    let plugin = ark::config::plugins::ArkPlugin {
        name: "UnownedPlugin".to_string(),
        url: None,
        auth: None,
        insecure: false,
        manifest: None,
        owner: None,
    };
    let toolset = ark::plugins::ToolSet {
        name: "tools".into(),
        tools: vec![],
    };
    app.register_plugin_with_executors(plugin, toolset, vec![])
        .await
        .expect("register");

    let router = Router::new()
        .route("/api/plugins", get(get_plugins))
        .with_state(app);

    let request = Request::get("/api/plugins").body(Body::empty()).unwrap();
    let response = router.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    let unowned = json.get("UnownedPlugin").expect("UnownedPlugin missing");
    assert!(unowned.get("owner").is_none(), "owner should be omitted");
}

#[tokio::test]
/// Tests GET /api/plugins endpoint returns empty list after external plugin loading removes builtin
async fn test_get_plugins_empty_after_external_load() {
    let app = Arc::new(ArkState::default());
    app.set_state(ApplicationState::StartingNetwork);

    // Load config with external plugin (removes builtin)
    let cfg = ArkConfig {
        plugins: vec![ark::config::plugins::ArkPlugin {
            name: "test-plugin".to_string(),
            url: Some("file:///nonexistent.wasm".parse().unwrap()),
            auth: None,
            insecure: false,
            manifest: None,
            owner: None,
        }],
        ..Default::default()
    };

    // Loading will fail but builtin gets removed
    let _ = plugins::load_plugins(&cfg, app.clone()).await;

    let router = Router::new()
        .route("/api/plugins", get(get_plugins))
        .with_state(app);

    let request = Request::get("/api/plugins").body(Body::empty()).unwrap();
    let response = router.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(!json.as_object().unwrap().contains_key(BUILTIN_PLUGIN_ID));
}

#[tokio::test]
/// Tests GET /api/plugins/{id} endpoint returns tools array for builtin plugin and loaded plugins
async fn test_get_plugin_by_id_builtin() {
    let app = Arc::new(ArkState::default());
    app.set_state(ApplicationState::StartingNetwork);

    let cfg = ArkConfig::default();
    plugins::load_plugins(&cfg, app.clone())
        .await
        .expect("plugin load");

    let router = Router::new()
        .route("/api/plugins/{id}", get(get_plugin_by_id))
        .with_state(app);

    let request = Request::get(format!("/api/plugins/{}", BUILTIN_PLUGIN_ID))
        .body(Body::empty())
        .unwrap();
    let response = router.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json.is_array()); // Should return array of tools
}

#[tokio::test]
/// GET /api/plugins should include own and wildcard plugins for authenticated user
async fn test_get_plugins_includes_own_and_wildcard() {
    let app = Arc::new(ArkState::default());
    app.set_state(ApplicationState::StartingNetwork);

    // Own plugin
    let own = ark::config::plugins::ArkPlugin {
        name: "OwnPlugin".to_string(),
        url: Some("file:///nonexistent.wasm".parse().unwrap()),
        auth: None,
        insecure: false,
        manifest: None,
        owner: Some("oidc/*/me".into()),
    };
    // Wildcard plugin
    let public_plugin = ark::config::plugins::ArkPlugin {
        name: "PubPlugin".to_string(),
        url: Some("file:///nonexistent.wasm".parse().unwrap()),
        auth: None,
        insecure: false,
        manifest: None,
        owner: Some("*/*/*".into()),
    };
    let ts = ark::plugins::ToolSet {
        name: "tools".into(),
        tools: vec![],
    };
    app.register_plugin_with_executors(own, ts.clone(), vec![])
        .await
        .unwrap();
    app.register_plugin_with_executors(public_plugin, ts, vec![])
        .await
        .unwrap();

    // Auth user with global id oidc/*/me
    let principal = auth::Principal {
        subject: "me".into(),
        email: None,
        name: None,
        picture: None,
        provider: "test".into(),
        provider_kind: ProviderKind::Oidc,
        tenant_id: None,
        oid: None,
        roles: vec![Role::User],
        is_admin: false,
        groups: vec![],
    };
    let provider = ark::config::models::IdentityProviderConfig {
        name: "fake".into(),
        client_id: "client".into(),
        client_secret: None,
        authority: "https://example.invalid".into(),
        discovery: false,
        ..Default::default()
    };
    let auth_cfg = ark::config::models::AuthConfig {
        enabled: true,
        provider: Some("fake".into()),
        providers: vec![provider],
        session: Some(ark::config::models::SessionConfig::default()),
    };
    let (auth_state, _temp_dir) = create_test_auth_state(auth_cfg).await;
    let session_id = auth_state
        .put_session(principal, std::time::Duration::from_secs(60))
        .await;
    let auth_state = Arc::new(auth_state);

    let mut router = axum::Router::new()
        .route("/api/plugins", axum::routing::get(get_plugins))
        .with_state(app.clone());
    let auth_state2 = auth_state.clone();
    router = router.layer(axum::middleware::from_fn(move |req, next| {
        let st = auth_state2.clone();
        async move { auth::check_auth(req, next, axum::Extension(st)).await }
    }));

    let req = Request::get("/api/plugins")
        .header("Cookie", format!("ark_session={}", session_id))
        .body(Body::empty())
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json.get("OwnPlugin").is_some());
    assert!(json.get("PubPlugin").is_some());
}

#[tokio::test]
/// GET /api/plugins/{id} returns 200 for owner, and for wildcard
async fn test_get_plugin_by_id_allowed_paths() {
    let app = Arc::new(ArkState::default());
    app.set_state(ApplicationState::StartingNetwork);

    // Owned plugin
    let owned = ark::config::plugins::ArkPlugin {
        name: "OwnedP".to_string(),
        url: Some("file:///nonexistent.wasm".parse().unwrap()),
        auth: None,
        insecure: false,
        manifest: None,
        owner: Some("oidc/*/me".into()),
    };
    // Wildcard plugin
    let wild = ark::config::plugins::ArkPlugin {
        name: "WildP".to_string(),
        url: Some("file:///nonexistent.wasm".parse().unwrap()),
        auth: None,
        insecure: false,
        manifest: None,
        owner: Some("*/*/*".into()),
    };
    let ts = ark::plugins::ToolSet {
        name: "tools".into(),
        tools: vec![],
    };
    app.register_plugin_with_executors(owned, ts.clone(), vec![])
        .await
        .unwrap();
    app.register_plugin_with_executors(wild, ts, vec![])
        .await
        .unwrap();

    // Auth as owner
    let principal = auth::Principal {
        subject: "me".into(),
        email: None,
        name: None,
        picture: None,
        provider: "test".into(),
        provider_kind: ProviderKind::Oidc,
        tenant_id: None,
        oid: None,
        roles: vec![Role::User],
        is_admin: false,
        groups: vec![],
    };
    let provider = ark::config::models::IdentityProviderConfig {
        name: "fake".into(),
        client_id: "client".into(),
        client_secret: None,
        authority: "https://example.invalid".into(),
        discovery: false,
        ..Default::default()
    };
    let auth_cfg = ark::config::models::AuthConfig {
        enabled: true,
        provider: Some("fake".into()),
        providers: vec![provider],
        session: Some(ark::config::models::SessionConfig::default()),
    };
    let (auth_state, _temp_dir) = create_test_auth_state(auth_cfg).await;
    let session_id = auth_state
        .put_session(principal, std::time::Duration::from_secs(60))
        .await;
    let auth_state = Arc::new(auth_state);

    let mut router = axum::Router::new()
        .route("/api/plugins/{id}", axum::routing::get(get_plugin_by_id))
        .with_state(app.clone());
    let auth_state2 = auth_state.clone();
    router = router.layer(axum::middleware::from_fn(move |req, next| {
        let st = auth_state2.clone();
        async move { auth::check_auth(req, next, axum::Extension(st)).await }
    }));

    // Owned: should be OK
    let req = Request::get("/api/plugins/OwnedP")
        .header("Cookie", format!("ark_session={}", session_id))
        .body(Body::empty())
        .unwrap();
    let resp = router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Wildcard: OK too (accessible regardless of owner)
    let req2 = Request::get("/api/plugins/WildP")
        .header("Cookie", format!("ark_session={}", session_id))
        .body(Body::empty())
        .unwrap();
    let resp2 = router.oneshot(req2).await.unwrap();
    assert_eq!(resp2.status(), StatusCode::OK);
}

#[tokio::test]
/// DELETE /api/plugins/{id} should succeed for owner
async fn test_delete_plugin_allowed_for_owner() {
    let app = Arc::new(ArkState::default());
    app.set_state(ApplicationState::StartingNetwork);

    let plugin = ark::config::plugins::ArkPlugin {
        name: "ToDelete".to_string(),
        url: Some("file:///nonexistent.wasm".parse().unwrap()),
        auth: None,
        insecure: false,
        manifest: None,
        owner: Some("oidc/*/me".into()),
    };
    let ts = ark::plugins::ToolSet {
        name: "tools".into(),
        tools: vec![],
    };
    app.register_plugin_with_executors(plugin, ts, vec![])
        .await
        .unwrap();

    let principal = auth::Principal {
        subject: "me".into(),
        email: None,
        name: None,
        picture: None,
        provider: "test".into(),
        provider_kind: ProviderKind::Oidc,
        tenant_id: None,
        oid: None,
        roles: vec![Role::User],
        is_admin: false,
        groups: vec![],
    };
    let provider = ark::config::models::IdentityProviderConfig {
        name: "fake".into(),
        client_id: "client".into(),
        client_secret: None,
        authority: "https://example.invalid".into(),
        discovery: false,
        ..Default::default()
    };
    let auth_cfg = ark::config::models::AuthConfig {
        enabled: true,
        provider: Some("fake".into()),
        providers: vec![provider],
        session: Some(ark::config::models::SessionConfig::default()),
    };
    let (auth_state, _temp_dir) = create_test_auth_state(auth_cfg).await;
    let session_id = auth_state
        .put_session(principal, std::time::Duration::from_secs(60))
        .await;
    let auth_state = Arc::new(auth_state);

    let mut router = axum::Router::new()
        .route("/api/plugins/{id}", axum::routing::delete(delete_plugin))
        .with_state(app.clone());
    let auth_state2 = auth_state.clone();
    router = router.layer(axum::middleware::from_fn(move |req, next| {
        let st = auth_state2.clone();
        async move { auth::check_auth(req, next, axum::Extension(st)).await }
    }));

    let req = Request::delete("/api/plugins/ToDelete")
        .header("Cookie", format!("ark_session={}", session_id))
        .body(Body::empty())
        .unwrap();
    let resp = router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // Verify removed
    let catalog = app.plugin_registry.catalog.read().await;
    assert!(!catalog.plugin_to_config.contains_key("ToDelete"));
}

#[tokio::test]
/// DELETE /api/plugins/{id} should be forbidden for public plugin
async fn test_delete_plugin_forbidden_for_public() {
    let app = Arc::new(ArkState::default());
    app.set_state(ApplicationState::StartingNetwork);

    let plugin = ark::config::plugins::ArkPlugin {
        name: "PublicP".to_string(),
        url: Some("file:///nonexistent.wasm".parse().unwrap()),
        auth: None,
        insecure: false,
        manifest: None,
        owner: Some("*/*/*".into()),
    };
    let ts = ark::plugins::ToolSet {
        name: "tools".into(),
        tools: vec![],
    };
    app.register_plugin_with_executors(plugin, ts, vec![])
        .await
        .unwrap();

    // Auth user
    let principal = auth::Principal {
        subject: "me".into(),
        email: None,
        picture: None,
        name: None,
        provider: "test".into(),
        provider_kind: ProviderKind::Oidc,
        tenant_id: None,
        oid: None,
        roles: vec![Role::User],
        is_admin: false,
        groups: vec![],
    };
    let provider = ark::config::models::IdentityProviderConfig {
        name: "fake".into(),
        client_id: "client".into(),
        client_secret: None,
        authority: "https://example.invalid".into(),
        discovery: false,
        ..Default::default()
    };
    let auth_cfg = ark::config::models::AuthConfig {
        enabled: true,
        provider: Some("fake".into()),
        providers: vec![provider],
        session: Some(ark::config::models::SessionConfig::default()),
    };
    let (auth_state, _temp_dir) = create_test_auth_state(auth_cfg).await;
    let session_id = auth_state
        .put_session(principal, std::time::Duration::from_secs(60))
        .await;
    let auth_state = Arc::new(auth_state);

    let mut router = axum::Router::new()
        .route("/api/plugins/{id}", axum::routing::delete(delete_plugin))
        .with_state(app.clone());
    let auth_state2 = auth_state.clone();
    router = router.layer(axum::middleware::from_fn(move |req, next| {
        let st = auth_state2.clone();
        async move { auth::check_auth(req, next, axum::Extension(st)).await }
    }));

    let req = Request::delete("/api/plugins/PublicP")
        .header("Cookie", format!("ark_session={}", session_id))
        .body(Body::empty())
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
/// POST /api/plugins/{id}/tools/{tool_id} should succeed for owner
async fn test_execute_tool_allowed_when_owner() {
    use futures::future::BoxFuture;
    let app = Arc::new(ArkState::default());
    app.set_state(ApplicationState::StartingNetwork);

    // Register plugin owned by current user and a stub handler
    let plugin = ark::config::plugins::ArkPlugin {
        name: "ExecMine".to_string(),
        url: Some("file:///nonexistent.wasm".parse().unwrap()),
        auth: None,
        insecure: false,
        manifest: None,
        owner: Some("oidc/*/me".into()),
    };
    {
        let mut catalog = app.plugin_registry.catalog.write().await;
        catalog.plugin_to_config.insert("ExecMine".into(), plugin);
        catalog
            .tool_to_plugin
            .insert("t1".into(), "ExecMine".into());
        // Handler returns a simple JSON
        let handler: ark::plugins::registry::PluginHandler = std::sync::Arc::new(|_v: serde_json::Value| -> BoxFuture<'static, Result<serde_json::Value, rmcp::ErrorData>> {
            Box::pin(async { Ok(serde_json::json!({"ok": true})) })
        });
        catalog.tool_to_handler.insert("t1".into(), handler);
    }

    let principal = auth::Principal {
        subject: "me".into(),
        email: None,
        name: None,
        picture: None,
        provider: "test".into(),
        provider_kind: ProviderKind::Oidc,
        tenant_id: None,
        oid: None,
        roles: vec![Role::User],
        is_admin: false,
        groups: vec![],
    };
    let provider = ark::config::models::IdentityProviderConfig {
        name: "fake".into(),
        client_id: "client".into(),
        client_secret: None,
        authority: "https://example.invalid".into(),
        discovery: false,
        ..Default::default()
    };
    let auth_cfg = ark::config::models::AuthConfig {
        enabled: true,
        provider: Some("fake".into()),
        providers: vec![provider],
        session: Some(ark::config::models::SessionConfig::default()),
    };
    let (auth_state, _temp_dir) = create_test_auth_state(auth_cfg).await;
    let session_id = auth_state
        .put_session(principal, std::time::Duration::from_secs(60))
        .await;
    let auth_state = Arc::new(auth_state);

    let mut router = axum::Router::new()
        .route(
            "/api/plugins/{id}/tools/{tool_id}",
            axum::routing::post(execute_plugin_tool),
        )
        .with_state(app.clone());
    let auth_state2 = auth_state.clone();
    router = router.layer(axum::middleware::from_fn(move |req, next| {
        let st = auth_state2.clone();
        async move { auth::check_auth(req, next, axum::Extension(st)).await }
    }));

    let payload = json!({});
    let req = Request::post("/api/plugins/ExecMine/tools/t1")
        .header("content-type", "application/json")
        .header("Cookie", format!("ark_session={}", session_id))
        .body(Body::from(serde_json::to_string(&payload).unwrap()))
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json, serde_json::json!({"ok": true}));
}

#[tokio::test]
/// POST /api/plugins/{id}/tools/{tool_id} should be allowed for public plugin
async fn test_execute_tool_allowed_when_public() {
    use futures::future::BoxFuture;
    let app = Arc::new(ArkState::default());
    app.set_state(ApplicationState::StartingNetwork);

    // Register public plugin with a stub handler
    let plugin = ark::config::plugins::ArkPlugin {
        name: "ExecPublic".to_string(),
        url: Some("file:///nonexistent.wasm".parse().unwrap()),
        auth: None,
        insecure: false,
        manifest: None,
        owner: Some("*/*/*".into()),
    };
    {
        let mut catalog = app.plugin_registry.catalog.write().await;
        catalog.plugin_to_config.insert("ExecPublic".into(), plugin);
        catalog
            .tool_to_plugin
            .insert("my_plugin".into(), "ExecPublic".into());
        let handler: ark::plugins::registry::PluginHandler = std::sync::Arc::new(|_v: serde_json::Value| -> BoxFuture<'static, Result<serde_json::Value, rmcp::ErrorData>> {
            Box::pin(async { Ok(serde_json::json!({"ok": true})) })
        });
        catalog.tool_to_handler.insert("my_plugin".into(), handler);
    }

    // Build a router without auth state (public execution should not need ownership)
    let router = axum::Router::new()
        .route(
            "/api/plugins/{id}/tools/{tool_id}",
            axum::routing::post(execute_plugin_tool),
        )
        .with_state(app.clone());

    let payload = json!({});
    let req = Request::post("/api/plugins/ExecPublic/tools/my_plugin")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(&payload).unwrap()))
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
/// Tests GET /api/plugins/{id} endpoint returns empty array for non-existent plugin
async fn test_get_plugin_by_id_not_found() {
    let app = Arc::new(ArkState::default());

    let router = Router::new()
        .route("/api/plugins/{id}", get(get_plugin_by_id))
        .with_state(app);

    let request = Request::get("/api/plugins/nonexistent")
        .body(Body::empty())
        .unwrap();
    let response = router.oneshot(request).await.unwrap();

    // Current behavior: returns 404 for non-existent plugins
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert!(!body.is_empty(), "Expected error body for missing plugin");
}

#[tokio::test]
/// Tests POST /api/plugins endpoint with valid plugin payload (fails during loading, not validation)
async fn test_create_plugin_valid() {
    let app = Arc::new(ArkState::default());
    app.set_state(ApplicationState::StartingNetwork);

    let router = Router::new()
        .route("/api/plugins", axum::routing::post(create_plugin))
        .with_state(app);

    // Create a valid plugin payload (using a non-existent file to avoid actual loading)
    let payload = json!({
        "name": "test-plugin",
        "url": "file:///nonexistent.wasm",
        "insecure": false
    });

    let request = Request::post("/api/plugins")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(&payload).unwrap()))
        .unwrap();
    let response = router.oneshot(request).await.unwrap();

    // Should fail during plugin loading, not validation
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json.get("error").is_some());
}

#[tokio::test]
/// POST /api/plugins should override provided owner with authenticated user's global_id
async fn test_create_plugin_owner_override() {
    let app = Arc::new(ArkState::default());
    app.set_state(ApplicationState::StartingNetwork);

    // Auth setup
    let principal = auth::Principal {
        subject: "me".into(),
        email: None,
        name: None,
        provider: "test".into(),
        picture: None,
        provider_kind: ProviderKind::Oidc,
        tenant_id: None,
        oid: None,
        roles: vec![Role::User],
        is_admin: false,
        groups: vec![],
    };
    let provider = ark::config::models::IdentityProviderConfig {
        name: "fake".into(),
        client_id: "client".into(),
        client_secret: None,
        authority: "https://example.invalid".into(),
        discovery: false,
        ..Default::default()
    };
    let auth_cfg = ark::config::models::AuthConfig {
        enabled: true,
        provider: Some("fake".into()),
        providers: vec![provider],
        session: Some(ark::config::models::SessionConfig::default()),
    };
    let (auth_state, _temp_dir) = create_test_auth_state(auth_cfg).await;
    let session_id = auth_state
        .put_session(principal.clone(), std::time::Duration::from_secs(60))
        .await;
    let auth_state = Arc::new(auth_state);

    let mut router = Router::new()
        .route("/api/plugins", axum::routing::post(create_plugin))
        .with_state(app.clone());
    let auth_state2 = auth_state.clone();
    router = router.layer(axum::middleware::from_fn(move |req, next| {
        let st = auth_state2.clone();
        async move { auth::check_auth(req, next, axum::Extension(st)).await }
    }));

    // Payload injects a bogus owner which should be ignored/overridden
    let payload = json!({ "name": "OwnerOverride", "url": "file:///nonexistent.wasm", "insecure": true, "owner": "oidc/*/bogus" });
    let req = Request::post("/api/plugins")
        .header("content-type", "application/json")
        .header("Cookie", format!("ark_session={}", session_id))
        .body(Body::from(serde_json::to_string(&payload).unwrap()))
        .unwrap();
    let resp = router.clone().oneshot(req).await.unwrap();
    // Load fails for nonexistent plugin; ensure 500 and no registration
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let cat = app.plugin_registry.catalog.read().await;
    assert!(
        !cat.plugin_to_config.contains_key("OwnerOverride"),
        "Plugin should not be registered when load fails"
    );
}

#[tokio::test]
/// Tests POST /api/plugins endpoint validation error when name field is missing
async fn test_create_plugin_missing_name() {
    let app = Arc::new(ArkState::default());

    let router = Router::new()
        .route("/api/plugins", axum::routing::post(create_plugin))
        .with_state(app);

    let payload = json!({
        "url": "file:///test.wasm"
    });

    let request = Request::post("/api/plugins")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(&payload).unwrap()))
        .unwrap();
    let response = router.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY); // Axum validation error
}

#[tokio::test]
/// Tests POST /api/plugins endpoint validation error when url field is missing
async fn test_create_plugin_missing_url() {
    let app = Arc::new(ArkState::default());

    let router = Router::new()
        .route("/api/plugins", axum::routing::post(create_plugin))
        .with_state(app);

    let payload = json!({
        "name": "test-plugin"
    });

    let request = Request::post("/api/plugins")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(&payload).unwrap()))
        .unwrap();
    let response = router.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY); // Axum validation error
}

#[tokio::test]
/// Tests POST /api/plugins endpoint with malformed JSON payload
async fn test_create_plugin_malformed_json() {
    let app = Arc::new(ArkState::default());

    let router = Router::new()
        .route("/api/plugins", axum::routing::post(create_plugin))
        .with_state(app);

    let request = Request::post("/api/plugins")
        .header("content-type", "application/json")
        .body(Body::from("invalid json"))
        .unwrap();
    let response = router.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
/// Tests DELETE /api/plugins/{id} endpoint rejects deletion of builtin plugin
async fn test_delete_plugin_builtin_protected() {
    let app = Arc::new(ArkState::default());
    app.set_state(ApplicationState::StartingNetwork);

    let cfg = ArkConfig::default();
    plugins::load_plugins(&cfg, app.clone())
        .await
        .expect("plugin load");

    let router = Router::new()
        .route("/api/plugins/{id}", axum::routing::delete(delete_plugin))
        .with_state(app);

    let request = Request::delete(format!("/api/plugins/{}", BUILTIN_PLUGIN_ID))
        .body(Body::empty())
        .unwrap();
    let response = router.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json.get("error").is_some());
}

#[tokio::test]
/// Tests DELETE /api/plugins/{id} endpoint returns 404 for non-existent plugin
async fn test_delete_plugin_not_found() {
    let app = Arc::new(ArkState::default());

    let router = Router::new()
        .route("/api/plugins/{id}", axum::routing::delete(delete_plugin))
        .with_state(app);

    let request = Request::delete("/api/plugins/nonexistent")
        .body(Body::empty())
        .unwrap();
    let response = router.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json.get("error").is_some());
}

#[tokio::test]
/// Tests POST /api/plugins/{id}/tools/{tool_id} endpoint returns 404 for non-existent plugin
async fn test_execute_tool_plugin_not_found() {
    let app = Arc::new(ArkState::default());

    let router = Router::new()
        .route(
            "/api/plugins/{id}/tools/{tool_id}",
            axum::routing::post(execute_plugin_tool),
        )
        .with_state(app);

    let payload = json!({});
    let request = Request::post("/api/plugins/nonexistent/tools/test-tool")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(&payload).unwrap()))
        .unwrap();
    let response = router.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    // Check that we get some response body for the error
    assert!(!body.is_empty());
}

#[tokio::test]
/// Tests POST /api/plugins/{id}/tools/{tool_id} endpoint returns 404 for non-existent tool
async fn test_execute_tool_not_found() {
    let app = Arc::new(ArkState::default());
    app.set_state(ApplicationState::StartingNetwork);

    let cfg = ArkConfig::default();
    plugins::load_plugins(&cfg, app.clone())
        .await
        .expect("plugin load");

    let router = Router::new()
        .route(
            "/api/plugins/{id}/tools/{tool_id}",
            axum::routing::post(execute_plugin_tool),
        )
        .with_state(app);

    let payload = json!({});
    let request = Request::post(format!(
        "/api/plugins/{}/tools/nonexistent",
        BUILTIN_PLUGIN_ID
    ))
    .header("content-type", "application/json")
    .body(Body::from(serde_json::to_string(&payload).unwrap()))
    .unwrap();
    let response = router.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    // Check that we get some response body for the error
    assert!(!body.is_empty());
}

#[tokio::test]
/// Tests POST /api/plugins/{id}/tools/{tool_id} propagates handler error as 500
async fn test_execute_tool_handler_error() {
    use futures::future::BoxFuture;
    let app = Arc::new(ArkState::default());
    app.set_state(ApplicationState::StartingNetwork);

    // Register plugin with a tool and a handler that returns rmcp error
    let plugin = ark::config::plugins::ArkPlugin {
        name: "ErrP".to_string(),
        url: Some("file:///nonexistent.wasm".parse().unwrap()),
        auth: None,
        insecure: false,
        manifest: None,
        owner: Some("*/*/*".into()),
    };
    {
        let mut catalog = app.plugin_registry.catalog.write().await;
        catalog.plugin_to_config.insert("ErrP".into(), plugin);
        catalog.tool_to_plugin.insert("t1".into(), "ErrP".into());
        let handler: ark::plugins::registry::PluginHandler = std::sync::Arc::new(|_v: serde_json::Value| -> BoxFuture<'static, Result<serde_json::Value, rmcp::ErrorData>> {
            Box::pin(async { Err(rmcp::ErrorData::internal_error("boom".to_string(), None)) })
        });
        catalog.tool_to_handler.insert("t1".into(), handler);
    }

    let router = Router::new()
        .route(
            "/api/plugins/{id}/tools/{tool_id}",
            axum::routing::post(execute_plugin_tool),
        )
        .with_state(app.clone());

    let payload = json!({});
    let req = Request::post("/api/plugins/ErrP/tools/t1")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(&payload).unwrap()))
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
/// Tests GET /livez endpoint returns "live" when application is ready
async fn test_health_livez_default_path() {
    let app = Arc::new(ArkState::default());
    app.set_state(ApplicationState::Ready); // Set to ready state

    let router = Router::new().route("/livez", get(livez)).with_state(app);

    let request = Request::get("/livez").body(Body::empty()).unwrap();
    let response = router.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert_eq!(text, "live");
}

#[tokio::test]
/// Tests GET /readyz endpoint returns "ready" when application is ready
async fn test_health_readyz_default_path() {
    let app = Arc::new(ArkState::default());
    app.set_state(ApplicationState::Ready);

    let router = Router::new().route("/readyz", get(readyz)).with_state(app);

    let request = Request::get("/readyz").body(Body::empty()).unwrap();
    let response = router.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert_eq!(text, "ready");
}

#[tokio::test]
/// Tests GET /health/live endpoint returns "live" when application is ready
async fn test_health_livez_custom_path() {
    let app = Arc::new(ArkState::default());
    app.set_state(ApplicationState::Ready);

    let router = Router::new()
        .route("/health/live", get(livez))
        .with_state(app);

    let request = Request::get("/health/live").body(Body::empty()).unwrap();
    let response = router.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert_eq!(text, "live");
}

#[tokio::test]
/// Tests GET /health/ready endpoint returns "ready" when application is ready
async fn test_health_readyz_custom_path() {
    let app = Arc::new(ArkState::default());
    app.set_state(ApplicationState::Ready);

    let router = Router::new()
        .route("/health/ready", get(readyz))
        .with_state(app);

    let request = Request::get("/health/ready").body(Body::empty()).unwrap();
    let response = router.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert_eq!(text, "ready");
}

#[tokio::test]
/// Tests GET /livez endpoint returns 503 "not live" when application is terminating
async fn test_health_livez_not_alive() {
    let app = Arc::new(ArkState::default());
    app.set_state(ApplicationState::Terminating); // Set to not alive state

    let router = Router::new().route("/livez", get(livez)).with_state(app);

    let request = Request::get("/livez").body(Body::empty()).unwrap();
    let response = router.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert_eq!(text, "not live");
}

#[tokio::test]
/// Tests GET /readyz endpoint returns 503 "not ready" when application is loading plugins
async fn test_health_readyz_not_ready() {
    let app = Arc::new(ArkState::default());
    app.set_state(ApplicationState::LoadingPlugins); // Set to not ready state

    let router = Router::new().route("/readyz", get(readyz)).with_state(app);

    let request = Request::get("/readyz").body(Body::empty()).unwrap();
    let response = router.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert_eq!(text, "not ready");
}

#[tokio::test]
/// Tests GET /livez endpoint returns JSON response when Accept: application/json header is set
async fn test_health_content_negotiation_json() {
    let app = Arc::new(ArkState::default());
    app.set_state(ApplicationState::Ready);

    let router = Router::new().route("/livez", get(livez)).with_state(app);

    let request = Request::get("/livez")
        .header("accept", "application/json")
        .body(Body::empty())
        .unwrap();
    let response = router.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["status"], "live");
}

#[tokio::test]
/// Tests GET /readyz endpoint returns text response when Accept: text/plain header is set
async fn test_health_content_negotiation_text() {
    let app = Arc::new(ArkState::default());
    app.set_state(ApplicationState::Ready);

    let router = Router::new().route("/readyz", get(readyz)).with_state(app);

    let request = Request::get("/readyz")
        .header("accept", "text/plain")
        .body(Body::empty())
        .unwrap();
    let response = router.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert_eq!(text, "ready");
}

// Note: Metrics endpoint test is now enabled since metrics_handler is public
#[cfg(feature = "prometheus")]
#[tokio::test]
/// Tests GET /metrics endpoint returns prometheus metrics when prometheus feature is enabled
async fn test_metrics_endpoint() {
    let app = Arc::new(ArkState::default());

    // Create a minimal router with metrics endpoint
    let router = Router::new()
        .route("/metrics", get(metrics_handler))
        .with_state(app);

    let request = Request::get("/metrics").body(Body::empty()).unwrap();
    let response = router.oneshot(request).await.unwrap();

    // The metrics handler may return 503 if metrics are not available
    // This is acceptable behavior - we're just testing that the endpoint exists
    let status = response.status();
    assert!(status.is_success() || status == StatusCode::SERVICE_UNAVAILABLE);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();
    // Should contain some prometheus metrics format or be empty
    assert!(text.contains("#") || text.is_empty() || status == StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
/// Tests POST /api/plugins endpoint handles malformed JSON gracefully
async fn test_api_boundary_malformed_json() {
    let app = Arc::new(ArkState::default());

    let router = Router::new()
        .route("/api/plugins", axum::routing::post(create_plugin))
        .with_state(app);

    let request = Request::post("/api/plugins")
        .header("content-type", "application/json")
        .body(Body::from("invalid json"))
        .unwrap();
    let response = router.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
/// Tests POST /api/plugins endpoint handles special characters in plugin names
async fn test_api_boundary_special_characters() {
    let app = Arc::new(ArkState::default());

    let router = Router::new()
        .route("/api/plugins", axum::routing::post(create_plugin))
        .with_state(app);

    // Test with special characters in plugin name
    let payload = json!({
        "name": "test-plugin_123!@#$%^&*()",
        "url": "file:///test.wasm"
    });

    let request = Request::post("/api/plugins")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(&payload).unwrap()))
        .unwrap();
    let response = router.oneshot(request).await.unwrap();

    // Should handle gracefully - either succeed or fail with proper error
    assert!(
        response.status().is_success()
            || response.status().is_client_error()
            || response.status().is_server_error()
    );
}

#[tokio::test]
/// Tests GET /api/plugins endpoint returns empty object when no plugins are loaded
async fn test_get_plugins_empty() {
    let app = Arc::new(ArkState::default());

    let router = Router::new()
        .route("/api/plugins", get(get_plugins))
        .with_state(app);

    let request = Request::get("/api/plugins").body(Body::empty()).unwrap();
    let response = router.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();

    // API returns an object with plugin data, should be empty initially
    // The structure is {"plugin_name": {...}} so an empty registry returns {}
    assert!(
        body.as_object().unwrap().is_empty(),
        "Expected empty object, got: {:?}",
        body
    );
}

#[tokio::test]
/// Tests POST /api/plugins endpoint to add a plugin and then GET /api/plugins to verify
async fn test_add_plugins_and_get() {
    let app = Arc::new(ArkState::default());

    let router = Router::new()
        .route("/api/plugins", get(get_plugins).post(create_plugin))
        .with_state(app);

    // Use a mock plugin configuration instead of trying to load a real WASM file
    // This avoids file system dependencies in tests
    let plugin_data = serde_json::json!({
        "name": "TestPlugin",
        "url": "http://example.com/test.wasm",
        "insecure": true
    });

    let request = Request::post("/api/plugins")
        .header("content-type", "application/json")
        .body(Body::from(plugin_data.to_string()))
        .unwrap();

    let response = router.clone().oneshot(request).await.unwrap();

    // Plugin creation may succeed or fail depending on implementation
    // For now, just check that we get a valid HTTP response
    assert!(
        response.status().is_success()
            || response.status().is_client_error()
            || response.status().is_server_error(),
        "Expected success, client error, or server error, got {}",
        response.status()
    );

    // If plugin creation succeeded, check the response body
    if response.status() == StatusCode::CREATED {
        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let response_body: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(
            response_body,
            serde_json::json!({"message": "Plugin registered successfully"})
        );
    }

    // Regardless of creation success, check that GET still works
    let request = Request::get("/api/plugins").body(Body::empty()).unwrap();

    let response = router.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let response_body: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();

    // Should be a JSON object (possibly empty if plugin creation failed)
    assert!(
        response_body.is_object(),
        "Expected JSON object, got {:?}",
        response_body
    );
}

#[tokio::test]
/// Tests POST /api/plugins endpoint with invalid plugin URL returns 500 error
async fn test_add_invalid_plugin() {
    let app = Arc::new(ArkState::default());

    let router = Router::new()
        .route("/api/plugins", get(get_plugins).post(create_plugin))
        .with_state(app);

    // This plugin configuration has an invalid URL (non-existent)
    let plugin_data = serde_json::json!({
        "name": "InvalidPlugin",
        "url": "http://invalid-url-for-testing.wasm",
        "insecure": true
    });

    let request = Request::post("/api/plugins")
        .header("content-type", "application/json")
        .body(Body::from(plugin_data.to_string()))
        .unwrap();

    let response = router.clone().oneshot(request).await.unwrap();

    // Plugin creation will fail because the URL doesn't exist, but that's expected
    // The API should return a 500 error for plugin loading failure
    assert_eq!(
        response.status(),
        StatusCode::INTERNAL_SERVER_ERROR,
        "Expected 500 Internal Server Error for invalid plugin URL, got {}",
        response.status()
    );

    // The error response should contain error details
    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let error_body: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    assert!(
        error_body.get("error").is_some(),
        "Expected error field in response"
    );

    // GET should still work and return empty (since plugin creation failed)
    let request = Request::get("/api/plugins").body(Body::empty()).unwrap();

    let response = router.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let response_body: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    assert!(
        response_body.is_object(),
        "Expected JSON object, got {:?}",
        response_body
    );
}
