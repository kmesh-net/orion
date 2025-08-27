use std::future::IntoFuture;

use orion_data_plane_api::envoy_data_plane_api::envoy::{
    config::core::v3::{DataSource, data_source::Specifier},
    extensions::transport_sockets::tls::v3::{CertificateValidationContext, secret},
};
use orion_xds::xds::{resources, server::start_aggregate_server};
use tracing::info;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info, orion_xds=debug".into()))
        .with(tracing_subscriber::fmt::layer())
        .init();

    let (_, delta_resources_rx) = tokio::sync::mpsc::channel(100);
    let (_stream_resource_tx, stream_resources_rx) = tokio::sync::mpsc::channel(100);
    let addr = "127.0.0.1:50051".parse()?;

    let grpc_server = tokio::spawn(async move {
        info!("Server started");
        let res = start_aggregate_server(addr, delta_resources_rx, stream_resources_rx).await;
        info!("Server stopped {res:?}");
    });
    tokio::time::sleep(std::time::Duration::from_secs(10)).await;

    let var_name = async move {
        // the secret name needs to match ../orion-proxy/conf/orion-bootstap-sds-simple.yaml
        // we are trying to change secret beefcake_ca to point to a different cert store
        // initially the proxy should return 502 error as it can't set up tls to upstream
        // once the secret is rotated the proxy will return response from upstream

        // run curl like this
        // ng3-proxy$ curl -vi --cacert test_certs/beefcakeCA-gathered/beefcake.intermediate.ca-chain.cert.pem  --cert test_certs/beefcakeCA-gathered/beefcake-dublin.cert.pem --key test_certs/beefcakeCA-gathered/beefcake-dublin.key.pem --resolve athlone_2.beefcake.com:8443:127.0.0.1 https://athlone_2.beefcake.com:8443

        let secret_id = "beefcake_ca";
        let validation_context = CertificateValidationContext {
            trusted_ca: Some(DataSource {
                specifier: Some(Specifier::Filename(
                    "./test_certs/beefcakeCA-gathered/beefcake.intermediate.ca-chain.cert.pem".to_owned(),
                )),
                ..Default::default()
            }),
            ..Default::default()
        };
        let secret_type = secret::Type::ValidationContext(validation_context);
        let secret = resources::create_secret(secret_id, secret_type);
        info!("Adding upstream secret {secret_id}");
        let _secret_resource = resources::create_secret_resource(secret_id, &secret);
    };
    let _xds_resource_producer = tokio::spawn(var_name);

    let _ = grpc_server.into_future().await;
    Ok(())
}
