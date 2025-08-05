FROM rust:1.84 AS orion-builder

RUN apt-get update && \
    apt-get install -y protobuf-compiler && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /tmp/orion

COPY Cargo.toml Cargo.lock ./

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

RUN cargo fetch

COPY . .

RUN cargo build --release

FROM debian:bookworm-slim

RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates && \
    rm -rf /var/lib/apt/lists/*

COPY ./orion-proxy/conf/orion-runtime.yaml /etc/orion/
COPY --chmod=+x ./docker/start_proxy.sh /start_proxy.sh
COPY --from=orion-builder /tmp/orion/target/release/orion /orion

ENTRYPOINT ["/start_proxy.sh"]
