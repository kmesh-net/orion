FROM ubuntu:24.04

RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    gettext-base \
    && rm -rf /var/lib/apt/lists/*

RUN useradd -m -u 1337 -s /bin/bash istio-proxy && \
    mkdir -p /var/log/orion /etc/orion && \
    chown -R istio-proxy:istio-proxy /var/log/orion /etc/orion

WORKDIR /home/istio-proxy

COPY orion /usr/local/bin/orion
RUN chmod +x /usr/local/bin/orion

COPY entrypoint.sh /usr/local/bin/entrypoint.sh
RUN chmod +x /usr/local/bin/entrypoint.sh

COPY config/orion-waypoint.yaml /etc/orion/config.yaml

USER istio-proxy

# Expose ports
# 15000: Admin interface
# 15006: Inbound traffic
# 15008: HBONE tunnel
# 15020: Health check
# 15021: Status port
# 15090: Prometheus metrics
EXPOSE 15000 15006 15008 15020 15021 15090

ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
