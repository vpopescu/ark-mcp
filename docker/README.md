This docker image defines two targets:

- run target  - image without TLS
- run-prod target - image with TLS, using sample certs

When running the TLS version, you will either need to add your own certs in the assets/ folder, or install the root certs (also in assets folder). This is because the /mcp endpoints require trusted SSL certs. 

Currently none of these targets enable authentication, since that requires configuration on the Azure and/or Google side. However they can be added to the config file when the docker container is started (see wiki).


to build:

```
docker build -t ark-mcp:latest -f Dockerfile --target [targetname]..
```
Where [targetname] is either 'run' or 'run-prod'


to run:

```
ocker run -e 'RUST_LOG=ark=debug' --rm -p 8000:8000 -p 3001:3001   --mount type=bind,source=[configfile],target=/etc/ark.config.yaml,readonly --mount type=bind,source=./.temp,target=/var/ark  ark-mcp:latest


```

where [configfile] is one of:

- "./config.non-tls.yaml" for the 'run' target
- "./config.tls.yaml" for the 'run-prod' target

This will also keep the database on the host (in the .temp folder inside the scripts folder) which makes for easier testing.


