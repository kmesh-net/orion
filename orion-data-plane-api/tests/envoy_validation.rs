use orion_data_plane_api::{
    bootstrap_loader::bootstrap::{BootstrapLoader, BootstrapResolver},
    decode::from_yaml,
    envoy_data_plane_api::{
        envoy::extensions::filters::network::http_connection_manager::v3::http_connection_manager::CodecType,
        google::protobuf::Duration,
    },
    envoy_validation::{ClusterValidation, FilterChainValidation, FilterValidation, LocalRateLimitValidation},
};
use std::path::PathBuf;

#[test]
fn yaml_get_downstream_tls_context() {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("tests");
    path.push("bootstrap_with_tls_server.yml");
    let loader = BootstrapLoader::load(path.into_os_string().into_string().unwrap());
    let listeners = loader.get_static_listener_configs().unwrap();
    let listener = listeners.first().unwrap();

    let fc = &listener.filter_chains;
    assert_eq!(fc.len(), 1);

    let ctx = fc[0].get_downstream_tls_context().unwrap().expect("DownstreamTlsContext is missing");
    assert_eq!(ctx.common_tls_context.unwrap().tls_params.unwrap().tls_minimum_protocol_version, 4);
    //tls1.3
}

#[test]
fn yaml_get_downstream_tls_context_is_none() {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("tests");
    path.push("bootstrap_with_http_connection_manager.yml");
    let loader = BootstrapLoader::load(path.into_os_string().into_string().unwrap());
    let listeners = loader.get_static_listener_configs().unwrap();
    let listener = listeners.first().unwrap();

    let fc = &listener.filter_chains;
    assert_eq!(fc.len(), 1);
    assert!(fc[0].get_downstream_tls_context().unwrap().is_none());
}

#[test]
fn yaml_get_http_connection_manager() {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("tests");
    path.push("bootstrap_with_http_connection_manager.yml");
    let loader = BootstrapLoader::load(path.into_os_string().into_string().unwrap());
    let listeners = loader.get_static_listener_configs().unwrap();
    let listener = listeners.first().unwrap();

    let fc = &listener.filter_chains;
    assert_eq!(fc.len(), 1);

    let _httpman = fc[0].filters[0].get_http_connection_manager().unwrap().expect("HttpConnectionManager is missing");
}

#[test]
fn filter_codec_type() {
    const INP_FILTER: &str = r#"
name: http_gateway
typedConfig:
  "@type": type.googleapis.com/envoy.extensions.filters.network.http_connection_manager.v3.HttpConnectionManager
  statPrefix: ingress_http
  codecType: HTTP1"#;
    use orion_data_plane_api::envoy_data_plane_api::envoy::config::listener::v3::Filter;
    let filter: Filter = from_yaml(INP_FILTER).unwrap();
    let httpman = filter.get_http_connection_manager().unwrap().unwrap();
    assert_eq!(CodecType::try_from(httpman.codec_type).unwrap().as_str_name(), "HTTP1");
}

#[test]
fn cluster_http_proto_options_ext() {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("tests");
    path.push("bootstrap_with_dynamic_resource.yml");

    const INP_CLUSTER: &str = r#"
name: xds_cluster
connect_timeout: 0.25s
type: STATIC
lb_policy: ROUND_ROBIN
typed_extension_protocol_options:
  envoy.extensions.upstreams.http.v3.HttpProtocolOptions:
    "@type": type.googleapis.com/envoy.extensions.upstreams.http.v3.HttpProtocolOptions
    explicit_http_config:
      http2_protocol_options:
        connection_keepalive:
          interval: 30s
          timeout: 5s
"#;

    use orion_data_plane_api::envoy_data_plane_api::envoy::{
        config::cluster::v3::Cluster,
        extensions::upstreams::http::v3::http_protocol_options::{
            UpstreamProtocolOptions, explicit_http_config::ProtocolConfig,
        },
    };

    let cluster: Cluster = from_yaml(INP_CLUSTER).unwrap();
    let proto_opts = cluster.get_http_protocol_options().unwrap().unwrap();

    let upstream_opts = proto_opts.upstream_protocol_options.unwrap();
    if let UpstreamProtocolOptions::ExplicitHttpConfig(cfg) = upstream_opts {
        if let ProtocolConfig::Http2ProtocolOptions(ref h2_opts) = cfg.protocol_config.as_ref().unwrap() {
            let ka = h2_opts.connection_keepalive.as_ref().unwrap();
            assert_eq!(ka.interval.as_ref().unwrap(), &Duration { seconds: 30, nanos: 0 });
            assert_eq!(ka.timeout.as_ref().unwrap(), &Duration { seconds: 5, nanos: 0 });
        } else {
            panic!("Expecting http2 options, got {:?}", cfg);
        }
    } else {
        panic!("Expecting ExplicitHttpConfig, got {:?}", upstream_opts);
    }
}

#[test]
fn yaml_get_local_ratelimit() {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("tests");
    path.push("bootstrap_with_local_ratelimit.yml");

    const INP_LOCAL_RATELIMIT: &str = r#"
    match: {prefix: "/path/with/rate/limit"}
    route: {cluster: service_protected_by_rate_limit}
    typed_per_filter_config:
      envoy.extensions.filters.http.local_ratelimit.v3.LocalRateLimit:
        "@type": type.googleapis.com/envoy.extensions.filters.http.local_ratelimit.v3.LocalRateLimit
        stat_prefix: http_local_ratelimit
        token_bucket:
          max_tokens: "10000"
          tokens_per_fill: "1000"
          fill_interval: "5s"
"#;

    use orion_data_plane_api::envoy_data_plane_api::envoy::config::route::v3::Route;

    let route: Route = from_yaml(INP_LOCAL_RATELIMIT).unwrap();
    let local_ratelimit = route.get_local_ratelimit().unwrap().unwrap();
    assert_eq!(local_ratelimit.token_bucket.unwrap().max_tokens, 10000);
}
