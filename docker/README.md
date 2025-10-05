to build:

```
docker build -t ark-mcp:latest -f Dockerfile ..
```

to run:

```
docker run --rm -p 8000:8000 -p 3001:3001   --mount type=bind,source="./config.json",target=/etc/ark.config.json,readonly  ark-mcp:latest
```
