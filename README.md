# Ark MCP Server

An MCP server which exposes WebAssembly plugins as MCP tools.
Inspired by hyper-mcp project, with enterprise features.

## Documentation

The documentation is slowly being added to the wiki section. Some of it is generated using AI, so it may contain some hallucinations.

## Plugins

Sample plugins:

| Name | Language | Repository |
| ---- | -------- | ---------- |
| hash | Rust  | https://github.com/vpopescu/ark-mcp-plugin-hash |
| time | Rust  | https://github.com/vpopescu/ark-mcp-plugin-time |
| TBD | C++ | TBD |
| TBD | C# | TBD |





## Quick start

The easiest way to try it out is to run the docker container on linux or Windows WSL. From root of the repository:

```bash
docker build -t ark-tryout -f docker/Dockerfile .
docker run --rm -p 8000:8000 -p 3001:3001 --mount type=bind,source="$(pwd)/docker/config.yaml",target=/etc/ark.config.yaml,readonly  ark-tryout
```

### Logging

To see debug logs, set the RUST_LOG variable:

```bash
export RUST_LOG=ark=debug
ark ...
```

or

```pwsh
$ENV:RUST_LOG="ark=debug"
ark ...
```

### Token signing (ID tokens)

The server can sign ID tokens (JWTs) when it acts as an authorization server. Configure token signing in `config.yaml` using the `token_signing` block.

Example `token_signing` (local PEM):

```yaml
token_signing:
	source: local
	key: assets/dev_server.key        # private key path (can be overridden via env)
	cert: assets/dev_server.pem      # optional public cert (can be overridden via env)
```

Environment variable overrides:

- `ARK_TOKEN_SIGNING_KEY` — overrides the `token_signing.key` path
- `ARK_TOKEN_SIGNING_CERT` — overrides the `token_signing.cert` path

When enabled, the server exposes `/.well-known/jwks.json` with the public key so
clients and libraries can validate issued ID tokens.


### Using node-based clients

Using tools such as @modelcontextprotocol/inspector or other node-based tool, you will probably need to turn off strict certificate checking if
using self signed certificates:

```bash
export NODE_TLS_REJECT_UNAUTHORIZED=0 npx @modelcontextprotocol/inspector
```

or (windows)

```pwsh
$ENV:NODE_TLS_REJECT_UNAUTHORIZED="0"
npx @modelcontextprotocol/inspector
```



