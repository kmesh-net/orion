FROM rust:1.88 as builder

RUN apt-get update && apt-get install -y protobuf-compiler

WORKDIR /usr/src/orion


COPY . .


RUN cargo build --release

# Stage 2:
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy binary from builder
COPY --from=builder /usr/src/orion/target/release/orion /app/orion

# Copy runtime config
COPY orion-proxy/conf/orion-runtime.yaml /app/orion-runtime.yaml

EXPOSE 8000 8001

CMD ["./orion", "--config", "/app/orion-runtime.yaml"]