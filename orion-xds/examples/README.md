# Orion XDS Examples

This directory contains examples demonstrating how to use Orion's XDS (Discovery Service) library for dynamic configuration management.

## Overview

XDS (Discovery Service) is a set of APIs that allows proxies to discover configuration dynamically. These examples show how to:

- Implement XDS clients that receive configuration updates
- Build XDS servers that provide configuration to clients
- Handle dynamic route updates and load assignments
- Manage TLS certificate rotation via SDS (Secret Discovery Service)

## Prerequisites

```bash
# Install Rust (if not already installed)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Install Protocol Buffers compiler
sudo apt-get install protobuf-compiler  # Ubuntu/Debian
# OR
brew install protobuf                    # macOS

# Build the project
cd orion
cargo build
```

## Examples

### 1. Basic Client (`client.rs`)

Shows how to connect to an XDS server and handle configuration updates.

**Run:**
```bash
# From orion root directory
cargo run --example client --package orion-xds
```

**What it does:**
- Connects to an XDS server at `127.0.0.1:50051`
- Subscribes to listener and cluster updates
- Logs received configuration changes
- Handles acknowledgments for updates

### 2. Basic Server (`server.rs`) 

Demonstrates implementing an XDS server that provides configuration to clients.

**Run:**
```bash
# From orion root directory  
cargo run --example server --package orion-xds
```

**What it does:**
- Starts an XDS server on `127.0.0.1:50051`
- Dynamically creates and removes listeners and clusters
- Shows how to add/remove configurations over time
- Updates configurations every few seconds

### 3. Routes and Load Assignment (`server_routes_and_loads.rs`)

Shows dynamic route configuration and load balancing updates.

**Run:**
```bash
# From orion root directory
cargo run --example server_routes_and_loads --package orion-xds  
```

**What it does:**
- Manages route configurations (RDS - Route Discovery Service)
- Handles cluster load assignments (EDS - Endpoint Discovery Service)  
- Demonstrates adding and removing endpoints dynamically

### 4. Secret Rotation (`server_secret_rotation.rs`)

Demonstrates TLS certificate management via SDS (Secret Discovery Service).

**Run:**
```bash
# From orion root directory
cargo run --example server_secret_rotation --package orion-xds
```

**What it does:**
- Shows how to rotate TLS certificates dynamically
- Manages both downstream (listener) and upstream (cluster) certificates
- Uses certificate files from the `test_certs/` directory

### 5. Simple Secret Rotation (`server_secret_rotation_simple.rs`)

A simplified version of certificate rotation focusing on upstream certificates only.

**Run:**
```bash
# From orion root directory  
cargo run --example server_secret_rotation_simple --package orion-xds
```

**What it does:**
- Demonstrates basic upstream certificate rotation
- Simpler than the full rotation example
- Good starting point for SDS understanding

## Testing End-to-End

To see the examples working together:

**Terminal 1 - Start a server:**
```bash
cargo run --example server --package orion-xds
```

**Terminal 2 - Connect a client:**  
```bash
cargo run --example client --package orion-xds
```

You should see the client receiving updates from the server and logging the configuration changes.

## Key Concepts

### XDS Protocol Types
- **LDS (Listener Discovery Service)**: Manages proxy listeners
- **RDS (Route Discovery Service)**: Manages HTTP route configurations  
- **CDS (Cluster Discovery Service)**: Manages upstream clusters
- **EDS (Endpoint Discovery Service)**: Manages endpoints within clusters
- **SDS (Secret Discovery Service)**: Manages TLS certificates and keys

### Code Structure

The examples use these main components from `orion-xds`:

- `start_aggregate_client()` - Creates an XDS client that can handle multiple resource types
- `start_aggregate_server()` - Creates an XDS server that serves multiple resource types  
- `resources::create_*()` - Helper functions to create different XDS resource types
- `ServerAction::Add/Remove` - Actions for dynamically updating server configurations

### Customization

The examples are designed to be easily customizable:

1. **Change server address**: Modify the `127.0.0.1:50051` address in both client and server
2. **Add more resource types**: Extend the examples to handle additional XDS resource types
3. **Custom configurations**: Modify the resource creation calls to match your use case
4. **Different update patterns**: Change the timing and logic for configuration updates

## Troubleshooting

**Connection issues:**
- Ensure the server is running before starting the client
- Check that port 50051 is not in use by another process

**Build issues:**
- Make sure `protoc` is installed and in your PATH
- Verify Rust version is 1.89.0 or later

**Certificate issues (for SDS examples):**
- Ensure the `test_certs/` directory exists with valid certificates
- Check file permissions on certificate files

## Next Steps

After running these examples, you can:

1. **Integrate with real control planes** like Istio or Consul Connect
2. **Build custom XDS servers** for your specific configuration needs  
3. **Use Orion as a data plane** with your XDS-based control plane
4. **Implement additional XDS resource types** as needed

For more information on XDS protocol, see the [xDS API documentation](https://www.envoyproxy.io/docs/envoy/latest/api-docs/xds_protocol).
