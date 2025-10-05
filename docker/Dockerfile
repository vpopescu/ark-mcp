# syntax=docker/dockerfile:1.6

# example: 
#    docker build -t ark-mcp:latest -f Dockerfile ..   
#    docker run --rm -p 8000:8000 -p 3001:3001   --mount type=bind,source="./config.yaml",target=/etc/ark.config.yaml,readonly  ark-mcp:latest


FROM rust:1.89-alpine3.20 AS builder-rust
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

# just for debugging
#RUN apk add --no-cache file binutils gdb

# service
COPY --from=builder-rust /build/target/release/ark .
#COPY --from=builder-rust /artifacts/debug/ark.debug ./ark.debug

# console
COPY --from=builder-node /build/dist ./www/dist

# config mount point
#VOLUME ["/etc/ark.config.yaml"]

# Expose any necessary ports (defaults)
EXPOSE 8000
EXPOSE 3001

ENTRYPOINT ["/home/bin/ark"]
CMD ["--transport", "streamable-http", "--config-file", "/etc/ark.config.yaml"]

