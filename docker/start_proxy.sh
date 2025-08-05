#!/bin/bash
set -e

if [[ -n "${CONTROL_PLANE_IP}" ]]; then
  sed -i "s|CONTROL_PLANE_IP|${CONTROL_PLANE_IP}|g" /etc/orion/orion-runtime.yaml
fi

export RUST_BACKTRACE=1
exec /orion --config /etc/orion/orion-runtime.yaml