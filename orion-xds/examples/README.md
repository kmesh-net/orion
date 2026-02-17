# Orion XDS Examples – Full Usage Guide

## Directory: `orion-xds/examples/`

This folder contains several examples that demonstrate how to use the Orion XDS library for service discovery and configuration. These are structured around a basic **XDS client-server model**, using **gRPC and async Rust**.

---

## Prerequisites

Before running any example, ensure your environment meets the following:

### Rust Environment

- Install Rust: https://rustup.rs
- Minimum Rust version: `1.70+` (recommended)

### Dependencies

Run this in the root of the Orion repo to make sure all dependencies are available:

```bash
cargo build
```

### Enable Logging (Optional but Useful)

Set the `RUST_LOG` environment variable to see debug/info logs:

```bash
export RUST_LOG=info,orion_xds=debug
```

---

## How to Run the Examples

These examples follow a basic flow:

- Start the **XDS server** (one of the server variants)
- In another terminal, start the **XDS client**
- Observe real-time communication: resource updates (clusters, listeners, secrets) flow from server to client

---

## Running Client and Server Example

### 1. Open **Terminal A** – Run the Server

```bash
cd orion-xds
cargo run --example server
```

What it does:

- Starts a gRPC server on `127.0.0.1:50051`
- Periodically creates and pushes **Cluster** and **Listener** resources
- Also shows how to **remove** those resources dynamically

You’ll see logs like:

```sh
2025-08-07T09:03:29.314549Z  INFO server: Server started
2025-08-07T09:03:29.314589Z  INFO orion_xds::xds::server: Server started 127.0.0.1:50051
2025-08-07T09:03:39.316111Z  INFO server: Adding cluster Cluster-9d0e506a-b3b9-4429-b266-83ae3cdbf4b7
2025-08-07T09:03:44.318214Z  INFO server: Adding listener Resource { name: "Listener-9d0e506a-b3b9-4429-b266-83ae3cdbf4b7", resource_name: None, aliases: [], version: "", resource: Some(Any { type_url: "type.googleapis.com/envoy.config.listener.v3.Listener", value: [10, 45, 76, 105, 115, 116, 101, 110, 101, 114, 45, 57, 100, 48, 101, 53, 48, 54, 97, 45, 98, 51, 98, 57, 45, 52, 52, 50, 57, 45, 98, 50, 54, 54, 45, 56, 51, 97, 101, 51, 99, 100, 98, 102, 52, 98, 55, 18, 19, 10, 17, 18, 12, 49, 57, 50, 46, 49, 54, 56, 46, 49, 46, 49, 48, 24, 192, 62, 26, 252, 3, 26, 199, 3, 10, 101, 116, 121, 112, 101, 46, 103, 111, 111, 103, 108, 101, 97, 112, 105, 115, 46, 99, 111, 109, 47, 101, 110, 118, 111, 121, 46, 101, 120, 116, 101, 110, 115, 105, 111, 110, 115, 46, 102, 105, 108, 116, 101, 114, 115, 46, 110, 101, 116, 119, 111, 114, 107, 46, 104, 116, 116, 112, 95, 99, 111, 110, 110, 101, 99, 116, 105, 111, 110, 95, 109, 97, 110, 97, 103, 101, 114, 46, 118, 51, 46, 72, 116, 116, 112, 67, 111, 110, 110, 101, 99, 116, 105, 111, 110, 77, 97, 110, 97, 103, 101, 114, 34, 221, 2, 10, 101, 116, 121, 112, 101, 46, 103, 111, 111, 103, 108, 101, 97, 112, 105, 115, 46, 99, 111, 109, 47, 101, 110, 118, 111, 121, 46, 101, 120, 116, 101, 110, 115, 105, 111, 110, 115, 46, 102, 105, 108, 116, 101, 114, 115, 46, 110, 101, 116, 119, 111, 114, 107, 46, 104, 116, 116, 112, 95, 99, 111, 110, 110, 101, 99, 116, 105, 111, 110, 95, 109, 97, 110, 97, 103, 101, 114, 46, 118, 51, 46, 72, 116, 116, 112, 67, 111, 110, 110, 101, 99, 116, 105, 111, 110, 77, 97, 110, 97, 103, 101, 114, 18, 243, 1, 8, 1, 34, 238, 1, 10, 56, 76, 105, 115, 116, 101, 110, 101, 114, 45, 57, 100, 48, 101, 53, 48, 54, 97, 45, 98, 51, 98, 57, 45, 52, 52, 50, 57, 45, 98, 50, 54, 54, 45, 56, 51, 97, 101, 51, 99, 100, 98, 102, 52, 98, 55, 45, 114, 111, 117, 116, 101, 45, 99, 111, 110, 102, 18, 177, 1, 10, 48, 76, 105, 115, 116, 101, 110, 101, 114, 45, 57, 100, 48, 101, 53, 48, 54, 97, 45, 98, 51, 98, 57, 45, 52, 52, 50, 57, 45, 98, 50, 54, 54, 45, 56, 51, 97, 101, 51, 99, 100, 98, 102, 52, 98, 55, 45, 118, 99, 18, 1, 42, 18, 11, 101, 120, 97, 109, 112, 108, 101, 46, 99, 111, 109, 26, 109, 10, 3, 18, 1, 47, 18, 46, 10, 44, 67, 108, 117, 115, 116, 101, 114, 45, 57, 100, 48, 101, 53, 48, 54, 97, 45, 98, 51, 98, 57, 45, 52, 52, 50, 57, 45, 98, 50, 54, 54, 45, 56, 51, 97, 101, 51, 99, 100, 98, 102, 52, 98, 55, 114, 54, 76, 105, 115, 116, 101, 110, 101, 114, 45, 57, 100, 48, 101, 53, 48, 54, 97, 45, 98, 51, 98, 57, 45, 52, 52, 50, 57, 45, 98, 50, 54, 54, 45, 56, 51, 97, 101, 51, 99, 100, 98, 102, 52, 98, 55, 45, 118, 99, 45, 114, 111, 117, 116, 101, 58, 48, 76, 105, 115, 116, 101, 110, 101, 114, 45, 57, 100, 48, 101, 53, 48, 54, 97, 45, 98, 51, 98, 57, 45, 52, 52, 50, 57, 45, 98, 50, 54, 54, 45, 56, 51, 97, 101, 51, 99, 100, 98, 102, 52, 98, 55, 45, 102, 99] }), ttl: None, cache_control: None, metadata: None }
2025-08-07T09:03:59.319701Z  INFO server: Removing cluster Cluster-9d0e506a-b3b9-4429-b266-83ae3cdbf4b7
2025-08-07T09:04:04.320811Z  INFO server: Removing listener Resource { name: "Listener-9d0e506a-b3b9-4429-b266-83ae3cdbf4b7", resource_name: None, aliases: [], version: "", resource: Some(Any { type_url: "type.googleapis.com/envoy.config.listener.v3.Listener", value: [10, 45, 76, 105, 115, 116, 101, 110, 101, 114, 45, 57, 100, 48, 101, 53, 48, 54, 97, 45, 98, 51, 98, 57, 45, 52, 52, 50, 57, 45, 98, 50, 54, 54, 45, 56, 51, 97, 101, 51, 99, 100, 98, 102, 52, 98, 55, 18, 19, 10, 17, 18, 12, 49, 57, 50, 46, 49, 54, 56, 46, 49, 46, 49, 48, 24, 192, 62, 26, 252, 3, 26, 199, 3, 10, 101, 116, 121, 112, 101, 46, 103, 111, 111, 103, 108, 101, 97, 112, 105, 115, 46, 99, 111, 109, 47, 101, 110, 118, 111, 121, 46, 101, 120, 116, 101, 110, 115, 105, 111, 110, 115, 46, 102, 105, 108, 116, 101, 114, 115, 46, 110, 101, 116, 119, 111, 114, 107, 46, 104, 116, 116, 112, 95, 99, 111, 110, 110, 101, 99, 116, 105, 111, 110, 95, 109, 97, 110, 97, 103, 101, 114, 46, 118, 51, 46, 72, 116, 116, 112, 67, 111, 110, 110, 101, 99, 116, 105, 111, 110, 77, 97, 110, 97, 103, 101, 114, 34, 221, 2, 10, 101, 116, 121, 112, 101, 46, 103, 111, 111, 103, 108, 101, 97, 112, 105, 115, 46, 99, 111, 109, 47, 101, 110, 118, 111, 121, 46, 101, 120, 116, 101, 110, 115, 105, 111, 110, 115, 46, 102, 105, 108, 116, 101, 114, 115, 46, 110, 101, 116, 119, 111, 114, 107, 46, 104, 116, 116, 112, 95, 99, 111, 110, 110, 101, 99, 116, 105, 111, 110, 95, 109, 97, 110, 97, 103, 101, 114, 46, 118, 51, 46, 72, 116, 116, 112, 67, 111, 110, 110, 101, 99, 116, 105, 111, 110, 77, 97, 110, 97, 103, 101, 114, 18, 243, 1, 8, 1, 34, 238, 1, 10, 56, 76, 105, 115, 116, 101, 110, 101, 114, 45, 57, 100, 48, 101, 53, 48, 54, 97, 45, 98, 51, 98, 57, 45, 52, 52, 50, 57, 45, 98, 50, 54, 54, 45, 56, 51, 97, 101, 51, 99, 100, 98, 102, 52, 98, 55, 45, 114, 111, 117, 116, 101, 45, 99, 111, 110, 102, 18, 177, 1, 10, 48, 76, 105, 115, 116, 101, 110, 101, 114, 45, 57, 100, 48, 101, 53, 48, 54, 97, 45, 98, 51, 98, 57, 45, 52, 52, 50, 57, 45, 98, 50, 54, 54, 45, 56, 51, 97, 101, 51, 99, 100, 98, 102, 52, 98, 55, 45, 118, 99, 18, 1, 42, 18, 11, 101, 120, 97, 109, 112, 108, 101, 46, 99, 111, 109, 26, 109, 10, 3, 18, 1, 47, 18, 46, 10, 44, 67, 108, 117, 115, 116, 101, 114, 45, 57, 100, 48, 101, 53, 48, 54, 97, 45, 98, 51, 98, 57, 45, 52, 52, 50, 57, 45, 98, 50, 54, 54, 45, 56, 51, 97, 101, 51, 99, 100, 98, 102, 52, 98, 55, 114, 54, 76, 105, 115, 116, 101, 110, 101, 114, 45, 57, 100, 48, 101, 53, 48, 54, 97, 45, 98, 51, 98, 57, 45, 52, 52, 50, 57, 45, 98, 50, 54, 54, 45, 56, 51, 97, 101, 51, 99, 100, 98, 102, 52, 98, 55, 45, 118, 99, 45, 114, 111, 117, 116, 101, 58, 48, 76, 105, 115, 116, 101, 110, 101, 114, 45, 57, 100, 48, 101, 53, 48, 54, 97, 45, 98, 51, 98, 57, 45, 52, 52, 50, 57, 45, 98, 50, 54, 54, 45, 56, 51, 97, 101, 51, 99, 100, 98, 102, 52, 98, 55, 45, 102, 99] }), ttl: None, cache_control: None, metadata: None }
```


### 2. Open **Terminal B** – Run the Client

```bash
cd orion-xds
cargo run --example client
```

What it does:

- Connects to the XDS server at `127.0.0.1:50051`
- Subscribes to Aggregated Discovery Service (ADS)
- Sends DeltaDiscoveryRequests for various resource types (Listeners, Clusters)
- Prints updates received for supported resource types (e.g., Clusters)

However, note:

> **Current Limitation:** The client fails to decode Listener configurations due to an unsupported field (`name`) inside the route configuration. You will see warnings about this in the logs.

Sample logs:
```
INFO client: Got update for cluster Cluster-1c6d9ce7-a843-4d91-842c-b4b9dd64cc83
WARN orion_xds::xds::client: problem decoding config update for Listener-1c6d9ce7-a843-4d91-842c-b4b9dd64cc83 : error Some(ConversionError(TracedError([... / routes [0], UnsupportedField("name"))))
```

**What this means:**
- Cluster resources are successfully received and parsed.
- ❌ Listener resources are received but fail during decoding due to an unsupported `"name"` field in the route.
- The client will continue retrying and resubscribing as expected, but it won't process listener configs correctly until the field is removed.

---

## Running Other Examples

Each file demonstrates a specialized XDS capability.

### `server_routes_and_loads.rs`

```bash
cargo run --example server_routes_and_loads
```
Sample Logs:
```sh
2025-08-07T09:13:53.411123Z  INFO server_routes_and_loads: Server started
2025-08-07T09:13:53.411184Z  INFO orion_xds::xds::server: Server started 127.0.0.1:50051
2025-08-07T09:14:03.412738Z  INFO server_routes_and_loads: Adding Cluster Load Assignment for cluster cluster_http
2025-08-07T09:14:08.414280Z  INFO server_routes_and_loads: Adding Route configuration  rds_route
2025-08-07T09:14:23.415020Z  INFO server_routes_and_loads: Removing cluster load assignment cluster_http
2025-08-07T09:14:28.416480Z  INFO server_routes_and_loads: Removing route configuration rds_route
```

**Purpose**:
Shows how to:

- Dynamically add and remove XDS resources at runtime
- Configure RouteConfiguration objects that map URL paths to upstream clusters
- Push ClusterLoadAssignments that specify endpoint details for a cluster

**Use case**:
Testing Dynamic Config Updates, Simulating Traffic Shaping and Routing Logic, Endpoint Failover and Service Discovery

**Client**:
For running the client you can rerun the previous example of the client.
