---
title: Connection Draining for Multiple Listener Versions
authors:
- "@Eeshu-Yadav"
reviewers:
- "@YaoZengzeng"
- "@dawid-nowak"
- "@hzxuzhonghu"
approvers:
- "@YaoZengzeng"
- "@dawid-nowak"
- "@hzxuzhonghu"
creation-date: 2025-10-09
---

## Connection Draining for Multiple Listener Versions

### Summary

This proposal implements Envoy-compatible connection draining for multiple listener versions in Orion. When listener configurations are updated via LDS (Listener Discovery Service), existing connections continue on old listener versions while new connections seamlessly transition to updated listeners. This ensures zero-downtime updates and follows Envoy's graceful draining behavior with protocol-specific timeout handling.

### Motivation

Currently, when updating a listener configuration in Orion:

1. **Connection Interruption**: Old connections get abruptly terminated
2. **No Graceful Period**: No graceful shutdown mechanism for existing connections  
3. **Non-Envoy Compliant**: Doesn't follow Envoy's standard draining behavior
4. **Protocol Agnostic**: No protocol-specific handling (HTTP/1.1, HTTP/2, TCP)

This causes service disruptions during configuration updates, making Orion unsuitable for production environments requiring high availability.

#### Goals

- Implement Envoy-compatible connection draining with protocol-specific behavior
- Support graceful listener updates via LDS without connection interruption
- Follow Envoy's timeout mechanisms and drain sequence
- Maintain full backward compatibility with existing configurations
- Enable production-ready zero-downtime deployments

#### Non-Goals

- Implementing custom draining protocols beyond Envoy compatibility
- Supporting non-standard timeout configurations outside Envoy's model
- Backward compatibility with non-Envoy proxy behaviors

### Proposal

The proposal implements a comprehensive connection draining system that manages multiple listener versions during LDS updates. When a listener configuration is updated, the system:

1. **Starts New Version**: Creates the new listener version with updated configuration
2. **Begins Draining**: Marks old versions for graceful draining
3. **Protocol-Specific Handling**: Applies appropriate draining behavior per protocol
4. **Timeout Management**: Enforces Envoy-compatible timeout sequences
5. **Resource Cleanup**: Removes old versions after all connections drain

The implementation follows Envoy's draining documentation and maintains compatibility with existing Envoy configurations.

### Design Details

#### Architecture Overview

```
┌─────────────────────────────────────────────────────────────────┐
│                    Orion Listener Manager                        │
│                                                                 │
│  ┌──────────────┐    LDS Update    ┌──────────────┐             │
│  │   Listener   │◄─────────────────┤   New        │             │
│  │   Version 1  │                  │   Listener   │             │
│  │   (Active)   │                  │   Version 2  │             │
│  └──────┬───────┘                  └──────┬───────┘             │
│         │                                 │                     │
│         │ Start Draining                  │ Accept New          │
│         ▼                                 ▼ Connections         │
│  ┌─────────────────────────────────────────────────────┐       │
│  │            Drain Signaling Manager                  │       │
│  │  ┌─────────────────────────────────────────────┐    │       │
│  │  │  Protocol-Specific Drain Handlers          │    │       │
│  │  │  ┌─────────────┬─────────────┬───────────┐  │    │       │
│  │  │  │   HTTP/1.1  │   HTTP/2    │    TCP    │  │    │       │
│  │  │  │             │             │           │  │    │       │
│  │  │  │ Connection: │ GOAWAY      │ SO_LINGER │  │    │       │
│  │  │  │ close       │ Frame       │ Timeout   │  │    │       │
│  │  │  └─────────────┴─────────────┴───────────┘  │    │       │
│  │  └─────────────────────────────────────────────┘    │       │
│  └─────────────────────────────────────────────────────┘       │
│                            │                                   │
│                            │ Timeout Management                │
│                            ▼                                   │
│  ┌─────────────────────────────────────────────────────┐       │
│  │              Drain Timeout Handler                  │       │
│  │  • HTTP: drain_timeout from HCM config             │       │
│  │  • TCP: Global server drain timeout                │       │
│  │  • Absolute: 600s maximum (--drain-time-s)         │       │
│  └─────────────────────────────────────────────────────┘       │
└─────────────────────────────────────────────────────────────────┘
```

#### Component Details

##### 1. Drain Signaling Manager

**Location**: `orion-lib/src/listeners/drain_signaling.rs`

```rust
#[derive(Debug)]
pub struct DrainSignalingManager {
    drain_contexts: Arc<RwLock<HashMap<String, Arc<ListenerDrainContext>>>>,
    global_drain_timeout: Duration,
    default_http_drain_timeout: Duration,
    listener_drain_state: Arc<RwLock<Option<ListenerDrainState>>>,
}

#[derive(Debug)]
pub struct ListenerDrainContext {
    pub listener_id: String,
    pub strategy: DrainStrategy,
    pub drain_start: Instant,
    pub initial_connections: usize,
    pub active_connections: Arc<RwLock<usize>>,
    pub completed: Arc<RwLock<bool>>,
}

#[derive(Debug, Clone)]
pub struct ListenerDrainState {
    pub started_at: Instant,
    pub strategy: super::listeners_manager::DrainStrategy,
    pub protocol_behavior: super::listeners_manager::ProtocolDrainBehavior,
    pub drain_scenario: DrainScenario,
    pub drain_type: ConfigDrainType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrainScenario {
    HealthCheckFail,
    ListenerUpdate,
    HotRestart,
}

#[derive(Debug, Clone)]
pub enum DrainStrategy {
    Tcp { global_timeout: Duration },
    Http { global_timeout: Duration, drain_timeout: Duration },
    Mixed { global_timeout: Duration, http_drain_timeout: Duration, tcp_connections: bool, http_connections: bool },
    Immediate,
}
```

**Key Operations**: 
- `start_listener_draining`: Initialize listener-wide draining
- `stop_listener_draining`: Terminate draining process
- `is_listener_draining`: Check draining status
- `create_drain_context`: Create per-listener drain tracking

##### 2. Connection Manager with Drain Support

**Location**: `orion-lib/src/listeners/listeners_manager.rs`

```rust
pub trait ConnectionManager: Send + Sync {
    fn on_connection_established(&self, listener_name: &str, conn_info: ConnectionInfo);
    fn on_connection_closed(&self, listener_name: &str, connection_id: &str);
    fn start_connection_draining(
        &self,
        listener_name: &str,
        connection_id: &str,
        protocol_behavior: &ProtocolDrainBehavior,
    );
    fn get_active_connections(&self, listener_name: &str) -> Vec<ConnectionInfo>;
    fn force_close_connection(&self, listener_name: &str, connection_id: &str);
}

#[derive(Debug, Default)]
pub struct DefaultConnectionManager {
    connections: Arc<DashMap<String, ConnectionInfo>>,
    listener_connection_counts: Arc<DashMap<String, AtomicUsize>>,
    http_managers: Arc<DashMap<String, Arc<HttpConnectionManager>>>,
}

#[derive(Debug, Clone)]
pub struct ConnectionInfo {
    pub id: String,
    pub protocol: ConnectionProtocol,
    pub established_at: Instant,
    pub last_activity: Instant,
    pub state: ConnectionState,
}

#[derive(Debug, Clone)]
pub enum ConnectionProtocol {
    Http1,
    Http2,
    Tcp,
    Unknown,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConnectionState {
    Active,
    Draining,
    Closing,
    Closed,
}

#[derive(Debug, Clone)]
pub enum ProtocolDrainBehavior {
    Http1 { connection_close: bool },
    Http2 { send_goaway: bool },
    Tcp { force_close_after: Duration },
    Auto,
}
```

**Key Operations**:
- `on_connection_established`: Track new connections
- `start_connection_draining`: Begin per-connection draining  
- `get_connections_by_state`: Filter connections by state
- `cleanup_stale_draining_connections`: Force close timeout connections
- `start_draining_http_managers`: Integrate with HTTP connection manager

##### 3. Enhanced Listener Manager with Drain Support

**Location**: `orion-lib/src/listeners/listeners_manager.rs`

```rust
pub struct ListenersManager {
    listener_configuration_channel: mpsc::Receiver<ListenerConfigurationChange>,
    route_configuration_channel: mpsc::Receiver<RouteConfigurationChange>,
    listener_handles: MultiMap<String, ListenerInfo>,
    version_counter: u64,
    config: ListenerManagerConfig,
    connection_manager: Arc<DefaultConnectionManager>,
}

#[derive(Debug)]
struct ListenerInfo {
    handle: abort_on_drop::ChildTask<()>,
    listener_conf: ListenerConfig,
    version: u64,
    state: ListenerState,
    connections_count: Arc<AtomicUsize>,
    drain_manager_handle: Option<abort_on_drop::ChildTask<()>>,
}

#[derive(Debug, Clone)]
enum ListenerState {
    Active,
    Draining { started_at: Instant, drain_config: ListenerDrainConfig },
}

#[derive(Debug, Clone)]
pub struct ListenerDrainConfig {
    pub drain_time: Duration,
    pub drain_strategy: DrainStrategy,
    pub protocol_handling: ProtocolDrainBehavior,
}

#[derive(Debug, Clone)]
pub enum DrainStrategy {
    Gradual,
    Immediate,
}
```

**Key Operations**:
- `start_listener`: Handle LDS updates with graceful transition using MultiMap
- `start_draining`: Initiate protocol-aware draining for listener version
- `start_drain_monitor`: Monitor drain progress with configurable timeouts
- `drain_old_listeners`: Cleanup policy enforcement for version management
- `resolve_address_conflicts`: Handle address binding conflicts during updates

##### 4. Configuration Integration

**Listener Configuration** (`orion-configuration/src/config/listener.rs`):
```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Listener {
    pub name: CompactString,
    pub address: SocketAddr,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub version_info: Option<String>,
    #[serde(with = "serde_filterchains")]
    pub filter_chains: HashMap<FilterChainMatch, FilterChain>,
    #[serde(default)]
    pub drain_type: DrainType,
    // ... other fields
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DrainType {
    #[default]
    Default,
    ModifyOnly,
}
```

**HTTP Connection Manager Drain Configuration** (`orion-configuration/src/config/network_filters/http_connection_manager.rs`):
```rust
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct HttpConnectionManager {
    pub codec_type: CodecType,
    #[serde(with = "humantime_serde")]
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub request_timeout: Option<Duration>,
    #[serde(with = "humantime_serde")]
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub drain_timeout: Option<Duration>,
    // ... other fields
}
```

**Listener Manager Configuration**:
```rust
#[derive(Debug, Clone)]
pub struct ListenerManagerConfig {
    pub max_versions_per_listener: usize,
    pub cleanup_policy: CleanupPolicy,
    pub cleanup_interval: Duration,
    pub drain_config: ListenerDrainConfig,
}

#[derive(Debug, Clone)]
pub enum CleanupPolicy {
    CountBasedOnly(usize),
}
```

##### 5. Example Configuration

**Bootstrap Configuration with Drain Settings**:
```yaml
static_resources:
  listeners:
  - name: "listener_0"
    address:
      socket_address:
        address: "0.0.0.0"
        port_value: 10000
    drain_type: DEFAULT  # or MODIFY_ONLY
    filter_chains:
    - filters:
      - name: "http_connection_manager"
        typed_config:
          "@type": "type.googleapis.com/envoy.extensions.filters.network.http_connection_manager.v3.HttpConnectionManager"
          stat_prefix: "ingress_http"
          drain_timeout: "5s"  # HTTP drain timeout
          route_config:
            name: "local_route"
            virtual_hosts:
            - name: "backend"
              domains: ["*"]
              routes:
              - match: { prefix: "/" }
                route: { cluster: "backend_service" }

# Listener manager configuration (applied at startup)
listener_manager:
  max_versions_per_listener: 2
  cleanup_policy: 
    count_based_only: 2
  cleanup_interval: "60s"
  drain_config:
    drain_time: "600s"    # Global drain timeout (equivalent to --drain-time-s)
    drain_strategy: "gradual"  # or "immediate"
    protocol_handling: "auto"  # auto-detect protocol behavior
```

#### Implementation Timeline

The implementation is divided into manageable phases aligned with current development:

**Phase 1: Core Drain Infrastructure** ✅ **COMPLETED in PR #104**
- ✅ MultiMap-based listener version management  
- ✅ Basic drain signaling manager with protocol awareness
- ✅ Connection state tracking (Active, Draining, Closing, Closed)
- ✅ Integration with existing listener manager
- ✅ HTTP connection manager drain integration

**Phase 2: Enhanced Protocol Handlers** 🚧 **IN PROGRESS**
- 🚧 HTTP/1.1 Connection: close header injection
- 🚧 HTTP/2 GOAWAY frame implementation  
- 🚧 TCP SO_LINGER graceful shutdown
- 🚧 Protocol detection from filter chain analysis

**Phase 3: Advanced Timeout Management** 📋 **PLANNED**
- ⏳ Configurable timeout policies with cascade handling
- ⏳ Integration with HTTP connection manager drain_timeout field
- ⏳ Global server drain timeout support (--drain-time-s equivalent)
- ⏳ Force close mechanisms with protocol-specific timeouts

**Phase 4: Production Hardening** 📋 **PLANNED**  
- ⏳ Comprehensive error handling and recovery
- ⏳ Metrics and observability integration
- ⏳ Performance optimization for high connection counts
- ⏳ Edge case handling and stress testing

#### Envoy Compatibility Matrix

| Feature | Envoy Behavior | Orion Implementation Status |
|---------|----------------|------------------------------|
| LDS Updates | Graceful transition to new listener | ✅ **IMPLEMENTED** - MultiMap-based version management |
| Multiple Listener Versions | Support multiple concurrent versions | ✅ **IMPLEMENTED** - MultiMap with version tracking |
| Drain Type Support | DEFAULT vs MODIFY_ONLY behavior | ✅ **IMPLEMENTED** - Full configuration support |
| Connection State Tracking | Track connection lifecycle | ✅ **IMPLEMENTED** - Active/Draining/Closing/Closed states |
| HTTP Manager Integration | HCM drain timeout field | ✅ **IMPLEMENTED** - HTTP connection manager integration |
| HTTP/1.1 Draining | Connection: close header | 🚧 **IN PROGRESS** - Header injection mechanism |
| HTTP/2 Draining | GOAWAY frame | 🚧 **IN PROGRESS** - RFC 7540 compliant implementation |
| TCP Draining | SO_LINGER timeout | 🚧 **IN PROGRESS** - Socket option configuration |
| Global Timeout | --drain-time-s argument | ⏳ **PLANNED** - Absolute maximum failsafe |
| Protocol Detection | Auto-detect from filter chains | ⏳ **PLANNED** - Filter chain analysis |

#### Test Plan

**Unit Tests**:
- Protocol-specific drain handler behavior
- Timeout enforcement and cascade handling
- Connection state tracking accuracy
- Configuration parsing and validation

**Integration Tests**:
- LDS update scenarios with multiple listener versions
- End-to-end draining flow for each protocol
- Timeout behavior under various load conditions
- Error handling during drain failures

**Performance Tests**:
- Connection draining under high connection counts
- Memory usage during extended drain periods
- CPU overhead of drain monitoring
- Latency impact on new connections during drain

**Compatibility Tests**:
- Envoy configuration compatibility
- XDS protocol compliance
- Metrics format compatibility

### Alternative Designs Considered

#### 1. Single Version Replacement
**Approach**: Replace listeners immediately without draining
**Pros**: Simple implementation, no resource overhead
**Cons**: Connection interruption, not Envoy-compatible

#### 2. Custom Drain Protocol
**Approach**: Implement proprietary draining mechanism
**Pros**: Potentially more efficient for specific use cases
**Cons**: Non-standard, compatibility issues, maintenance burden

#### 3. External Drain Controller
**Approach**: Separate service managing drain operations
**Pros**: Decoupled architecture, independent scaling
**Cons**: Added complexity, network overhead, single point of failure

### Security Considerations

- **Resource Exhaustion**: Drain timeouts prevent indefinite resource consumption
- **DoS Protection**: Maximum connection limits during drain periods
- **Information Disclosure**: Drain status metrics don't expose sensitive data
- **Access Control**: Drain operations respect existing RBAC policies

### Observability

**Metrics**:
- `orion_listener_versions_active`: Number of active listener versions per listener name
- `orion_listener_drain_duration_seconds`: Time spent draining per listener version
- `orion_listener_drain_connections_active`: Active connections during drain per listener
- `orion_listener_drain_timeouts_total`: Count of drain timeout events
- `orion_connection_state_transitions_total`: Connection state change events
- `orion_drain_strategy_usage_total`: Usage count per drain strategy type

**Logging**:
- Listener version creation and drain initiation events
- Connection state transitions (Active → Draining → Closing → Closed)
- Protocol-specific drain progress and timeout warnings  
- Configuration validation errors and conflicts
- Force close events and cleanup operations

**Tracing**:
- End-to-end LDS update and drain operation spans
- Per-connection lifecycle tracking and state transitions
- Protocol-specific drain handler execution
- Timeout enforcement and decision points

---

## References

1. [Envoy Draining Documentation](https://www.envoyproxy.io/docs/envoy/latest/intro/arch_overview/operations/draining)
2. [Envoy HTTP Connection Manager Proto](https://www.envoyproxy.io/docs/envoy/latest/api-v3/extensions/filters/network/http_connection_manager/v3/http_connection_manager.proto)
3. [Envoy Listener Proto](https://www.envoyproxy.io/docs/envoy/latest/api-v3/config/listener/v3/listener.proto)
4. [Envoy LDS Protocol](https://www.envoyproxy.io/docs/envoy/latest/configuration/listeners/lds)

---

## Appendix

### Glossary

- **Connection Draining**: Process of gracefully closing existing connections while preventing new ones
- **LDS (Listener Discovery Service)**: XDS protocol for dynamic listener configuration updates
- **Drain Timeout**: Maximum time allowed for connections to close gracefully before force termination
- **GOAWAY Frame**: HTTP/2 control frame indicating no new streams should be created
- **SO_LINGER**: Socket option controlling close behavior and timeout
- **MultiMap**: Data structure allowing multiple values per key for listener version management

### Acknowledgments

This feature addresses GitHub issues [#98](https://github.com/kmesh-net/orion/issues/98) and [#102](https://github.com/kmesh-net/orion/issues/102), implementing connection draining for the multiple listener versions functionality that was previously merged in PR [#99](https://github.com/kmesh-net/orion/pull/99). The design incorporates valuable feedback from @hzxuzhonghu about Envoy compliance and protocol-specific handling, @YaoZengzeng and @dawid-nowak's architectural guidance, and follows Envoy's draining specification while adapting to Orion's Rust-based architecture. 

The current implementation in PR #104 builds upon the MultiMap-based listener version management from the merged multiple listener versions feature, ensuring seamless integration with existing LDS handling and configuration management systems.