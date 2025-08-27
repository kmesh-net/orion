### Description

This document describes the statistics available in Orion proxy. The metrics are grouped by their functionality, such as listener stats, TLS statistics, HTTP connection stats, TCP proxy stats, cluster stats, health check statistics, and server statistics.

To enable metrics, you have to build Orion with the feature `metrics` enabled. By default, Orion does support OpenTelemetry for metrics, which allows you to export metrics to various backends like Prometheus, StatsD, or any OpenTelemetry collector.

### Basic Configuration

To enable metrics, add the following configuration to your `orion.yaml` file:

```yaml
envoy_bootstrap:
  stats_flush_interval: 10s

  stats_sinks:
    - name: envoy.stat_sinks.metrics_service
      typed_config:
          "@type": type.googleapis.com/envoy.extensions.stat_sinks.open_telemetry.v3.SinkConfig
          grpc_service:
              google_grpc:
                  target_uri: "http://192.168.0.2:4317" # open-telemetry collector address
```

### Prometheus scrape configuration

In addition to OpenTelemetry, Orion supports Prometheus scraping out of the box. To enable it, you must build Orion with the `prometheus` feature.
This means compiling it with `--features metrics,prometheus`.

Next, enable the admin interface in your `orion.yaml` file as follows:

```yaml
envoy_bootstrap:
    admin:
      address:
        socketAddress:
          address: "127.0.0.1"
          portValue: 9901
```

This will expose metrics at `http://127.0.0.1:9901/stats/prometheus`.
You can combine both configurations to export metrics to OpenTelemetry and scrape them with Prometheus simultaneously.

### Status Legend

| Status | Description |
| :---   | :--- |
| ✅     | Available |
| ❌      | Not applicable for Orion |
| 🚧     | Work in progress |


### Listener stats
| Name | Type | Status | Description |
| :--- | :--- | :--- | :--- |
| `downstream_cx_total` | Counter | ✅ | Total connections |
| `downstream_cx_destroy` | Counter | ✅ | Total destroyed connections |
| `downstream_cx_active` | Gauge | ✅ | Total active connections |
| `downstream_cx_length_ms` | Histogram | ✅ | Connection length milliseconds |
| `downstream_cx_transport_socket_connect_timeout` | Counter | 🚧 | Total connections that timed out during transport socket connection negotiation |
| `downstream_cx_overflow` | Counter | | Total connections rejected due to enforcement of listener connection limit |
| `downstream_cx_overload_reject` | Counter | | Total connections rejected due to configured overload actions |
| `downstream_global_cx_overflow` | Counter | | Total connections rejected due to enforcement of global connection limit |
| `connections_accepted_per_socket_event` | Histogram | ❌ | Number of connections accepted per listener socket event |
| `downstream_pre_cx_timeout` | Counter | 🚧 | Sockets that timed out during listener filter processing |
| `downstream_pre_cx_active` | Gauge | | Sockets currently undergoing listener filter processing |
| `extension_config_missing` | Counter | 🚧 | Total connections closed due to missing listener filter extension configuration |
| `network_extension_config_missing` | Counter | | Total connections closed due to missing network filter extension configuration |
| `no_filter_chain_match` | Counter |  ✅ | Total connections that didn’t match any filter chain |
| `downstream_listener_filter_remote_close` | Counter | 🚧 | Total connections closed by remote when peek data for listener filters |
| `downstream_listener_filter_error` | Counter | 🚧 | Total numbers of read errors when peeking data for listener filters |

### TLS statistics
| Name | Type | Status | Description |
| :--- | :--- | :--- | :--- |
| `connection_error` | Counter | | Total TLS connection errors not including failed certificate verifications |
| `handshake` | Counter | ✅ | Total successful TLS connection handshakes |
| `session_reused` | Counter | | Total successful TLS session resumptions |
| `no_certificate` | Counter | | Total successful TLS connections with no client certificate |
| `fail_verify_no_cert` | Counter | | Total TLS connections that failed because of missing client certificate |
| `fail_verify_error` | Counter | | Total TLS connections that failed CA verification |
| `fail_verify_san` | Counter | | Total TLS connections that failed SAN verification |
| `fail_verify_cert_hash` | Counter | | Total TLS connections that failed certificate pinning verification |
| `ocsp_staple_failed` | Counter | | Total TLS connections that failed compliance with the OCSP policy |
| `ocsp_staple_omitted` | Counter | | Total TLS connections that succeeded without stapling an OCSP response |
| `ocsp_staple_responses` | Counter | | Total TLS connections where a valid OCSP response was available (irrespective of whether the client requested stapling) |
| `ocsp_staple_requests` | Counter | | Total TLS connections where the client requested an OCSP staple |
| `ciphers.<cipher>` | Counter | | Total successful TLS connections that used cipher <cipher> |
| `curves.<curve>` | Counter | | Total successful TLS connections that used ECDHE curve <curve> |
| `sigalgs.<sigalg>` | Counter | | Total successful TLS connections that used signature algorithm <sigalg> |
| `versions.<version>` | Counter | | Total successful TLS connections that used protocol version <version> |
| `was_key_usage_invalid` | Counter | | Total successful TLS connections that used an invalid keyUsage extension. (This is not available in BoringSSL FIPS yet due to issue #28246) |

### http conn stats
| Name | Type | Status | Description |
| :--- | :--- | :--- | :--- |
| `downstream_cx_total` | Counter | ✅ | Total connections |
| `downstream_cx_ssl_total` | Counter | ✅ | Total TLS connections |
| `downstream_cx_http1_total` | Counter | | Total HTTP/1.1 connections |
| `downstream_cx_upgrades_total` | Counter | | Total successfully upgraded connections. These are also counted as total http1/http2 connections. |
| `downstream_cx_http2_total` | Counter | | Total HTTP/2 connections |
| `downstream_cx_http3_total` | Counter | | Total HTTP/3 connections |
| `downstream_cx_destroy` | Counter |  ✅  | Total connections destroyed |
| `downstream_cx_destroy_remote` | Counter | 🚧 | Total connections destroyed due to remote close |
| `downstream_cx_destroy_local` | Counter | 🚧 | Total connections destroyed due to local close |
| `downstream_cx_destroy_active_rq` | Counter | | Total connections destroyed with 1+ active request |
| `downstream_cx_destroy_local_active_rq` | Counter | | Total connections destroyed locally with 1+ active request |
| `downstream_cx_destroy_remote_active_rq` | Counter | | Total connections destroyed remotely with 1+ active request |
| `downstream_cx_active` | Gauge |  ✅  | Total active connections |
| `downstream_cx_ssl_active` |  ✅ | | Total active TLS connections |
| `downstream_cx_http1_active` | Gauge | | Total active HTTP/1.1 connections |
| `downstream_cx_upgrades_active` | Gauge | | Total active upgraded connections. These are also counted as active http1/http2 connections. |
| `downstream_cx_http1_soft_drain` | Gauge | | Total active HTTP/1.x connections waiting for another downstream request to safely close the connection. |
| `downstream_cx_http2_active` | Gauge | | Total active HTTP/2 connections |
| `downstream_cx_http3_active` | Gauge | | Total active HTTP/3 connections |
| `downstream_cx_protocol_error` | Counter | | Total protocol errors |
| `downstream_cx_length_ms` | Histogram |  ✅ | Connection length milliseconds |
| `downstream_cx_rx_bytes_total` | Counter | ✅ | Total bytes received |
| `downstream_cx_rx_bytes_buffered` | Gauge | | Total received bytes currently buffered |
| `downstream_cx_tx_bytes_total` | Counter | ✅ | Total bytes sent |
| `downstream_cx_tx_bytes_buffered` | Gauge | | Total sent bytes currently buffered |
| `downstream_cx_drain_close` | Counter | | Total connections closed due to draining |
| `downstream_cx_idle_timeout` | Counter | | Total connections closed due to idle timeout |
| `downstream_cx_max_duration_reached` | Counter | | Total connections closed due to max connection duration |
| `downstream_cx_max_requests_reached` | Counter | | Total connections closed due to max requests per connection |
| `downstream_cx_overload_disable_keepalive` | Counter | | Total connections for which HTTP 1.x keepalive has been disabled due to Envoy overload |
| `downstream_flow_control_paused_reading_total` | Counter | | Total number of times reads were disabled due to flow control |
| `downstream_flow_control_resumed_reading_total` | Counter | | Total number of times reads were enabled on the connection due to flow control |
| `downstream_rq_total` | Counter | ✅  | Total requests |
| `downstream_rq_http1_total` | Counter | | Total HTTP/1.1 requests |
| `downstream_rq_http2_total` | Counter | | Total HTTP/2 requests |
| `downstream_rq_http3_total` | Counter | | Total HTTP/3 requests |
| `downstream_rq_active` | Gauge | ✅ | Total active requests |
| `downstream_rq_rejected_via_ip_detection` | Counter | | Total requests rejected because the original IP detection failed |
| `downstream_rq_response_before_rq_complete` | Counter | | Total responses sent before the request was complete |
| `downstream_rq_rx_reset` | Counter | | Total request resets received |
| `downstream_rq_tx_reset` | Counter | | Total request resets sent |
| `downstream_rq_non_relative_path` | Counter | | Total requests with a non-relative HTTP path |
| `downstream_rq_too_large` | Counter | | Total requests resulting in a 413 due to buffering an overly large body |
| `downstream_rq_completed` | Counter | | Total requests that resulted in a response (e.g. does not include aborted requests) |
| `downstream_rq_failed_path_normalization` | Counter | | Total requests redirected due to different original and normalized URL paths or when path normalization failed. This action is configured by setting the path_with_escaped_slashes_action config option. |
| `downstream_rq_1xx` | Counter | ✅ | Total 1xx responses |
| `downstream_rq_2xx` | Counter | ✅ | Total 2xx responses |
| `downstream_rq_3xx` | Counter | ✅ | Total 3xx responses |
| `downstream_rq_4xx` | Counter | ✅ | Total 4xx responses |
| `downstream_rq_5xx` | Counter | ✅ | Total 5xx responses |
| `downstream_rq_ws_on_non_ws_route` | Counter | | Total upgrade requests rejected by non upgrade routes. This now applies both to WebSocket and non-WebSocket upgrades |
| `downstream_rq_time` | Histogram | | Total time for request and response (milliseconds) |
| `downstream_rq_idle_timeout` | Counter | | Total requests closed due to idle timeout |
| `downstream_rq_max_duration_reached` | Counter | | Total requests closed due to max duration reached |
| `downstream_rq_timeout` | Counter | | Total requests closed due to a timeout on the request path |
| `downstream_rq_overload_close` | Counter | | Total requests closed due to Envoy overload |
| `downstream_rq_redirected_with_normalized_path` | Counter | | Total requests redirected due to different original and normalized URL paths. This action is configured by setting the path_with_escaped_slashes_action config option. |
| `downstream_rq_too_many_premature_resets` | Counter | | Total number of connections closed due to too many premature request resets on the connection. |
| `rs_too_large` | Counter | | Total response errors due to buffering an overly large body |

### Tcp proxy stats
| Name | Type | Status | Description |
|------|------|--------|-------------|
| downstream_cx_total | Counter | ✅ | Total connections |
| downstream_cx_destroy | Counter | ✅ | Total destroyed connections |
| downstream_cx_active | Gauge | ✅ | Total active connections |
| downstream_cx_length_ms | Histogram | ✅ | Connection length milliseconds |
| downstream_cx_no_route | Counter | | Number of connections for which no matching route was found or the cluster for the route was not found |
| downstream_cx_tx_bytes_total | Counter | | Total bytes written to the downstream connection |
| downstream_cx_tx_bytes_buffered | Gauge | | Total bytes currently buffered to the downstream connection |
| downstream_cx_rx_bytes_total | Counter | | Total bytes read from the downstream connection |
| downstream_cx_rx_bytes_buffered | Gauge | | Total bytes currently buffered from the downstream connection |
| downstream_flow_control_paused_reading_total | Counter | | Total number of times flow control paused reading from downstream |
| downstream_flow_control_resumed_reading_total | Counter | | Total number of times flow control resumed reading from downstream |
| early_data_received_count_total | Counter | | Total number of connections where tcp proxy received data before upstream connection establishment is complete |
| idle_timeout | Counter | | Total number of connections closed due to idle timeout |
| max_downstream_connection_duration | Counter | | Total number of connections closed due to max_downstream_connection_duration timeout |
| on_demand_cluster_attempt | Counter | | Total number of connections that requested on demand cluster |
| on_demand_cluster_missing | Counter | | Total number of connections closed due to on demand cluster is missing |
| on_demand_cluster_success | Counter | | Total number of connections that requested and received on demand cluster |
| on_demand_cluster_timeout | Counter | | Total number of connections closed due to on demand cluster lookup timeout |
| upstream_flush_total | Counter | | Total number of connections that continued to flush upstream data after the downstream connection was closed |
| upstream_flush_active | Gauge | | Total connections currently continuing to flush upstream data after the downstream connection was closed |

### cluster stats
| Name | Type | Status | Description |
| :--- | :--- | :--- | :--- |
| `upstream_cx_total` | Counter | ✅ |  Total connections |
| `upstream_cx_active` | Gauge | ✅ | Total active connections |
| `upstream_cx_http1_total` | Counter | | Total HTTP/1.1 connections |
| `upstream_cx_http2_total` | Counter | | Total HTTP/2 connections |
| `upstream_cx_http3_total` | Counter | | Total HTTP/3 connections |
| `upstream_cx_connect_fail` | Counter | ✅ | Total connection failures |
| `upstream_cx_connect_timeout` | Counter | ✅ | Total connection connect timeouts |
| `upstream_cx_connect_with_0_rtt` | Counter | | Total connections able to send 0-rtt requests (early data). |
| `upstream_cx_idle_timeout` | Counter | ✅ | Total connection idle timeouts |
| `upstream_cx_max_duration_reached` | Counter | | Total connections closed due to max duration reached |
| `upstream_cx_connect_attempts_exceeded` | Counter | | Total consecutive connection failures exceeding configured connection attempts |
| `upstream_cx_overflow` | Counter | | Total times that the cluster’s connection circuit breaker overflowed |
| `upstream_cx_connect_ms` | Histogram | | Connection establishment milliseconds |
| `upstream_cx_length_ms` | Histogram | | Connection length milliseconds |
| `upstream_cx_destroy` | Counter | ✅ | Total destroyed connections |
| `upstream_cx_destroy_local` | Counter | | Total connections destroyed locally |
| `upstream_cx_destroy_remote` | Counter | | Total connections destroyed remotely |
| `upstream_cx_destroy_with_active_rq` | Counter | | Total connections destroyed with 1+ active request |
| `upstream_cx_destroy_local_with_active_rq` | Counter | | Total connections destroyed locally with 1+ active request |
| `upstream_cx_destroy_remote_with_active_rq` | Counter | | Total connections destroyed remotely with 1+ active request |
| `upstream_cx_close_notify` | Counter | | Total connections closed via HTTP/1.1 connection close header or HTTP/2 or HTTP/3 GOAWAY |
| `upstream_cx_rx_bytes_total` | Counter | 🚧 | Total received connection bytes |
| `upstream_cx_rx_bytes_buffered` | Gauge | | Received connection bytes currently buffered |
| `upstream_cx_tx_bytes_total` | Counter | 🚧 | Total sent connection bytes |
| `upstream_cx_tx_bytes_buffered` | Gauge | | Send connection bytes currently buffered |
| `upstream_cx_pool_overflow` | Counter | | Total times that the cluster’s connection pool circuit breaker overflowed |
| `upstream_cx_protocol_error` | Counter | | Total connection protocol errors |
| `upstream_cx_max_requests` | Counter | | Total connections closed due to maximum requests |
| `upstream_cx_none_healthy` | Counter | | Total times connection not established due to no healthy hosts |
| `upstream_rq_total` | Counter | ✅ | Total requests |
| `upstream_rq_active` | Gauge |  ✅ | Total active requests |
| `upstream_rq_pending_total` | Counter | | Total requests pending a connection pool connection |
| `upstream_rq_pending_overflow` | Counter | | Total requests that overflowed connection pool or requests (mainly for HTTP/2 and above) circuit breaking and were failed |
| `upstream_rq_pending_failure_eject` | Counter | | Total requests that were failed due to a connection pool connection failure or remote connection termination |
| `upstream_rq_pending_active` | Gauge | | Total active requests pending a connection pool connection |
| `upstream_rq_cancelled` | Counter | | Total requests cancelled before obtaining a connection pool connection |
| `upstream_rq_maintenance_mode` | Counter | | Total requests that resulted in an immediate 503 due to maintenance mode |
| `upstream_rq_timeout` | Counter | ✅ | Total requests that timed out waiting for a response |
| `upstream_rq_max_duration_reached` | Counter | | Total requests closed due to max duration reached |
| `upstream_rq_per_try_timeout` | Counter | ✅ | Total requests that hit the per try timeout (except when request hedging is enabled) |
| `upstream_rq_rx_reset` | Counter | | Total requests that were reset remotely |
| `upstream_rq_tx_reset` | Counter | | Total requests that were reset locally |
| `upstream_rq_retry` | Counter | ✅ | Total request retries |
| `upstream_rq_retry_backoff_exponential` | Counter | | Total retries using the exponential backoff strategy |
| `upstream_rq_retry_backoff_ratelimited` | Counter | | Total retries using the ratelimited backoff strategy |
| `upstream_rq_retry_limit_exceeded` | Counter | | Total requests not retried due to exceeding the configured number of maximum retries |
| `upstream_rq_retry_success` | Counter | | Total request retry successes |
| `upstream_rq_retry_overflow` | Counter | | Total requests not retried due to circuit breaking or exceeding the retry budget |
| `upstream_flow_control_paused_reading_total` | Counter | | Total number of times flow control paused reading from upstream |
| `upstream_flow_control_resumed_reading_total` | Counter | | Total number of times flow control resumed reading from upstream |
| `upstream_flow_control_backed_up_total` | Counter | | Total number of times the upstream connection backed up and paused reads from downstream |
| `upstream_flow_control_drained_total` | Counter | | Total number of times the upstream connection drained and resumed reads from downstream |
| `upstream_internal_redirect_failed_total` | Counter | | Total number of times failed internal redirects resulted in redirects being passed downstream. |
| `upstream_internal_redirect_succeeded_total` | Counter | | Total number of times internal redirects resulted in a second upstream request. |
| `membership_change` | Counter | | Total cluster membership changes |
| `membership_healthy` | Gauge | | Current cluster healthy total (inclusive of both health checking and outlier detection) |
| `membership_degraded` | Gauge | | Current cluster degraded total |
| `membership_excluded` | Gauge | | Current cluster excluded total |
| `membership_total` | Gauge | | Current cluster membership total |
| `retry_or_shadow_abandoned` | Counter | | Total number of times shadowing or retry buffering was canceled due to buffer limits |
| `config_reload` | Counter | | Total API fetches that resulted in a config reload due to a different config |
| `update_attempt` | Counter | | Total attempted cluster membership updates by service discovery |
| `update_success` | Counter | | Total successful cluster membership updates by service discovery |
| `update_failure` | Counter | | Total failed cluster membership updates by service discovery |
| `update_duration` | Histogram | | Amount of time spent updating configs |
| `update_empty` | Counter | | Total cluster membership updates ending with empty cluster load assignment and continuing with previous config |
| `update_no_rebuild` | Counter | | Total successful cluster membership updates that didn’t result in any cluster load balancing structure rebuilds |
| `version` | Gauge | | Hash of the contents from the last successful API fetch |
| `warming_state` | Gauge | | Current cluster warming state |
| `max_host_weight` | Gauge | | Maximum weight of any host in the cluster |
| `bind_errors` | Counter | | Total errors binding the socket to the configured source address |
| `assignment_timeout_received` | Counter | | Total assignments received with endpoint lease information. |
| `assignment_stale` | Counter | | Number of times the received assignments went stale before new assignments arrived. |

### Health check statistics
| Name | Type | Status | Description |
| :--- | :--- | :--- | :--- |
| `attempt` | Counter | | Number of health checks |
| `success` | Counter | | Number of successful health checks |
| `failure` | Counter | | Number of immediately failed health checks (e.g. HTTP 503) as well as network failures |
| `passive_failure` | Counter | | Number of health check failures due to passive events (e.g. x-envoy-immediate-health-check-fail) |
| `network_failure` | Counter | | Number of health check failures due to network error |
| `verify_cluster` | Counter | | Number of health checks that attempted cluster name verification |
| `healthy` | Gauge | 🚧 | Number of healthy members |


### Server statistics
| Name | Type | Status | Description |
| :--- | :--- | :--- | :--- |
|uptime|  Gauge| ✅ |  Current server uptime in seconds
|concurrency|  Gauge|  ✅ | Number of worker threads
|memory_allocated|  Gauge|  ✅  | Current amount of allocated memory in bytes. Total of both new and old Envoy processes on hot restart.
|memory_heap_size| Gauge|  ✅  | Current reserved heap size in bytes. New Envoy process heap size on hot restart.
|memory_physical_size| Gauge|  ✅  | Current estimate of total bytes of the physical memory. New Envoy process physical memory size on hot restart.
|live| Gauge| | 1 if the server is not currently draining, 0 otherwise
|state| Gauge| | Current State of the Server.
|parent_connections | Gauge| | Total connections of the old Envoy process on hot restart
|total_connections | Gauge| | Total connections of both new and old Envoy processes
|version| Gauge|  🚧 | Integer represented version number based on SCM revision or stats_server_version_override if set.
|days_until_first_cert_expiring| Gauge| | Number of days until the next certificate being managed will expire
|days_until_first_ocsp_response_expiring| Gauge| | Number of days until the next OCSP response being managed will expire
|hot_restart_epoch| Gauge| | Current hot restart epoch – an integer passed via command line flag --restart-epoch usually indicating generation.
|hot_restart_generation| Gauge| | Current hot restart generation – like hot_restart_epoch but computed automatically by incrementing from parent.
|initialization_time_ms| Gauge| | Total time taken for Envoy initialization in milliseconds. This is the time from server start-up until the worker threads are ready to accept new connections
|debug_assertion_failures| Gauge| | Number of debug assertion failures detected in a release build if compiled with --define log_debug_assert_in_release=enabled or zero otherwise
|envoy_bug_failures| Gauge| | Number of envoy bug failures detected in a release build. File or report the issue if this increments as this may be serious.
|static_unknown_fields| Gauge| | Number of messages in static configuration with unknown fields
|dynamic_unknown_fields| Gauge| | Number of messages in dynamic configuration with unknown fields
|wip_protos| Counter | Number of messages and fields marked as work-in-progress being used
