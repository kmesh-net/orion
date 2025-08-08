# Orion XDS

Orion XDS is a Rust implementation of the xDS (Discovery Service) protocol for dynamic proxy configuration. It enables dynamic configuration management for service mesh data planes like Orion proxy.

## What is xDS?

xDS is a set of APIs that allows data plane proxies to discover configuration dynamically from control plane services. It's used by service mesh systems like Istio, Consul Connect, and others for:

- Dynamic listener management (LDS)
- HTTP route configuration (RDS)
- Upstream cluster management (CDS)
- Endpoint load balancing (EDS)
- TLS certificate rotation (SDS)

## Features

- **Complete xDS Protocol Support**: LDS, RDS, CDS, EDS, and SDS
- **Client & Server**: Can act as both xDS client (data plane) and server (control plane)
- **Async**: Built on tokio for high-performance async operations
- **Type-Safe**: Rust's type system ensures configuration safety
- **Memory Safe**: No memory leaks or data races

## Examples

The `examples/` directory contains complete working examples:

```bash
# Basic client-server example
cargo run --example server --package orion-xds  # Terminal 1
cargo run --example client --package orion-xds  # Terminal 2

# Route and load balancing example  
cargo run --example server_routes_and_loads --package orion-xds

# TLS certificate rotation
cargo run --example server_secret_rotation --package orion-xds
```

See [examples/README.md](examples/README.md) for detailed documentation.

## Architecture

- **Client**: Connects to xDS servers and receives configuration updates
- **Server**: Serves xDS configurations to clients
- **Resources**: Helper functions for creating xDS resource types
- **Protocol**: Full implementation of the xDS v3 protocol

## Use Cases

- **Service Mesh Data Plane**: Use with Istio, Consul Connect, etc.
- **Dynamic Proxy Configuration**: Update proxy settings without restarts  
- **Load Balancer Management**: Dynamic endpoint and routing updates
- **Certificate Management**: Automatic TLS certificate rotation
- **Custom Control Planes**: Build your own configuration management system

## License

Apache License 2.0 - see [LICENSE](../LICENSE) for details.
