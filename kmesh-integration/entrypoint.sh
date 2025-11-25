#!/bin/bash
# Orion startup script that substitutes environment variables in config

set -e

if [ -z "${NODE_ID}" ]; then
    export NODE_ID="waypoint~${POD_IP}~${POD_NAME}.${NAMESPACE}~${NAMESPACE}.svc.cluster.local"
fi

echo "Starting Orion with NODE_ID: ${NODE_ID}"

envsubst < /etc/orion/config.yaml > /tmp/orion-config-processed.yaml

exec /usr/local/bin/orion --config /tmp/orion-config-processed.yaml
