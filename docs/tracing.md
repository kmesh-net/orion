### Description

This document provides an overview of tracing functionality in Orion proxy.

### Confirugration

To enable tracing, the Orion proxy must be built with the `tracing` feature flag. Each HTTP Connection Manager (HCM) can be configured in the bootstrap YAML file with `tracing` settings. Below is an example configuration added to the HttpConnectionManager settings:

```yaml
      tracing:
           client_sampling:
             value: 50.0
           random_sampling:
             value: 80.0
           overall_sampling:
             value: 100.0
           provider:
               name: envoy.tracers.opentelemetry
               typed_config:
                   "@type": type.googleapis.com/envoy.config.trace.v3.OpenTelemetryConfig
                   service_name: "orion-service-name"
                   grpc_service:
                     google_grpc:
                         target_uri: "http://192.168.86.27:4317"
                         stat_prefix: "orion"
```

### Opentelementry Attributes

These are the OpenTelemetry attributes used to enrich the tracing data.

| attribute                    | semconv_group   | description                                               | envoy_default   | role          |  status  |
|:-----------------------------|:----------------|:----------------------------------------------------------|:----------------|:--------------|:---------|
| http.request.method          | HTTP            | HTTP request method (e.g., GET).                          | Yes             | Server+Client |    ✅    |
| http.request.method_original | HTTP            | Original method if it was overridden.                     | No              | Server+Client |    🚧    |
| http.response.status_code    | HTTP            | Numeric HTTP response status code.                        | Yes             | Server+Client |    ✅    |
| http.route                   | HTTP            | Route template/parametrized path.                         | No              | Server+Client |    🚧    |
| url.scheme                   | URL             | URI scheme (http, https).                                 | Yes             | Server+Client |    ✅    |
| url.path                     | URL             | Absolute path, without query.                             | Yes             | Server+Client |    ✅    |
| url.query                    | URL             | Query string (without '?').                               | Yes             | Server+Client |    ✅    |
| url.full                     | URL             | Entire URI (scheme://host/path?query).                    | Yes             | Server+Client |    ✅    |
| server.address               | Network/Server  | Logical server/host address.                              | Yes             | Server        |    🚧    |
| server.port                  | Network/Server  | Server port number.                                       | Yes             | Server        |    🚧    |
| client.address               | Network/Client  | Client IP/hostname.                                       | Yes             | Client        |    🚧    |
| client.port                  | Network/Client  | Client source port.                                       | Yes             | Client        |    🚧    |
| network.protocol.name        | Network         | Application protocol name (e.g., http).                   | Yes             | Server+Client |    ✅    |
| network.protocol.version     | Network         | Application protocol version (e.g., 1.1, 2, 3).           | Yes             | Server+Client |    ✅    |
| user_agent.original          | User-Agent      | Full user agent string.                                   | Yes             | Server        |    ✅    |
| http.request.body.size       | HTTP            | Size of the request body in bytes.                        | No              | Server+Client |    🚧    |
| http.response.body.size      | HTTP            | Size of the response body in bytes.                       | No              | Server+Client |    🚧    |
| http.request.resend_count    | HTTP            | How many times this request was resent.                   | No              | Client        |    🚧    |
| http.request.header.<key>    | HTTP (pattern)  | Selected request header(s) to capture.                    | No              | Server+Client |    🚧    |
| http.response.header.<key>   | HTTP (pattern)  | Selected response header(s) to capture.                   | No              | Server+Client |    🚧    |
| rpc.system                   | RPC             | Identifier of RPC system (e.g., grpc).                    | Conditional     | Server+Client |    🚧    |
| rpc.service                  | RPC             | Fully-qualified service name.                             | Conditional     | Server+Client |    🚧    |
| rpc.method                   | RPC             | Method name.                                              | Conditional     | Server+Client |    🚧    |
| rpc.grpc.status_code         | RPC             | gRPC status code.                                         | Conditional     | Server+Client |    🚧    |
| upstream.cluster.name        | Envoy-specific  | Name of the upstream cluster handling the request.        | Yes             | Client        |    ✅    |
| upstream.address             | Envoy-specific  | Address (host:port) of the selected upstream endpoint.    | Yes             | Client        |    ✅    |
| span.operation               | Envoy-specific  | Span operation name (often from route decorator or host). | Yes             | Server+Client |    ✅    |
| error                        | Generic         | Error indicator (true if HTTP 5xx or gRPC non-OK).        | Conditional     | Server+Client |    ✅    |
