# syntax=docker/dockerfile:1.6
# example, to build with tls:
#
#    docker build -t ark-mcp:latest --target run-prod -f Dockerfile ..
#
# or without:
#    docker build -t ark-mcp:latest --target run -f Dockerfile ..
#
# to run without tls: 
#    docker run --rm -p 8000:8000 -p 3001:3001   --mount type=bind,source="./config.yaml",target=/etc/ark.config.tls.yaml,readonly  ark-mcp:latest
# to run with tls:
#    docker run --rm -p 8000:8000 -p 3001:3001   --mount type=bind,source="./config.non-tls.yaml",target=/etc/ark.config.tls.yaml,readonly  ark-mcp:latest


FROM rust:1.90-alpine3.20 AS builder-rust
WORKDIR /build
RUN apk add --no-cache  pkgconfig sccache build-base git python3 
ENV RUSTC_WRAPPER=/usr/bin/sccache \
    SCCACHE_DIR=/cache/sccache \
    RUST_BACKTRACE=1 \
    CARGO_NET_GIT_FETCH_WITH_CLI=true

# Show toolchain versions for diagnostics
RUN rustc -V && cargo -V


COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY assets ./assets
COPY migrations ./migrations

# service
WORKDIR /build
RUN --mount=type=cache,id=cargo-registry,target=/usr/local/cargo/registry \
    --mount=type=cache,id=cargo-git,target=/usr/local/cargo/git \
    --mount=type=cache,id=sccache,target=/cache/sccache \
    cargo build --release --locked --features="prometheus"

# prepare a production build
RUN apk add --no-cache binutils \
    && objcopy --only-keep-debug /build/target/release/ark /build/target/release/ark.debug \
    && strip --strip-all /build/target/release/ark \
    && objcopy --add-gnu-debuglink=/build/target/release/ark.debug /build/target/release/ark \
    && mkdir -p /artifacts/debug && mv /build/target/release/ark.debug /artifacts/debug/ark.debug \
    && apk del binutils    


# ----------- Build admin console -----------
FROM node:22-alpine3.20 AS builder-node
WORKDIR /build

# Copy only the web app directory from default context
COPY www/package*.json ./
RUN --mount=type=cache,id=npm-cache,target=/root/.npm \
    npm ci || npm install
COPY www .
# Show toolchain versions for diagnostics
RUN node -v && npm -v
RUN --mount=type=cache,id=npm-cache,target=/root/.npm \
    NODE_ENV=production npm run build

# ----------- Final image -----------
FROM alpine:3.20 AS run
WORKDIR /home/bin
# Install minimal runtime dependencies

RUN apk add --no-cache ca-certificates 


# service
COPY --from=builder-rust /build/target/release/ark .

# console
COPY --from=builder-node /build/dist ./www/dist


# Expose any necessary ports (defaults)
EXPOSE 8000
EXPOSE 3001

ENTRYPOINT ["/home/bin/ark"]
CMD ["--transport", "streamable-http", "--config-file", "/etc/ark.config.yaml"]


FROM alpine:3.20 AS run-prod
WORKDIR /home/bin
RUN apk add --no-cache ca-certificates

COPY --from=builder-rust /build/target/release/ark .
COPY --from=builder-node /build/dist ./www/dist
COPY --from=builder-rust /build/assets/dev_server.pem /etc/ark-certs/dev_server.pem
COPY --from=builder-rust /build/assets/dev_server.key /etc/ark-certs/dev_server.key


COPY migrations /etc/ark/migrations
RUN adduser -D -u 1000 ark && chown -R ark:ark /etc/ark/migrations
USER ark
ENV ARK_MIGRATIONS_DIR=/etc/ark/migrations
ENV ARK_AUTO_APPLY_MIGRATIONS=true

EXPOSE 8000
EXPOSE 3001

ENTRYPOINT ["/home/bin/ark"]
CMD ["--transport", "streamable-http", "--config-file", "/etc/ark.config.yaml"]

