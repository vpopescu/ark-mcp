Migrations
==========

This directory contains SQL migrations used by the ark MCP server.

Conventions
-----------
- Migrations are organized by backend under `migrations/<backend>/` (e.g. `migrations/sqlite/`).
- File names use the `V{number}__short_description.sql` pattern. Numbers are zero-padded to 3 digits.
- Keep migrations small and atomic. Do not modify an already-committed migration that has been applied in production.

Developer workflow
------------------
1. Create a migration using the `xtask` helper:

   cargo run --manifest-path xtask/Cargo.toml -- new-migration "add_plugin_index" --backend sqlite

2. Edit the generated SQL file to perform the necessary schema changes.
3. Commit the migration alongside the code changes that depend on it.

Runtime behavior
----------------
- By default the binary applies embedded migrations that were compiled into the binary using `refinery`.
- Operators can override automatic application using env:
  - `ARK_AUTO_APPLY_MIGRATIONS=false` to skip auto-apply on startup
  - `ARK_MIGRATIONS_DIR=/path/to/sql` to apply SQL files from the filesystem instead of the embedded set

Filesystem-mode (refinery-backed)
---------------------------------
When `ARK_MIGRATIONS_DIR` is set the server now uses `refinery::load_sql_migrations()`
to discover and parse SQL migration files and then executes them via `refinery::Runner`.
This means filesystem-mode is tracked in the same way as embedded migrations (applied-once,
history recorded in the `refinery_schema_history` table). The runner is configured to
abort on divergent or missing migrations by default to avoid silent drift.

Packaging
---------
- The compiled binary contains the embedded migration set and will apply them automatically unless the operator overrides behaviour.
- It is recommended to also COPY the `migrations/` tree into your ops/container image so operators can inspect or manually run migrations.
