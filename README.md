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

<!-- ## Contributing -->
<!-- If you're interested in being a contributor and want to get involved in developing Orion Proxy, please see [CONTRIBUTING](CONTRIBUTING.md) for more details on submitting patches and the contribution workflow. -->
### Docker 

   ```console 
   git clone https://github.com/kmesh-net/orion
   cd orion
   git submodule init
   git submodule update --force
   docker build -t orion2 -f docker/Dockerfile .
   ```
## License

Orion Proxy is licensed under the
[Apache License, Version 2.0](./LICENSE).
