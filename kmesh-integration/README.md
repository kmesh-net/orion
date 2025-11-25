# Orion as Kmesh Waypoint 

##  Directory Structure

```
kmesh-integration/
├── README.md                        # This file
├── Dockerfile                       # Orion waypoint container image
├── orion                            # Pre-built Orion binary
├── entrypoint.sh                    # Container entrypoint script
├── config/
│   └── orion-waypoint.yaml          # Orion configuration
├── docs/
│   └── KMESH-FULL-INTEGRATION.md    # Complete verification & architecture
├── scripts/
│   ├── setup-kmesh-kind.sh          # Automated setup script ( Start here!)
│   └── test-kmesh-full.sh           # Comprehensive test suite
└── yamls/
    ├── orion-deployment.yaml        # Orion waypoint deployment
    └── orion-service.yaml           # Orion waypoint service
```

##  Quick Start 

### Prerequisites

- **kind cluster** running (v1.31.0+)
- **kubectl** configured
- **istioctl** (v1.23+) - [Install guide](https://istio.io/latest/docs/setup/getting-started/#download)
- **helm** (v3+)
- **Docker** running

### One-Command Setup

```bash
cd kmesh-integration
./scripts/setup-kmesh-kind.sh
```

### Run Tests

```bash
./scripts/test-kmesh-full.sh
```

## Verification

### Check Kmesh Integration

```bash
# Verify namespace label
kubectl get namespace bookinfo -o jsonpath='{.metadata.labels.istio\.io/dataplane-mode}'
# Output: Kmesh

# Verify pod annotation (PROOF OF eBPF)
kubectl get pod -n bookinfo -l app=productpage \
  -o jsonpath='{.items[0].metadata.annotations.kmesh\.net/redirection}'
# Output: enabled

# Check Kmesh daemon
kubectl get pods -n kmesh-system
# Output: kmesh-xxxxx (Running)
```

### Test Traffic

```bash
# Send request through Orion waypoint
kubectl exec -n bookinfo deploy/sleep -- \
  curl -v http://productpage:9080/productpage

# Check Orion logs
kubectl logs -n bookinfo -l app=orion-waypoint --tail=50
```

### Check Orion Listeners

```bash
kubectl logs -n bookinfo -l app=orion-waypoint | grep "Started version"
```

Expected output:
```
Started version 11 of listener main_internal
Started version 12 of listener connect_originate
Started version 10 of listener connect_terminate
```

### Check Logs
```bash
# Kmesh logs
kubectl logs -n kmesh-system -l app=kmesh --tail=50

# Orion logs
kubectl logs -n bookinfo -l app=orion-waypoint --tail=50

# Istio logs
kubectl logs -n istio-system -l app=istiod --tail=50
```