// user agent used for outgoing HTTP(s) requests for example to OCI registry
pub const REQUEST_USER_AGENT: &str = "ark-mcp/1";

// default bind address for MCP server
pub const DEFAULT_MCP_BIND_ADDRESS: &str = "127.0.0.1:3001";

// default bind address for API/management server
pub const DEFAULT_MGMT_BIND_ADDRESS: &str = "127.0.0.1:8000";

// constants used to built the MCP ServerInfo
pub const MCP_SERVER_INFO_NAME: &str = "ArkMCP";
pub const MCP_SERVER_INFO_TITLE: &str = "Ark MCP Server";
pub const MCP_SERVER_INFO_VERSION: &str = "0.1-alpha";
pub const MCP_SERVER_INFO_URL: &str = "https://github.com/vpopescu/ark-mcp";
