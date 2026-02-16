<img src="docs/pics/logo/orion_proxy_logo.png" alt="orion-proxy-logo" style="zoom: 100%;" />

<!--
[![LICENSE](https://img.shields.io/github/license/kmesh-net/orion)](/LICENSE) [![codecov](https://codecov.io/gh/kmesh-net/kmesh/graph/badge.svg?token=0EGQ84FGDU)](https://img.shields.io/github/license/kmesh-net/orion) [![FOSSA Status](https://app.fossa.com/api/projects/git%2Bgithub.com%2Fkmesh-net%2Forion.svg?type=shield)](https://app.fossa.com/projects/git%2Bgithub.com%2Fkmesh-net%2Forion?ref=badge_shield)
-->

## Introduction

Orion Proxy is a high performance and memory safe implementation of popular [Envoy Proxy](https://www.envoyproxy.io/). Orion Proxy is implemented in Rust using high-quality open source components.

### Key Features

**Memory Safety**

Rust programming language allows to avoid a whole lot of bugs related to memory management and data races making Orion Proxy a very robust and secure application.

**Performance**

Orion Proxy offers 2x-4x better throughput and latency than Envoy Proxy. Refer to [Performance](docs/performance/performance.md) to see performance figures and for more details how we tested Orion Proxy.

<table>
  <tr>
    <th colspan="2" align="center">HTTP Benchmark Results</th>
  </tr>
  <tr>
    <td width="50%"><img src="docs/pics/performance/baseline/wrk-reqsec.png" alt="HTTP Requests per Second"/><br/><p align="center"><b>Requests per Second</b></p></td>
    <td width="50%"><img src="docs/pics/performance/baseline/wrk-latency.png" alt="HTTP Latency"/><br/><p align="center"><b>Latency (microseconds)</b></p></td>
  </tr>
  <tr>
    <th colspan="2" align="center">HTTPS Benchmark Results</th>
  </tr>
  <tr>
    <td width="50%"><img src="docs/pics/performance/tls/wrk-reqsec.png" alt="HTTPS Requests per Second"/><br/><p align="center"><b>Requests per Second</b></p></td>
    <td width="50%"><img src="docs/pics/performance/tls/wrk-latency.png" alt="HTTPS Latency"/><br/><p align="center"><b>Latency (microseconds)</b></p></td>
  </tr>
</table>

**Compatibility**

Orion Proxy configuration is generated from Envoy's xDS protobuf definitions. Orion Proxy aims to be a drop in replacement for Envoy.

## Architecture

Orion Proxy is designed as a high-performance L7 proxy compatible with Envoy's xDS API while delivering superior performance through Rust's zero-cost abstractions and memory safety guarantees.

<img src="docs/pics/architecture/orion_architecture.png" alt="Orion Architecture" style="zoom: 80%;" />

### Core Components

- **xDS Client**: Subscribes to and processes Envoy xDS APIs (LDS, RDS, CDS, EDS) for dynamic configuration updates
- **Configuration Manager**: Manages runtime configuration, converts Envoy protobuf definitions to Orion's internal representation
- **Listener Manager**: Handles incoming connections, applies listener filters (TLV, Proxy Protocol, TLS Inspector)
- **Router (L7)**: HTTP routing, header manipulation, retries, timeouts, and traffic shifting
- **Cluster Manager**: Manages upstream clusters, implements load balancing algorithms (round-robin, least-request, random)
- **Transport Layer**: High-performance async I/O using Tokio, supports HTTP/1.1, HTTP/2, and raw TCP
- **TLS Engine**: Powered by rustls for memory-safe TLS/mTLS, client certificate validation
- **Observability**: Prometheus metrics export, OpenTelemetry tracing integration
- **Admin Interface**: HTTP API for runtime configuration inspection, metrics, and health checks

### Key Design Principles

- **Zero-Copy I/O**: Minimizes memory allocations and copies through Rust's ownership system and `Bytes` buffers
- **Async Runtime**: Built on Tokio for efficient handling of thousands of concurrent connections
- **Memory Safety**: Eliminates entire classes of bugs (use-after-free, data races) through Rust's type system
- **Envoy Compatibility**: Direct protobuf compatibility with Envoy xDS APIs for seamless integration with Istio and other control planes

## Quick Start

### Building

```console
git clone https://github.com/kmesh-net/orion
cd orion
git submodule init
git submodule update --force
cargo build
```

### Running

```console
cargo run --bin orion -- --config orion/conf/orion-runtime.yaml
```

### Docker

Build and run with Docker:

```bash
# Build
docker build -t orion-proxy -f docker/Dockerfile .

# Run
docker run -p 8000:8000 --name orion-proxy orion-proxy

# Verify service
curl -v http://localhost:8000/direct-response # Should return HTTP 200 with "meow! 🐱"
```

For detailed Docker configuration options, see [docker/README.md](docker/README.md).

## CPU/Thread Limit Configuration

Orion can be configured to use a specific number of CPU cores/threads by setting the `ORION_CPU_LIMIT` environment variable. This is especially useful in containerized environments where access to `/sys/fs` may be restricted.

### Kubernetes Example (Downward API)

Add the following to your container spec to set `ORION_CPU_LIMIT` to the container's CPU limit:

```yaml
env:
    - name: ORION_CPU_LIMIT
      valueFrom:
          resourceFieldRef:
              resource: limits.cpu
              divisor: "1"
```

Orion will automatically use this value to determine the number of threads/cores.

## Testing with Backend Servers

For testing load balancing with real backend servers:

```bash
# Start two nginx containers
docker run -d -p 4001:80 --name backend1 nginx:alpine
docker run -d -p 4002:80 --name backend2 nginx:alpine

# Start Orion Proxy (uses host networking to access localhost:4001/4002)
docker run -d --network host --name orion-proxy orion-proxy

# Test load balancing
curl http://localhost:8000/  # Proxies to nginx backends!

# Cleanup
docker rm -f backend1 backend2 orion-proxy
```

## Examples and Demos

### TLV Listener Filter Demo

Orion includes a comprehensive TLV (Type-Length-Value) listener filter demo compatible with the Kmesh project for service mesh integration. This demo provides end-to-end testing of the TLV filter functionality.

To test the TLV filter:

```bash
cd examples/tlv-filter-demo
./test_tlv_config.sh
```

This demo will:
- Start Orion with TLV filter configuration matching Kmesh format
- Load the TLV listener filter using TypedStruct configuration
- Send actual TLV packets to test the filter functionality
- Extract and verify original destination information from TLV data
- Show debug logs confirming successful TLV processing
- Verify compatibility with Kmesh TLV configuration format

For detailed information, see [examples/tlv-filter-demo/README.md](examples/tlv-filter-demo/README.md).

<!-- ## Contributing -->
<!-- If you're interested in being a contributor and want to get involved in developing Orion Proxy, please see [CONTRIBUTING](CONTRIBUTING.md) for more details on submitting patches and the contribution workflow. -->

## License

Orion Proxy is licensed under the [Apache License, Version 2.0](./LICENSE).

[![FOSSA Status](https://app.fossa.com/api/projects/git%2Bgithub.com%2Fkmesh-net%2Forion.svg?type=large)](https://app.fossa.com/projects/git%2Bgithub.com%2Fkmesh-net%2Forion?ref=badge_large)
