FROM rust:1.86 AS orion-builder

RUN apt-get update && \
    apt-get install -y protobuf-compiler && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /tmp/orion

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
