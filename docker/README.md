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

## Port Reference

| Port  | Purpose                   | Required for Basic Setup |
|-------|---------------------------|---------------------------|
| 8000  | HTTP Listener             | ✅ Yes (primary service)   |
| 8001  | TCP Listener              | ❌ Only if using TCP proxy |
| 50051 | xDS gRPC Configuration    | ❌ Only with external xDS  |

> **Note**: Only port 8000 is required for basic HTTP proxy functionality. Map additional ports only when needed for specific features.
