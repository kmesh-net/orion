# Internal Listener and Upstream Transport Demo

This demo demonstrates Orion's internal listener and upstream transport functionality for service mesh communication.

## Quick Test

```bash
./test_internal_config.sh
```

## Configuration

### Internal Listener

```yaml
listeners:
  - name: internal_mesh_listener
    address:
      internal:
        buffer_size_kb: 1024
    filter_chains:
      - name: internal_proxy_chain
        terminal_filter:
          tcp_proxy:
            cluster: internal_backend_cluster
```

### Internal Endpoints

```yaml
clusters:
  - name: internal_service_cluster
    type: STATIC
    load_assignment:
      endpoints:
        - lb_endpoints:
            - endpoint:
                address:
                  internal:
                    server_listener_name: internal_mesh_listener
                    endpoint_id: service_a_endpoint_1
```

### Internal Upstream Transport

```yaml
transport_socket:
  internal_upstream:
    passthrough_metadata:
      - kind: HOST
        name: envoy.filters.listener.original_dst
    transport_socket:
      raw_buffer: {}
```

### Bootstrap Extensions

```yaml
bootstrap_extensions:
  - internal_listener:
      buffer_size_kb: 2048
```

## Usage

```bash
# Start Orion
../../target/debug/orion -c orion-config.yaml

# Test endpoints
curl http://localhost:10000/
curl http://localhost:10000/service-a

# Monitor
curl http://localhost:9901/stats
```

## Features

- Internal listeners for service mesh communication
- Internal endpoints with server_listener_name references
- Metadata passthrough via internal upstream transport
- Global bootstrap extensions configuration
