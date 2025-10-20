-- V001: initial schema for ark MCP (sessions + plugins)
-- Generated from inline bootstrap SQL

CREATE TABLE IF NOT EXISTS sessions (
    session_id TEXT PRIMARY KEY,
    principal_json TEXT NOT NULL,
    expiry_utc TEXT NOT NULL,
    expiry_epoch INTEGER NOT NULL,
    is_admin INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_sessions_expiry ON sessions(expiry_epoch);
CREATE INDEX IF NOT EXISTS idx_sessions_is_admin ON sessions(is_admin);

CREATE TABLE IF NOT EXISTS plugins (
    owner TEXT NOT NULL,
    plugin_id TEXT NOT NULL,
    plugin_name TEXT,
    plugin_path TEXT,
    plugin_data BLOB,
    metadata TEXT NOT NULL,
    date_added_utc TEXT NOT NULL,
    PRIMARY KEY (owner, plugin_id)
);
CREATE INDEX IF NOT EXISTS idx_plugins_owner ON plugins(owner);
CREATE INDEX IF NOT EXISTS idx_plugins_date_added ON plugins(date_added_utc);


