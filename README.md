# Ark MCP Server

An MCP server which exposes WebAssembly plugins as MCP tools.
Inspired by hyper-mcp project.


## Quick start

The easiest way to try it out is to run the docker container on linux r Windows WSL. From root of the repository:

```bash
docker build -t ark-tryout -f docker/Dockerfile .
docker run --rm -p 8000:8000 -p 3001:3001 --mount type=bind,source="$(pwd)/docker/config.yaml",target=/etc/ark.config.yaml,readonly  ark-tryout
```

Note that if you enable https, you will need to update the cors settings in the config file.


## Notes

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


