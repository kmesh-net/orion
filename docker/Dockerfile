# Stage 1: The Builder
FROM rust:1.84 AS orion-builder

# install build-time dependencies for the Rust application.
RUN apt-get update && \
    apt-get install -y protobuf-compiler && \
    rm -rf /var/lib/apt/lists/*

# set the working directory inside the container.
WORKDIR /tmp/orion

# copy the dependency manifests first to leverage Docker's layer caching.
COPY Cargo.toml Cargo.lock ./

# create a dummy main.rs file and workspace members. This is a common pattern
RUN mkdir -p src && echo 'fn main() {}' > src/main.rs

RUN set -e; \
    for member in \
    orion-lib \
    orion-xds \
    orion-proxy \
    orion-error \
    orion-configuration \
    orion-data-plane-api \
    envoy-data-plane-api; \
    do \
    mkdir -p "${member}/src"; \
    printf '[package]\nname = "%s"\nversion = "0.1.0"\nedition = "2021"\n\n[lib]\npath = "src/lib.rs"\n' \
    "${member}" > "${member}/Cargo.toml"; \
    touch "${member}/src/lib.rs"; \
    done

# fetch and download all dependencies defined in Cargo.lock.
RUN cargo fetch

# copy the rest of the application source code.
COPY . .

# build the application in release mode for performance.
RUN cargo build --release

# Stage 2: The Final Runtime Image
FROM debian:bookworm-slim

# install the root CA certificates package.
RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates && \
    rm -rf /var/lib/apt/lists/*

# copy the runtime configuration file into the final image.
COPY ./orion-proxy/conf/orion-runtime.yaml /etc/orion/

# copy the startup script and make it executable.
COPY --chmod=+x ./docker/start_proxy.sh /start_proxy.sh

# copy only the compiled application binary from the builder stage.
COPY --from=orion-builder /tmp/orion/target/release/orion /orion

# expose the ports the application listens on.
EXPOSE 8080 8000 50051

# set the command that will be run when the container starts.
ENTRYPOINT ["/start_proxy.sh"]
