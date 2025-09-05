# TLV Filter Demo

This demo showcases Orion's TLV (Type-Length-Value) listener filter implementation, compatible with Kmesh configurations.

## Quick Test

Run the automated test script:

```bash
./test_tlv_config.sh
```

This script:
1. Validates Orion configuration loading
2. Tests TLV filter integration
3. Optionally tests end-to-end packet processing (if client is available)

## Configuration

The TLV filter is configured in `orion-config.yaml`:

```yaml
listeners:
  - name: tlv_demo_listener
    address: 0.0.0.0:9000
    filter_chains:
      - filters:
          - name: envoy.listener.kmesh_tlv
            typed_config:
              "@type": type.googleapis.com/envoy.extensions.filters.listener.kmesh_tlv.v3.KmeshTlv
```

## Manual Testing

### Start Orion
```bash
../../target/debug/orion -c orion-config.yaml
```

### Send TLV Packets
```bash
rustc send_tlv.rs -o send_tlv
./send_tlv <ip> <port>
```

The client constructs TLV packets containing original destination information and sends them to the Orion listener for testing the filter's TLV processing capabilities.

## TLV Client

The included Rust client (`send_tlv.rs`) is used for testing the TLV filter:

- **Purpose**: Constructs and sends TLV packets to test the filter's processing
- **Functionality**: Encodes original destination IP/port in TLV format and sends to Orion listener
- **Usage**: Compile with `rustc send_tlv.rs -o send_tlv` then run `./send_tlv <ip> <port>`
- **Protocol**: Supports both IPv4 and IPv6 addresses in TLV packets

### Protocol Buffer
```protobuf
syntax = "proto3";
package envoy.extensions.filters.listener.kmesh_tlv.v3;

message KmeshTlv {}
```

### Configuration Parameters
- **Filter Name**: `envoy.listener.kmesh_tlv`
- **Type URL**: `type.googleapis.com/envoy.extensions.filters.listener.kmesh_tlv.v3.KmeshTlv`

### TLV Protocol Support
- **TLV_TYPE_SERVICE_ADDRESS (0x1)**: Service address information
- **TLV_TYPE_ENDING (0xfe)**: End marker
- **Maximum TLV Length**: 256 bytes

## Architecture

```
Client → Orion Listener → TLV Filter → Filter Chains → Backend
                           ↓
                   TLV Processing
              (Extract original destination)
```

## Kmesh Compatibility

Fully compatible with Kmesh project configurations using identical protobuf package names and TypedStruct configuration pattern.
