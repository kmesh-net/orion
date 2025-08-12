FROM rust:1.84 AS orion-builder

#RUN rustup component add rustfmt

WORKDIR /tmp/orion

RUN <<EOF
apt update
apt install -y protobuf-compiler
EOF


COPY ./orion-xds ./orion-xds
COPY ./orion-proxy ./orion-proxy
COPY ./orion-data-plane-api ./orion-data-plane-api
COPY ./orion-error ./orion-error
COPY ./orion-lib ./orion-lib
COPY ./envoy-data-plane-api ./envoy-data-plane-api
COPY ./orion-configuration ./orion-configuration


COPY rustfmt.toml ./
COPY Cargo.toml ./
COPY Cargo.lock ./


RUN cargo build --release

### Split into two files; one to build and one to actually run it
###https://docs.docker.com/develop/develop-images/multistage-build/

FROM debian:bookworm-slim
RUN <<EOF
apt update
apt upgrade -y
apt install -y ca-certificates libssl3
EOF

COPY ./docker/start_proxy.sh ./start_proxy.sh

COPY --from=orion-builder /tmp/orion/target/release/orion /orion
EXPOSE 8080 8000 50051
ENTRYPOINT ["./orion"]
