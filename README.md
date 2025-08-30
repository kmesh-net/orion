<img src="docs/pics/logo/orion_proxy_logo.png" alt="orion-proxy-logo" style="zoom: 100%;" />

<!--
[![LICENSE](https://img.shields.io/github/license/kmesh-net/orion)](/LICENSE) [![codecov](https://codecov.io/gh/kmesh-net/kmesh/graph/badge.svg?token=0EGQ84FGDU)](https://img.shields.io/github/license/kmesh-net/orion) 
-->

## Introduction

Orion Proxy is a high performance and memory safe implementation of popular [Envoy Proxy](https://www.envoyproxy.io/). Orion Proxy is implemented in Rust using high-quality open source components. 

### Key features

**Memory Safety**

Rust programming language allows to avoid a whole lot of bugs related to memory management and data races making Orion Proxy a very robust and secure application.  


**Performance**

Orion Proxy offers 2x-4x better throughput and latency than Envoy Proxy. Refer to [Performance](docs/performance/performance.md) to see performance figures and for more details how we tested Orion Proxy .  


**Compatibility**

Orion Proxy configuration is generated from Envoy's xDS protobuf definitions. Orion Proxy aims to be a drop in replacement for Envoy.



## Quick Start

**Note:** To control how many CPU cores/threads Orion uses (especially in containers), set the `ORION_CPU_LIMIT` environment variable. In Kubernetes, use the Downward API:

```yaml
env:
    - name: ORION_CPU_LIMIT
        valueFrom:
            resourceFieldRef:
                resource: limits.cpu
                divisor: "1"
```

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

### Testing with Backend Servers

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

For detailed Docker configuration options, see [docker/README.md](docker/README.md).


<!-- ## Contributing -->
<!-- If you're interested in being a contributor and want to get involved in developing Orion Proxy, please see [CONTRIBUTING](CONTRIBUTING.md) for more details on submitting patches and the contribution workflow. -->

## License

Orion Proxy is licensed under the
[Apache License, Version 2.0](./LICENSE).
