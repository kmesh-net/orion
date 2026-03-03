# Orion-Kmesh Architecture Diagrams

This document provides visual representations of the Orion-Kmesh architecture using Mermaid diagrams. These diagrams illustrate the relationships between crates, data flows, and major code interactions.

## Table of Contents

1. [Crate Dependency Graph](#crate-dependency-graph)
2. [Architectural Layers](#architectural-layers)
3. [Application Startup Flow](#application-startup-flow)
4. [xDS Configuration Update Flow](#xds-configuration-update-flow)
5. [Request Processing Flow](#request-processing-flow)
6. [Access Logging Flow](#access-logging-flow)
7. [Cluster Selection & Load Balancing](#cluster-selection--load-balancing)
8. [Component Interaction Overview](#component-interaction-overview)
9. [Configuration Channel Architecture](#configuration-channel-architecture)
10. [Thread Architecture](#thread-architecture)

## Crate Dependency Graph

This diagram shows the dependencies between all orion-* crates.

```mermaid
graph TD
    %% Layer 0 - Foundation
    error[orion-error<br/>Error handling]
    header[orion-http-header<br/>HTTP headers]
    interner[orion-interner<br/>String interning]
    dataplane[orion-data-plane-api<br/>Envoy protos]

    %% Layer 1 - Utilities
    format[orion-format<br/>Access log formatting]
    metrics[orion-metrics<br/>OpenTelemetry metrics]
    tracing[orion-tracing<br/>Distributed tracing]

    %% Layer 2 - Configuration
    config[orion-configuration<br/>Config parsing]

    %% Layer 3 - Control Plane
    xds[orion-xds<br/>xDS client]

    %% Layer 4 - Runtime
    lib[orion-lib<br/>Proxy runtime]

    %% Layer 5 - Application
    proxy[orion-proxy<br/>Main application]

    %% Dependencies - Layer 1
    format --> header
    format --> interner
    metrics --> config
    metrics --> interner
    tracing --> header
    tracing --> interner
    tracing --> config
    tracing --> error

    %% Dependencies - Layer 2
    config --> error
    config --> format
    config --> interner
    config -.-> dataplane

    %% Dependencies - Layer 3
    xds --> config
    xds --> dataplane
    xds --> error

    %% Dependencies - Layer 4
    lib --> config
    lib --> dataplane
    lib --> error
    lib --> format
    lib --> header
    lib --> interner
    lib --> metrics
    lib --> tracing
    lib --> xds

    %% Dependencies - Layer 5
    proxy --> config
    proxy --> error
    proxy --> format
    proxy --> lib
    proxy --> metrics
    proxy --> tracing
    proxy --> xds

    %% Styling
    classDef layer0 fill:#e1f5ff,stroke:#01579b,stroke-width:2px
    classDef layer1 fill:#f3e5f5,stroke:#4a148c,stroke-width:2px
    classDef layer2 fill:#fff3e0,stroke:#e65100,stroke-width:2px
    classDef layer3 fill:#e8f5e9,stroke:#1b5e20,stroke-width:2px
    classDef layer4 fill:#fce4ec,stroke:#880e4f,stroke-width:2px
    classDef layer5 fill:#fff9c4,stroke:#f57f17,stroke-width:2px

    class error,header,interner,dataplane layer0
    class format,metrics,tracing layer1
    class config layer2
    class xds layer3
    class lib layer4
    class proxy layer5
```

## Architectural Layers

This diagram shows the layered architecture of Orion-Kmesh.

```mermaid
graph TB
    subgraph "Layer 5: Application"
        proxy[orion-proxy<br/>Main Orchestrator]
    end

    subgraph "Layer 4: Runtime"
        lib[orion-lib<br/>Proxy Runtime<br/>Listeners, Clusters, Transport]
    end

    subgraph "Layer 3: Control Plane"
        xds[orion-xds<br/>xDS Client]
    end

    subgraph "Layer 2: Configuration"
        config[orion-configuration<br/>Config Parser & Validator]
    end

    subgraph "Layer 1: Utilities & Observability"
        format[orion-format<br/>Access Logging]
        metrics[orion-metrics<br/>Metrics]
        tracing[orion-tracing<br/>Tracing]
    end

    subgraph "Layer 0: Foundation"
        error[orion-error<br/>Error Handling]
        header[orion-http-header<br/>Headers]
        interner[orion-interner<br/>String Interning]
        dataplane[orion-data-plane-api<br/>Envoy Protos]
    end

    proxy --> lib
    proxy --> xds
    lib --> xds
    lib --> config
    xds --> config
    lib --> format
    lib --> metrics
    lib --> tracing
    config --> format
    metrics --> config
    tracing --> config
    format --> header
    format --> interner
    metrics --> interner
    tracing --> header
    tracing --> interner
    config --> error
    xds --> error
    tracing --> error
    lib --> error
    proxy --> error
    config -.-> dataplane
    xds --> dataplane
    lib --> dataplane

    classDef layer0 fill:#e1f5ff,stroke:#01579b,stroke-width:2px
    classDef layer1 fill:#f3e5f5,stroke:#4a148c,stroke-width:2px
    classDef layer2 fill:#fff3e0,stroke:#e65100,stroke-width:2px
    classDef layer3 fill:#e8f5e9,stroke:#1b5e20,stroke-width:2px
    classDef layer4 fill:#fce4ec,stroke:#880e4f,stroke-width:2px
    classDef layer5 fill:#fff9c4,stroke:#f57f17,stroke-width:2px

    class error,header,interner,dataplane layer0
    class format,metrics,tracing layer1
    class config layer2
    class xds layer3
    class lib layer4
    class proxy layer5
```

## Application Startup Flow

This diagram illustrates the initialization sequence when the proxy starts.

```mermaid
sequenceDiagram
    participant Main as main.rs
    participant Proxy as orion_proxy::run()
    participant Config as Config::new()
    participant TracingMgr as TracingManager
    participant Runtime as proxy::run_orion()
    participant Workers as Worker Threads

    Main->>Main: Setup allocator (jemalloc/dhat)
    Main->>Proxy: Call run()

    Proxy->>TracingMgr: Initialize logging
    activate TracingMgr
    TracingMgr-->>Proxy: Logging ready
    deactivate TracingMgr

    Proxy->>Config: Parse CLI options & config files
    activate Config
    Config->>Config: Deserialize Bootstrap YAML
    Config->>Config: Extract Listeners/Clusters/Secrets
    Config-->>Proxy: Configuration loaded
    deactivate Config

    Proxy->>Proxy: Set RUNTIME_CONFIG global
    Proxy->>TracingMgr: Update with LogConfig

    Proxy->>Runtime: Call run_orion()
    activate Runtime

    Runtime->>Runtime: launch_runtimes()
    Runtime->>Runtime: Compute thread allocation
    Runtime->>Runtime: Calculate core affinity

    loop For each worker thread
        Runtime->>Workers: Create Tokio runtime
        activate Workers
        Runtime->>Workers: Spawn thread with core affinity
        Workers->>Workers: Initialize listeners
        Workers->>Workers: Initialize clusters
        Workers->>Workers: Start accepting connections
    end

    Runtime->>Runtime: Spawn xDS handler (if configured)
    Runtime->>Runtime: Spawn admin API server
    Runtime->>Runtime: Setup signal handlers

    Runtime-->>Proxy: All workers running
    deactivate Runtime
```

## xDS Configuration Update Flow

This diagram shows how dynamic configuration updates flow through the system.

```mermaid
sequenceDiagram
    participant XDS as xDS Server<br/>(Control Plane)
    participant Client as DeltaDiscoveryClient
    participant Handler as XdsConfigurationHandler
    participant Converter as Type Converters
    participant Channels as Config Channels
    participant Workers as Worker Threads
    participant Listeners as ListenersManager
    participant Clusters as ClusterManager
    participant Secrets as SecretManager

    Handler->>Handler: resolve_endpoints()
    Handler->>Handler: Find xDS cluster from config
    Handler->>Client: Connect to xDS server
    activate Client
    Client->>XDS: Subscribe to resources

    loop Configuration Updates
        XDS->>Client: DeltaDiscoveryResponse
        Client->>Client: Deserialize protobuf
        Client->>Handler: Send XdsResourcePayload
        deactivate Client

        activate Handler
        Handler->>Handler: Match resource type

        alt Listener Update
            Handler->>Converter: Convert Listener proto
            Converter-->>Handler: Listener config
            Handler->>Channels: Send ListenerConfigurationChange
            Channels->>Workers: Broadcast to all workers
            Workers->>Listeners: AddOrUpdate(Listener)
            Listeners->>Listeners: Rebind or update routes
        else Cluster Update
            Handler->>Converter: Convert Cluster proto
            Converter-->>Handler: Cluster config
            Handler->>Channels: Send ClusterConfigurationChange
            Channels->>Workers: Broadcast to all workers
            Workers->>Clusters: Update cluster endpoints
            Clusters->>Clusters: Start health checks
        else Route Update
            Handler->>Converter: Convert RouteConfig proto
            Converter-->>Handler: RouteConfiguration
            Handler->>Channels: Send RouteConfigurationChange
            Channels->>Workers: Broadcast to all workers
            Workers->>Listeners: Update route tables
        else Endpoint Update
            Handler->>Converter: Convert ClusterLoadAssignment
            Converter-->>Handler: Endpoints
            Handler->>Clusters: Update load assignment
            Clusters->>Clusters: Rebalance connections
        else Secret Update
            Handler->>Converter: Convert Secret proto
            Converter-->>Handler: TLS certificates/keys
            Handler->>Secrets: Update secrets
            Secrets->>Listeners: Reload TLS contexts
        end

        Handler->>Client: Send ACK
        activate Client
        Client->>XDS: ACK with version
        deactivate Client
        deactivate Handler
    end
```

## Request Processing Flow

This diagram shows the complete lifecycle of an HTTP request through the proxy.

```mermaid
sequenceDiagram
    participant Client as Client
    participant Listener as Listener
    participant FilterChain as Filter Chain Matcher
    participant TLS as TLS Handler
    participant HTTP as HTTP Handler
    participant Router as Route Matcher
    participant Filters as HTTP Filters
    participant Cluster as Cluster Manager
    participant LB as Load Balancer
    participant Upstream as Upstream Server
    participant AccessLog as Access Logger

    Client->>Listener: TCP connection
    activate Listener

    Listener->>FilterChain: Match connection
    activate FilterChain
    FilterChain->>FilterChain: Check SNI
    FilterChain->>FilterChain: Check ALPN
    FilterChain->>FilterChain: Check source IP
    FilterChain-->>Listener: Selected filter chain
    deactivate FilterChain

    alt TLS configured
        Listener->>TLS: TLS handshake
        activate TLS
        TLS->>TLS: Validate certificate
        TLS-->>Listener: TLS session
        deactivate TLS
    end

    Listener->>HTTP: Create HTTP handler
    activate HTTP
    Client->>HTTP: HTTP request

    HTTP->>AccessLog: InitContext(start_time)
    HTTP->>AccessLog: DownstreamContext(request)

    HTTP->>Router: Match route
    activate Router
    Router->>Router: Check path prefix
    Router->>Router: Check headers
    Router->>Router: Check methods
    Router-->>HTTP: Selected route
    deactivate Router

    HTTP->>Filters: Apply HTTP filters
    activate Filters
    Filters->>Filters: RBAC check
    Filters->>Filters: Rate limiting
    Filters->>Filters: Header manipulation
    Filters-->>HTTP: Filter result
    deactivate Filters

    HTTP->>Cluster: Get cluster
    activate Cluster
    HTTP->>AccessLog: UpstreamContext(cluster)

    Cluster->>LB: Select endpoint
    activate LB
    LB->>LB: Filter healthy endpoints
    LB->>LB: Apply locality weights
    LB->>LB: Round-robin selection
    LB-->>Cluster: Selected endpoint
    deactivate LB

    Cluster->>Upstream: Forward request
    activate Upstream
    Upstream-->>Cluster: Response
    deactivate Upstream
    deactivate Cluster

    HTTP->>AccessLog: DownstreamResponse(response)
    HTTP->>Client: Forward response
    HTTP->>AccessLog: FinishContext(duration, bytes)
    deactivate HTTP

    AccessLog->>AccessLog: Format log message
    AccessLog->>AccessLog: Write to log output

    deactivate Listener
```

## Access Logging Flow

This diagram details how access logs are generated and formatted.

```mermaid
graph TD
    subgraph "Request Processing"
        A[Request Start] --> B[InitContext]
        B --> C[DownstreamContext]
        C --> D[Route Matching]
        D --> E[UpstreamContext]
        E --> F[Forward to Cluster]
        F --> G[DownstreamResponse]
        G --> H[FinishContext]
    end

    subgraph "Log Formatting (orion-format)"
        I[LogFormatterLocal] --> J{Thread-local cache}
        J -->|Miss| K[Parse format string]
        J -->|Hit| L[Cached template]
        K --> L
        L --> M[Template tree]
    end

    subgraph "Context Evaluation"
        M --> N{For each Placeholder}
        N --> O[Evaluate Operator]
        O --> P{Category}
        P -->|Init| Q[Timestamp data]
        P -->|Downstream| R[Request data]
        P -->|Upstream| S[Cluster data]
        P -->|Response| T[Status data]
        P -->|Finish| U[Duration data]
        Q --> V[StringType result]
        R --> V
        S --> V
        T --> V
        U --> V
    end

    subgraph "Output"
        V --> W[FormattedMessage]
        W --> X{Output format}
        X -->|JSON| Y[JSON writer]
        X -->|Text| Z[Text writer]
        Y --> AA[Log file/stdout]
        Z --> AA
    end

    H --> I
    B --> Q
    C --> R
    E --> S
    G --> T
    H --> U

    style A fill:#e8f5e9
    style AA fill:#fff3e0
    style I fill:#f3e5f5
    style M fill:#e1f5ff
    style W fill:#fce4ec
```

## Cluster Selection & Load Balancing

This diagram shows the cluster selection and load balancing process.

```mermaid
graph TD
    A[HTTP Request] --> B{Route Matched}
    B --> C[Get Target Cluster]

    C --> D{Cluster Type}

    D -->|Static| E[StaticCluster]
    D -->|Dynamic| F[DynamicCluster]
    D -->|OriginalDst| G[OriginalDstCluster]

    E --> H[Static Endpoints]
    F --> I[DNS Resolution]
    I --> J[Resolved Endpoints]
    G --> K[Connection Metadata]
    K --> L[Original Destination]

    H --> M[Health Check Filter]
    J --> M
    L --> M

    M --> N{Healthy Endpoints}
    N -->|None| O[Connection Failed]
    N -->|Available| P[Locality Selection]

    P --> Q{Locality Weights}
    Q --> R[Select Locality]

    R --> S{Load Balancing Policy}

    S -->|Round Robin| T[Next in rotation]
    S -->|Least Request| U[Endpoint with fewest active]
    S -->|Random| V[Random selection]

    T --> W[Selected Endpoint]
    U --> W
    V --> W

    W --> X{Connection Pool}
    X -->|Exists| Y[Reuse connection]
    X -->|None| Z[Create new connection]

    Y --> AA[Forward Request]
    Z --> AA

    AA --> AB{Response Status}
    AB -->|Success| AC[Update Success Metrics]
    AB -->|Failure| AD[Update Failure Metrics]
    AD --> AE{Retry Policy}
    AE -->|Retry| M
    AE -->|No Retry| AF[Return Error]
    AC --> AG[Return Response]

    style A fill:#e8f5e9
    style W fill:#fff3e0
    style AA fill:#e1f5ff
    style AG fill:#e8f5e9
    style O fill:#ffebee
    style AF fill:#ffebee
```

## Component Interaction Overview

This high-level diagram shows how major components interact during runtime.

```mermaid
graph TB
    subgraph "Control Plane Communication"
        XDS[xDS Server]
        XDSClient[DeltaDiscoveryClient]
    end

    subgraph "Configuration Management"
        Bootstrap[Bootstrap Config]
        ConfigChannels[Configuration Channels]
        SecretMgr[Secret Manager]
    end

    subgraph "Worker Thread 1"
        L1[Listeners]
        C1[Clusters]
        R1[Routes]
    end

    subgraph "Worker Thread 2"
        L2[Listeners]
        C2[Clusters]
        R2[Routes]
    end

    subgraph "Worker Thread N"
        LN[Listeners]
        CN[Clusters]
        RN[Routes]
    end

    subgraph "Observability"
        Metrics[OpenTelemetry Metrics]
        Tracing[OpenTelemetry Tracing]
        AccessLog[Access Logs]
    end

    subgraph "Admin API"
        Admin[Admin Server]
    end

    XDS -.->|gRPC Stream| XDSClient
    XDSClient -->|Updates| ConfigChannels
    Bootstrap -->|Initial Config| ConfigChannels

    ConfigChannels -->|Listener Updates| L1
    ConfigChannels -->|Listener Updates| L2
    ConfigChannels -->|Listener Updates| LN

    ConfigChannels -->|Cluster Updates| C1
    ConfigChannels -->|Cluster Updates| C2
    ConfigChannels -->|Cluster Updates| CN

    ConfigChannels -->|Route Updates| R1
    ConfigChannels -->|Route Updates| R2
    ConfigChannels -->|Route Updates| RN

    ConfigChannels -->|Secret Updates| SecretMgr
    SecretMgr -.->|TLS Contexts| L1
    SecretMgr -.->|TLS Contexts| L2
    SecretMgr -.->|TLS Contexts| LN

    L1 -.->|Request Metrics| Metrics
    L2 -.->|Request Metrics| Metrics
    LN -.->|Request Metrics| Metrics

    L1 -.->|Trace Spans| Tracing
    L2 -.->|Trace Spans| Tracing
    LN -.->|Trace Spans| Tracing

    L1 -.->|Access Logs| AccessLog
    L2 -.->|Access Logs| AccessLog
    LN -.->|Access Logs| AccessLog

    Admin -.->|Query| ConfigChannels
    Admin -.->|Query| Metrics

    style XDS fill:#e8f5e9
    style Bootstrap fill:#fff3e0
    style L1 fill:#e1f5ff
    style L2 fill:#e1f5ff
    style LN fill:#e1f5ff
    style Metrics fill:#f3e5f5
    style Tracing fill:#f3e5f5
    style AccessLog fill:#f3e5f5
    style Admin fill:#fff9c4
```

## Configuration Channel Architecture

This diagram shows the async channel architecture for configuration distribution.

```mermaid
graph LR
    subgraph "Configuration Sources"
        Static[Static Config<br/>Bootstrap YAML]
        XDS[xDS Updates]
    end

    subgraph "Configuration Handler"
        Handler[XdsConfigurationHandler]
        Converter[Type Converters]
    end

    subgraph "Channel Distribution"
        ListenerChan[Listener Channel<br/>mpsc::Sender]
        RouteChan[Route Channel<br/>mpsc::Sender]
        ClusterChan[Cluster Channel<br/>mpsc::Sender]
    end

    subgraph "Worker Thread 1"
        ListenerRx1[Listener Receiver]
        RouteRx1[Route Receiver]
        ClusterRx1[Cluster Receiver]
        LM1[ListenersManager]
    end

    subgraph "Worker Thread 2"
        ListenerRx2[Listener Receiver]
        RouteRx2[Route Receiver]
        ClusterRx2[Cluster Receiver]
        LM2[ListenersManager]
    end

    subgraph "Worker Thread N"
        ListenerRxN[Listener Receiver]
        RouteRxN[Route Receiver]
        ClusterRxN[Cluster Receiver]
        LMN[ListenersManager]
    end

    Static --> Handler
    XDS --> Handler
    Handler --> Converter
    Converter -->|ListenerConfigurationChange| ListenerChan
    Converter -->|RouteConfigurationChange| RouteChan
    Converter -->|ClusterConfigurationChange| ClusterChan

    ListenerChan -.->|Clone & Broadcast| ListenerRx1
    ListenerChan -.->|Clone & Broadcast| ListenerRx2
    ListenerChan -.->|Clone & Broadcast| ListenerRxN

    RouteChan -.->|Clone & Broadcast| RouteRx1
    RouteChan -.->|Clone & Broadcast| RouteRx2
    RouteChan -.->|Clone & Broadcast| RouteRxN

    ClusterChan -.->|Clone & Broadcast| ClusterRx1
    ClusterChan -.->|Clone & Broadcast| ClusterRx2
    ClusterChan -.->|Clone & Broadcast| ClusterRxN

    ListenerRx1 --> LM1
    RouteRx1 --> LM1
    ClusterRx1 --> LM1

    ListenerRx2 --> LM2
    RouteRx2 --> LM2
    ClusterRx2 --> LM2

    ListenerRxN --> LMN
    RouteRxN --> LMN
    ClusterRxN --> LMN

    style Static fill:#fff3e0
    style XDS fill:#e8f5e9
    style Handler fill:#f3e5f5
    style ListenerChan fill:#e1f5ff
    style RouteChan fill:#e1f5ff
    style ClusterChan fill:#e1f5ff
    style LM1 fill:#fce4ec
    style LM2 fill:#fce4ec
    style LMN fill:#fce4ec
```

## Thread Architecture

This diagram illustrates the multi-threaded runtime architecture.

```mermaid
graph TB
    subgraph "Main Thread"
        Main[main.rs]
        ProxyInit[Proxy Initialization]
        RuntimeLauncher[Runtime Launcher]
    end

    subgraph "Configuration Thread"
        XDSHandler[xDS Configuration Handler]
        XDSClient[Delta Discovery Client]
        ConfigProcessor[Config Update Processor]
    end

    subgraph "Admin Thread"
        AdminAPI[Admin API Server]
        ConfigDump[Config Dump Endpoint]
        MetricsEndpoint[Metrics Endpoint]
    end

    subgraph "Worker Thread 1 (Core 0)"
        Runtime1[Tokio Runtime 1]
        Listener1[Listener Tasks]
        Cluster1[Cluster Tasks]
        Health1[Health Check Tasks]
    end

    subgraph "Worker Thread 2 (Core 1)"
        Runtime2[Tokio Runtime 2]
        Listener2[Listener Tasks]
        Cluster2[Cluster Tasks]
        Health2[Health Check Tasks]
    end

    subgraph "Worker Thread N (Core N-1)"
        RuntimeN[Tokio Runtime N]
        ListenerN[Listener Tasks]
        ClusterN[Cluster Tasks]
        HealthN[Health Check Tasks]
    end

    subgraph "Shared State (Thread-Safe)"
        Secrets[Arc RwLock SecretManager]
        GlobalConfig[Arc-swap Config]
        Tracers[Arc-swap Tracers]
        Interner[Global String Interner]
    end

    Main --> ProxyInit
    ProxyInit --> RuntimeLauncher
    RuntimeLauncher -->|Spawn| XDSHandler
    RuntimeLauncher -->|Spawn| AdminAPI
    RuntimeLauncher -->|Spawn with affinity| Runtime1
    RuntimeLauncher -->|Spawn with affinity| Runtime2
    RuntimeLauncher -->|Spawn with affinity| RuntimeN

    XDSHandler --> XDSClient
    XDSClient --> ConfigProcessor
    ConfigProcessor -.->|Update| GlobalConfig
    ConfigProcessor -.->|Update| Secrets

    AdminAPI --> ConfigDump
    AdminAPI --> MetricsEndpoint
    ConfigDump -.->|Read| GlobalConfig
    MetricsEndpoint -.->|Read| GlobalConfig

    Runtime1 --> Listener1
    Runtime1 --> Cluster1
    Runtime1 --> Health1

    Runtime2 --> Listener2
    Runtime2 --> Cluster2
    Runtime2 --> Health2

    RuntimeN --> ListenerN
    RuntimeN --> ClusterN
    RuntimeN --> HealthN

    Listener1 -.->|Read| Secrets
    Listener2 -.->|Read| Secrets
    ListenerN -.->|Read| Secrets

    Listener1 -.->|Use| Interner
    Listener2 -.->|Use| Interner
    ListenerN -.->|Use| Interner

    Listener1 -.->|Use| Tracers
    Listener2 -.->|Use| Tracers
    ListenerN -.->|Use| Tracers

    style Main fill:#fff9c4
    style XDSHandler fill:#e8f5e9
    style AdminAPI fill:#f3e5f5
    style Runtime1 fill:#e1f5ff
    style Runtime2 fill:#e1f5ff
    style RuntimeN fill:#e1f5ff
    style Secrets fill:#ffebee
    style GlobalConfig fill:#ffebee
    style Tracers fill:#ffebee
    style Interner fill:#ffebee
```

## Summary

These diagrams provide a comprehensive visual representation of the Orion-Kmesh architecture:

1. **Dependency graphs** show the modular structure and crate relationships
2. **Flow diagrams** illustrate the sequence of operations for key scenarios
3. **Component diagrams** demonstrate runtime interactions and data flow
4. **Architecture diagrams** reveal the thread model and shared state management

Together with the [architecture documentation](./architecture.md), these diagrams provide a complete picture of how Orion-Kmesh is structured and operates.
