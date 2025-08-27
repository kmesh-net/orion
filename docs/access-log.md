# Access Log

The access log is a record of requests processed by Orion, useful for monitoring and debugging. Messages are formatted using orion-format, following the Envoy specifications.

## Features

- Asynchronous logging. Once the message is formatted, it is sent to the logging system asynchronously. To ease the logging operation, the access_log module provides a few log macros without the need to handle the Sender channel directly.
- The log configuration is parsed from the HCM and from the TcpProxy configuration (orion-configuration has been extended to support this).
- Each Listener can have its own log configuration, including a different log format and a different type of log (e.g. file, stdout, stderr, etc.).
- Each Listener can handle multiple log writers, useful for writing formatted messages to multiple destinations.
- Log rotation is supported and fully configurable in the Orion configuration (access_log section). The available options are those of the tracing-appender crate.
- XDS support. The system supports dynamic XDS configuration for access logs, which allows reconfiguration of access logs without the need to restart the Orion process.
- Multiple loggers can be enabled at the same time, each with its own channel, reducing contention on the Sender side and improving I/O performance by allowing multiple loggers to write to different destinations concurrently.
- The macro log_access_balanced! is provided to log messages in a balanced way. The logger is selected based on the hash of the calling thread ID, which ensures that the same thread, and possibly the requests coming from the same connection (true when using multiple single-threaded runtimes), will always log to the same logger.
- The macro log_access_single! is provided to log messages to a single logger, which is useful for logging admin messages. Files are opened lazily, so the file is not created until the first message is logged. This is useful to avoid creating empty files when log_access_balanced! is not used.
- The macro with_access_log_enabled! is provided to wrap a block of code that needs to be executed only if the access log is enabled. This is useful to avoid unnecessary computation when the access log is not enabled (usually the formatting operation).
- The access_log is enabled and configured in the access_log section of the Orion configuration file. All the options are optional, and sensible default values are used if not specified.

Example:
```yaml
access_logging:
  num_instances: 2
  queue_length: 64
  max_log_files: 5
  log_rotation: daily
```

- A new `num_service_threads` option is available in the `runtime` section of the Orion configuration file, which allows configuring the number of threads used in the service runtime. This runtime is used by the XDS handler, the access log system, and the admin interface.

## Architecture

```

                                                                                                    ┌──────────────┐       ┌────┐   ┌────┐    ┌────┐
                                                                                             ┌─────▶│  Listener1   │──────▶│Log1├──▶│Log2├───▶│LogN│
                                                                                             │      └──────────────┘       └────┘   └────┘    └────┘
                                                                                             │
                                                                                             │
                                                                       ┌───────────────┐     │      ┌──────────────┐       ┌────┐   ┌────┐    ┌────┐
                                                                       │               │     ├─────▶│  Listener2   │──────▶│Log1├──▶│Log2├───▶│LogN│
                                          ┌───────────────────┐        │               │     │      └──────────────┘       └────┘   └────┘    └────┘
                                ┌ ─ ─ ─ ─▶│                   ├───────▶│AccessLogger#0 │─────┤
                                          └───────────────────┘        │               │     │
                                │                                      │               │     │      ┌──────────────┐       ┌────┐   ┌────┐
                                                                       └───────────────┘     └─────▶│  ListenerN   │──────▶│Log1├──▶│Log2│
                                │       tokio::sync::mpsc                                           └──────────────┘       └────┘   └────┘

                                │

                                │
                                                                                                    ┌──────────────┐       ┌────┐   ┌────┐    ┌────┐
┌─────────────────────┐         │                                                            ┌─────▶│  Listener1   │──────▶│Log1├──▶│Log2├───▶│LogN│
│log_access_balanced! │─ ─ ─ ─ ─                                                             │      └──────────────┘       └────┘   └────┘    └────┘
└─────────────────────┘         │                                                            │
                                                                                             │
                                │                                      ┌───────────────┐     │      ┌──────────────┐       ┌────┐
                                                                       │               │     ├─────▶│  Listener2   │──────▶│Log1│
                                │         ┌───────────────────┐        │               │     │      └──────────────┘       └────┘
                                 ─ ─ ─ ─ ▶│                   ├───────▶│AccessLogger#1 │─────┤
                                          └───────────────────┘        │               │     │
                                                                       │               │     │      ┌──────────────┐       ┌────┐   ┌────┐
                                        tokio::sync::mpsc              └───────────────┘     └─────▶│  ListenerN   │──────▶│Log1├──▶│Log2│
                                                                                                    └──────────────┘       └────┘   └────┘
```

## Limitations

- If multiple loggers are enabled, the message is formatted separately for each logger, even when the format is the same. There is room for optimization here, but it has not been implemented yet.

- The following log writers are currently not supported:
  - `fluentd`
  - `http_grpc`
  - `tcp_grpc`
  - `opentelemetry`
  - `wasm`


- The following tables show the operators currently implemented for logging. The “HCM” (HttpConnectionManager) and “TcpProxy” columns indicate their status: a check mark (✅) means the operator is supported, a dash (–) means it is not applicable for that type of connection, and a cross mark (❌) means it is not yet implemented. Operators shown in bold are those used in Envoy’s default log format for HTTP connections.



| Operator                               | HCM | TCPProxy |
| :------------------------------------- | :-: | :------: |
| **BYTES_RECEIVED**                     | ✅  |    ✅   |
| **BYTES_SENT**                         | ✅  |    ✅   |
| **PROTOCOL**                           | ✅  |    -    |
| UPSTREAM_PROTOCOL                      | ✅  |    -    |
| **RESPONSE_CODE**                      | ✅  |    -    |
| REQUEST_HEADERS_BYTES                  | ✅  |    -    |
| RESPONSE_HEADERS_BYTES                 | ✅  |    -    |
| **DURATION**                           | ✅  |    -    |
| REQUEST_DURATION                       | ✅  |    -    |
| REQUEST_TX_DURATION                    | ✅  |    -    |
| RESPONSE_DURATION                      | ✅  |    -    |
| RESPONSE_TX_DURATION                   | ✅  |    -    |
| **RESPONSE_FLAGS**                     | ✅  |    ✅   |
| **RESPONSE_FLAGS_LONG**                | ✅  |    ✅   |
| **UPSTREAM_HOST**                      | ✅  |    ✅   |
| UPSTREAM_LOCAL_ADDRESS                 | ❌  |    ✅   |
| UPSTREAM_LOCAL_ADDRESS_WITHOUT_PORT    | ❌  |    ✅   |
| UPSTREAM_LOCAL_PORT                    | ❌  |    ✅   |
| UPSTREAM_REMOTE_ADDRESS                | ✅  |    ✅   |
| UPSTREAM_REMOTE_ADDRESS_WITHOUT_PORT   | ❌  |    ✅   |
| UPSTREAM_REMOTE_PORT                   | ❌  |    ✅   |
| DOWNSTREAM_LOCAL_ADDRESS               | ❌  |    ✅   |
| DOWNSTREAM_LOCAL_ADDRESS_WITHOUT_PORT  | ❌  |    ✅   |
| DOWNSTREAM_LOCAL_PORT                  | ❌  |    ✅   |
| DOWNSTREAM_REMOTE_ADDRESS              | ❌  |    ✅   |
| DOWNSTREAM_REMOTE_ADDRESS_WITHOUT_PORT | ❌  |    ✅   |
| DOWNSTREAM_REMOTE_PORT                 | ❌  |    ✅   |
| UPSTREAM_CLUSTER                       | ✅  |    ✅   |
| UPSTREAM_CLUSTER_RAW                   | ✅  |    ✅   |
| CONNECTION_ID                          | -   |    ✅   |
| UPSTREAM_CONNECTION_ID                 | -   |    ✅   |
| UNIQUE_ID                              | ✅  |    -    |
| TRACE_ID                               | ✅  |    -    |
| **START_TIME**                         | ✅  |    ✅   |
| REQ(:SCHEME)                           | ✅  |    -    |
| **REQ(:METHOD)**                       | ✅  |    -    |
| REQ(:PATH)                             | ✅  |    -    |
| **REQ(:AUTHORITY)**                    | ✅  |    -    |
| **REQ(X-ENVOY-ORIGINAL-PATH?:PATH)**   | ✅  |    -    |
| RESP(:STATUS)                          | ✅  |    -    |
| **REQ(HEADER)**                        | ✅  |    -    |
| **RESP(HEADER)**                       | ✅  |    -    |

NOTE 1: %UPSTREAM_CLUSTER% and %UPSTREAM_CLUSTER_RAW% are identical as we are not currently supporting `alt_stat_name` for clusters
NOTE 2: The UNIQUE_ID is consistent with envoy's unique ID: if the x-request-id is present and valid, it is used; otherwise, a new unique ID is generated.
