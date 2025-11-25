# Kmesh Integration Setup Guide

## Prerequisites

Navigate to the kmesh-integration directory:

```bash
cd kmesh-integration
```

## Setup Steps

### 1. Setup Kmesh Kind Cluster (Optional)

If you don't want to enter commands manually, run the setup script:

```bash
./scripts/setup-kmesh-kind.sh
```

### 2. Configure Kubernetes Context

```bash
kubectl config use-context kind-kind
```

### 3. Enable Kmesh Dataplane Mode

Label the bookinfo namespace to use Kmesh:

```bash
kubectl label namespace bookinfo istio.io/dataplane-mode=Kmesh --overwrite
```

### 4. Build and Load Orion Waypoint Image

Build the Docker image:

```bash
docker build -t orion-waypoint:latest .
```

Load the image into the kind cluster:

```bash
kind load docker-image orion-waypoint:latest
```

### 5. Deploy Orion Waypoint

Apply the deployment configuration:

```bash
kubectl apply -f yamls/orion-deployment.yaml -n bookinfo
```

Apply the service configuration:

```bash
kubectl apply -f yamls/orion-service.yaml -n bookinfo
```

Wait for the pods to be ready:

```bash
kubectl wait --for=condition=ready pod -l app=orion-waypoint -n bookinfo --timeout=60s
```

### 6. Configure Waypoint for ProductPage

Label the productpage service to use the orion-waypoint:

```bash
kubectl label service productpage -n bookinfo istio.io/use-waypoint=orion-waypoint --overwrite
```

## Verification

### Check Kmesh Redirection

Verify the Kmesh redirection annotation on the productpage pod:

```bash
kubectl get pod -n bookinfo -l app=productpage -o jsonpath='{.items[0].metadata.annotations.kmesh\.net/redirection}' && echo
```

### Test HTTP Connection

Check if the productpage is accessible and returns a successful status code:

```bash
echo "Status code: $(kubectl exec -n bookinfo deploy/sleep -- curl -s -o /dev/null -w "%{http_code}" http://productpage:9080/productpage)"
```

Expected output: `Status code: 200`

### Check Orion Waypoint Logs

View the logs to confirm the waypoint is running:

```bash
kubectl logs -n bookinfo -l app=orion-waypoint | grep "Started version"
```

## Full Testing

For comprehensive testing, run the full test script:

```bash
./scripts/test-kmesh-full.sh
```