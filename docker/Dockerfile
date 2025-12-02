FROM rust:1.91.1 AS orion-builder

#RUN rustup component add rustfmt

WORKDIR /tmp

RUN <<EOF
apt update
apt install -y protobuf-compiler git ca-certificates
EOF

RUN git clone https://github.com/kmesh-net/orion

WORKDIR /tmp/orion

RUN git submodule init
RUN git submodule update --force

RUN cargo build --release

# ### Split into two files; one to build and one to actually run it
# ###https://docs.docker.com/develop/develop-images/multistage-build/

FROM debian:trixie-slim
RUN <<EOF
apt update
apt upgrade -y
EOF
COPY --from=orion-builder /tmp/orion/target/release/orion /orion
ENTRYPOINT ["/orion"]
