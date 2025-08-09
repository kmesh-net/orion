# Orion Docker Setup

This document provides detailed instructions for building and running Orion using Docker.

## Building the Image

```bash
docker build -t orion-proxy -f docker/Dockerfile .
```

## Running the Container

### Basic Setup

Use port mapping:

```bash
docker run -d \
  -p 8000:8000 \
  --name orion-proxy \
  orion-proxy
```

For full functionality (including TCP proxy):

```bash
docker run -d \
  -p 8000:8000 \
  -p 8001:8001 \
  --name orion-proxy \
  orion-proxy
```

Alternative with host networking:

```bash
docker run -d \
  --network host \
  --name orion-proxy \
  orion-proxy
```

> **Note**: Host networking may not work in all Docker environments (e.g., Docker Desktop on macOS/Windows). Use port mapping for reliable access to services.

## Configuration Options

### Custom Configuration File

Mount your own configuration file:

```bash
docker run -d \
  --network host \
  -v /path/to/your/config.yaml:/etc/orion/orion-runtime.yaml \
  --name orion-proxy \
  orion-proxy
```

### Control Plane Setup

Configure with external control plane:

```bash
docker run -d \
  --network host \
  -e CONTROL_PLANE_IP=<your_control_plane_ip> \
  --name orion-proxy \
  orion-proxy
```

### Environment Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `CONTROL_PLANE_IP` | IP address of the control plane | `127.0.0.1` |
| `LOG_LEVEL` | Logging level (debug, info, warn, error) | `info` |

## Verification

### Service Health Check
Once the container is running, verify the HTTP listener is responding.

**If using port mapping (`-p 8000:8000`):**
```bash
curl -v http://localhost:8000/direct-response # Should return HTTP 200 with "meow! 🐱"
```

**If using host networking (`--network host`) and it's working:**
```bash
curl -v http://localhost:8000/direct-response # Should return HTTP 200 with "meow! 🐱"
```

### Load Balancing Test
Test the default route that forwards to the configured backend cluster:

```bash
curl -v http://localhost:8000/ # Will attempt to proxy to backend (may timeout if backends aren't running)
```

### Setting up Backend Servers for Testing

The default configuration points to backend servers at `10.206.137.58:4001` and `10.206.137.58:4002`. To test with actual backends, you can:

1. **Update the configuration** by mounting a custom config file with your backend IPs:
   ```bash
   # Edit your local config file to point to your backend servers
   cp orion-proxy/conf/orion-runtime.yaml my-config.yaml
   # Edit my-config.yaml and update the cluster endpoints addresses and ports
   
   # Run with custom config
   docker run -d \
     -p 8000:8000 \
     -v $(pwd)/my-config.yaml:/etc/orion/orion-runtime.yaml \
     --name orion-proxy \
     orion-proxy
   ```

2. **Start simple test backends** using Docker:
   ```bash
   # Start test HTTP servers
   docker run -d -p 4001:80 --name backend1 nginx:alpine
   docker run -d -p 4002:80 --name backend2 nginx:alpine
   
   # Update config to use localhost:4001 and localhost:4002, then restart orion
   ```

For production deployment, update the cluster configuration in `orion-runtime.yaml` to point to your actual backend service IPs and ports.

## Port Reference

| Port  | Purpose                   | Required for Basic Setup |
|-------|---------------------------|---------------------------|
| 8000  | HTTP Listener             | ✅ Yes (primary service)   |
| 8001  | TCP Listener              | ❌ Only if using TCP proxy |
| 50051 | xDS gRPC Configuration    | ❌ Only with external xDS  |

> **Note**: Only port 8000 is required for basic HTTP proxy functionality. Map additional ports only when needed for specific features.
