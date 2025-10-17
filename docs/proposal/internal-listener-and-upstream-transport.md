---
title: Internal Listener and Upstream Transport Support
authors:
- "@Eeshu-Yadav"
reviewers:
- "@YaoZengzeng"
- "@dawid-nowak"
approvers:
- "@YaoZengzeng"
- "@dawid-nowak"

creation-date: 2025-10-03

---

## Internal Listener and Upstream Transport Support

### Summary

This proposal implements internal listener and upstream transport functionality in Orion to enable waypoint proxy capabilities. Internal listeners allow in-process communication without network APIs, while internal upstream transport enables metadata passthrough between proxy hops. This enables ambient mesh deployments with TCP proxy chaining and multi-hop routing.

### Motivation

To support ambient service mesh deployments, Orion needs:

1. **Internal connections**: Accept connections from within the same process via in-memory channels
2. **Name-based routing**: Route to internal listeners by name instead of network addresses
3. **Metadata propagation**: Preserve request context across proxy hops for routing and observability
4. **Performance optimization**: Eliminate network stack overhead for co-located proxy communication

#### Goals

- Implement Envoy-compatible internal listener and upstream transport support
- Enable clusters to connect via `server_listener_name` with metadata passthrough
- Provide thread-safe connection handling with proper lifecycle management
- Maintain full compatibility with Envoy xDS configurations
- Ensure zero performance regression for network listeners


### Proposal

The proposal introduces three main components to enable internal listener and upstream transport functionality:

1. **Internal Connection Factory**: A global, thread-safe registry that manages internal listener registration and connection establishment between clusters and listeners within the same proxy instance.

2. **Enhanced Internal Listener Runtime**: Extension of the existing listener infrastructure to handle internal connections, process them through filter chains, and manage lifecycle events.

3. **Internal Upstream Transport**: Implementation of cluster-side functionality to establish connections to internal endpoints and pass metadata through the transport socket layer.

The implementation follows Envoy's internal listener design while leveraging Rust's type system and async runtime for safety and performance.


#### Notes

**Design Decisions:**
- In-process only communication using Tokio duplex streams
- Global factory pattern with `std::sync::OnceLock` for thread-safe initialization
- Weak references for automatic lifecycle management
- Initial support for host, cluster, and route metadata (request metadata in future)
- Full Envoy configuration compatibility with listener name validation


### Design Details

#### Architecture Overview

```
┌─────────────────────────────────────────────────────────────────┐
│                         Orion Proxy Process                     │
│                                                                 │
│  ┌──────────────┐                    ┌──────────────┐           │
│  │   External   │                    │   Internal   │           │
│  │   Listener   │                    │   Listener   │           │
│  │  (Network)   │                    │ (In-Memory)  │           │
│  └──────┬───────┘                    └──────┬───────┘           │
│         │                                   │                   │
│         │ TCP Connection                    │ Register          │
│         ▼                                   ▼                   │
│  ┌────────────────────────────────────────────────────┐         │
│  │         Internal Connection Factory                │         │
│  │  ┌──────────────────────────────────────────┐      │         │
│  │  │  Listener Registry                       │      │         │
│  │  │  HashMap<String, ListenerHandle>         │      │         │
│  │  └──────────────────────────────────────────┘      │         │
│  └────────────────────────────┬───────────────────────┘         │
│                               │                                 │
│                               │ Connect                         │
│                               ▼                                 │
│  ┌──────────────┐      ┌─────────────────┐                      │
│  │   Cluster    │─────▶│  TCP Proxy      │                      │
│  │   (Internal  │      │  Filter         │                      │
│  │   Endpoint)  │      └─────────────────┘                      │
│  └──────────────┘                                               │
│         │                                                       │
│         │ Internal Connection (Duplex Stream)                   │
│         ▼                                                       │
│  ┌──────────────────────────────────────┐                       │
│  │  Internal Upstream Transport         │                       │
│  │  - Metadata Passthrough              │                       │
│  │  - Host/Cluster/Route Metadata       │                       │
│  └──────────────────────────────────────┘                       │
└─────────────────────────────────────────────────────────────────┘
```

#### Component Details

##### 1. Internal Connection Factory

**Location**: `orion-lib/src/transport/internal_connection.rs`

```rust
pub struct InternalConnectionFactory {
    listeners: Arc<RwLock<HashMap<String, InternalListenerHandle>>>,
}

pub struct InternalListenerHandle {
    pub name: String,
    pub connection_sender: mpsc::UnboundedSender<InternalConnectionPair>,
    listener_ref: Weak<()>,
}

pub struct InternalConnectionPair {
    pub upstream: Arc<InternalStream>,
    pub downstream: Arc<InternalStream>,
}

pub struct InternalStream {
    metadata: InternalConnectionMetadata,
    stream: tokio::io::DuplexStream,
    is_closed: Arc<RwLock<bool>>,
}
```

**Key Operations**: `register_listener`, `unregister_listener`, `connect_to_listener`, `is_listener_active`, `list_listeners`, `get_stats`

##### 2. Enhanced Internal Listener Runtime

**Location**: `orion-lib/src/listeners/listener.rs`

```rust
async fn run_internal_listener(
    name: &'static str,
    filter_chains: HashMap<FilterChainMatch, FilterchainType>,
    mut route_updates_receiver: broadcast::Receiver<RouteConfigurationChange>,
    mut secret_updates_receiver: broadcast::Receiver<TlsContextChange>,
) -> Error {
    let factory = global_internal_connection_factory();
    let (_handle, mut connection_receiver, _listener_ref) = 
        factory.register_listener(name.to_string()).await?;
    
    loop {
        tokio::select! {
            Some(connection_pair) = connection_receiver.recv() => {
                tokio::spawn(handle_internal_connection(connection_pair, filter_chains_clone));
            }
            Ok(route_update) = route_updates_receiver.recv() => {
                process_route_update(&name, &filter_chains, route_update);
            }
            Ok(secret_update) = secret_updates_receiver.recv() => {
                process_secret_update(&name, &mut filter_chains_clone, secret_update);
            }
        }
    }
}
```

##### 3. Internal Cluster Connector

**Location**: `orion-lib/src/transport/internal_cluster_connector.rs`

```rust
pub struct InternalClusterConnector {
    listener_name: String,
    endpoint_id: Option<String>,
}

impl InternalClusterConnector {
    pub async fn connect(&self) -> Result<AsyncStream> {
        let factory = global_internal_connection_factory();
        factory.connect_to_listener(&self.listener_name, self.endpoint_id.clone()).await
    }
}

pub struct InternalChannelConnector {
    connector: InternalClusterConnector,
    cluster_name: &'static str,
}
```

##### 4. Configuration Data Structures

**Listener Configuration** (`orion-configuration/src/config/listener.rs`):

```rust
pub enum ListenerAddress {
    Socket(SocketAddr),
    Internal(InternalListener),
}

pub struct InternalListener {
    pub buffer_size_kb: Option<u32>,
}
```

**Cluster Configuration** (`orion-configuration/src/config/cluster.rs`):

```rust
pub enum EndpointAddress {
    Socket(SocketAddr),
    Internal(InternalEndpointAddress),
}

pub struct InternalEndpointAddress {
    pub server_listener_name: CompactString,
    pub endpoint_id: Option<CompactString>,
}

pub enum TransportSocket {
    InternalUpstream(InternalUpstreamTransport),
    RawBuffer,
}

pub struct InternalUpstreamTransport {
    pub passthrough_metadata: Vec<MetadataValueSource>,
    pub transport_socket: Box<TransportSocket>,
}

pub struct MetadataValueSource {
    pub kind: MetadataKind,
    pub name: CompactString,
}

pub enum MetadataKind {
    Host,
    Route,
    Cluster,
}
```

##### 5. Example Configuration

**Bootstrap Configuration**:

```yaml
static_resources:
  listeners:
  # External listener accepting network connections
  - name: "listener_0"
    address:
      socket_address:
        address: "0.0.0.0"
        port_value: 15001
    filter_chains:
    - filters:
      - name: "tcp_proxy"
        typed_config:
          "@type": "type.googleapis.com/envoy.extensions.filters.network.tcp_proxy.v3.TcpProxy"
          stat_prefix: "ingress_tcp"
          cluster: "internal_cluster"
  
  # Internal listener accepting in-process connections
  - name: "waypoint_internal"
    address:
      envoy_internal_address:
        server_listener_name: "waypoint_internal"
    filter_chains:
    - filters:
      - name: "http_connection_manager"
        typed_config:
          "@type": "type.googleapis.com/envoy.extensions.filters.network.http_connection_manager.v3.HttpConnectionManager"
          stat_prefix: "waypoint_http"
          route_config:
            name: "local_route"
            virtual_hosts:
            - name: "backend"
              domains: ["*"]
              routes:
              - match: { prefix: "/" }
                route: { cluster: "backend_service" }

  clusters:
  # Cluster routing to internal listener
  - name: "internal_cluster"
    type: "STATIC"
    load_assignment:
      cluster_name: "internal_cluster"
      endpoints:
      - lb_endpoints:
        - endpoint:
            address:
              envoy_internal_address:
                server_listener_name: "waypoint_internal"
    transport_socket:
      name: "internal_upstream"
      typed_config:
        "@type": "type.googleapis.com/envoy.extensions.transport_sockets.internal_upstream.v3.InternalUpstreamTransport"
        passthrough_metadata:
        - kind: { host: {} }
          name: "envoy.filters.listener.original_dst"
        - kind: { cluster: {} }
          name: "istio.workload"
        transport_socket:
          name: "raw_buffer"
          typed_config:
            "@type": "type.googleapis.com/envoy.extensions.transport_sockets.raw_buffer.v3.RawBuffer"
  
  # Backend service cluster
  - name: "backend_service"
    type: "STATIC"
    load_assignment:
      cluster_name: "backend_service"
      endpoints:
      - lb_endpoints:
        - endpoint:
            address:
              socket_address:
                address: "10.0.0.5"
                port_value: 8080
```

#### Implementation Phases

The implementation is divided into four GitHub issues for manageable development and review:

**Phase 1: Internal Connection Factory** - Connection factory with thread-safe registry and lifecycle management

**Phase 2: Enhanced Internal Listener Runtime** - Connection acceptance and filter chain integration

**Phase 3: Cluster Internal Connection Support** - Cluster connectors with load balancing for internal endpoints

**Phase 4: Internal Upstream Transport & Metadata Passthrough** - Metadata extraction and passthrough implementation

#### Test Plan

**Unit Tests**:
- Listener registration/unregistration in connection factory
- Connection establishment between listeners and clusters
- Thread safety and concurrent access
- Error handling for non-existent/inactive listeners

**Integration Tests**:
- End-to-end flow: External listener → Internal listener → Backend
- Configuration parsing and validation
- Metadata propagation across proxy hops

---

## References

1. [Envoy Internal Listener Documentation](https://www.envoyproxy.io/docs/envoy/latest/configuration/other_features/internal_listener)
2. [Envoy Internal Upstream Transport Proto](https://www.envoyproxy.io/docs/envoy/latest/api-v3/extensions/transport_sockets/internal_upstream/v3/internal_upstream.proto)
3. [Envoy Metadata Types](https://www.envoyproxy.io/docs/envoy/latest/api-v3/type/metadata/v3/metadata.proto)


---

## Appendix

### Glossary

- **Internal Listener**: A listener that accepts connections from within the proxy process rather than from the network
- **Waypoint Proxy**: A shared proxy in ambient service mesh that handles L7 processing for multiple workloads
- **Internal Upstream Transport**: Transport socket that enables metadata passthrough for internal connections
- **Server Listener Name**: Unique identifier for an internal listener used by clusters to establish connections
- **Metadata Passthrough**: Mechanism to propagate context (host/cluster/route metadata) across proxy hops
- **Duplex Stream**: Bidirectional async I/O stream provided by Tokio for in-memory communication

### Acknowledgments

This feature was proposed by @YaoZengzeng in this [issue](https://github.com/kmesh-net/orion/issues/59) and has been reviewed by @dawid-nowak @YaoZengzeng. The design follows Envoy's internal listener specification while adapting to Orion's Rust-based architecture and async runtime.
