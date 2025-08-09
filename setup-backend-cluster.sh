#!/bin/bash

# Orion Proxy Backend Setup Script
# This script demonstrates Orion Proxy with working backend servers

set -e

echo "🚀 Setting up Orion Proxy with backend cluster..."

# Function to cleanup on exit
cleanup() {
    echo "🧹 Cleaning up..."
    docker rm -f backend1 backend2 orion-proxy 2>/dev/null || true
    echo "✅ Cleanup complete"
}

# Set trap to cleanup on script exit
trap cleanup EXIT

# Build Orion image if needed
if ! docker images | grep -q "orion-proxy"; then
    echo "🔨 Building Orion Proxy image..."
    docker build -t orion-proxy -f docker/Dockerfile . > /dev/null
fi

# Start backend servers on host ports
echo "📦 Starting backend servers..."
docker run -d -p 4001:80 --name backend1 nginx:alpine
docker run -d -p 4002:80 --name backend2 nginx:alpine

# Give backends time to start
sleep 3

echo "🔧 Backend servers started:"
echo "  - Backend 1: http://localhost:4001"
echo "  - Backend 2: http://localhost:4002"

# Test backends directly
echo "🧪 Testing backends directly..."
if curl -s http://localhost:4001 > /dev/null; then
    echo "✅ Backend 1 is responding"
else
    echo "❌ Backend 1 failed"
    exit 1
fi

if curl -s http://localhost:4002 > /dev/null; then
    echo "✅ Backend 2 is responding" 
else
    echo "❌ Backend 2 failed"
    exit 1
fi

# Start Orion Proxy with original configuration but using host networking
# This allows the proxy to reach localhost:4001/4002
echo "🌟 Starting Orion Proxy..."
docker run -d --network host --name orion-proxy orion-proxy

# Wait for proxy to start
echo "⏳ Waiting for Orion Proxy to start..."
sleep 5

# Test direct response endpoint
echo "🧪 Testing Orion Proxy endpoints..."
echo -n "  Direct response: "
if response=$(curl -s http://localhost:8000/direct-response 2>/dev/null) && [[ "$response" == "meow! 🐱" ]]; then
    echo "✅ PASS"
else
    echo "❌ FAIL - Response: '$response'"
fi

# Test load balancing - should get nginx welcome page
echo -n "  Load balancing:  "
if response=$(curl -s http://localhost:8000/ 2>/dev/null) && [[ "$response" == *"nginx"* ]]; then
    echo "✅ PASS - Successfully proxied to nginx backend!"
else
    echo "⚠️  Check manually: curl http://localhost:8000/"
fi

echo ""
echo "🎉 Setup complete! Test your setup:"
echo "  Direct response: curl http://localhost:8000/direct-response"
echo "  Load balanced:   curl http://localhost:8000/"
echo "  Backend 1 direct: curl http://localhost:4001/"  
echo "  Backend 2 direct: curl http://localhost:4002/"
echo ""
echo "📊 View logs:"
echo "  docker logs orion-proxy"
echo "  docker logs backend1"
echo ""
echo "Press Ctrl+C to stop all services and cleanup..."
echo ""

# Show real-time logs
echo "📋 Orion Proxy logs (press Ctrl+C to exit):"
docker logs -f orion-proxy
