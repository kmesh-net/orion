// Copyright 2025 The kmesh Authors
//
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//   http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.
//
//

use crate::{
    secrets::{TlsConfigurator, TransportSecret, WantsToBuildClient},
    Result, SecretManager,
};
use orion_configuration::config::transport::UpstreamTransportSocketConfig;
use rustls::ClientConfig;

use super::proxy_protocol::ProxyProtocolConfigurator;

#[derive(Debug, Clone, Default)]
pub enum UpstreamTransportSocketConfigurator {
    Tls(TlsConfigurator<ClientConfig, WantsToBuildClient>),
    ProxyProtocol(ProxyProtocolConfigurator),
    #[default]
    None,
}

impl UpstreamTransportSocketConfigurator {
    pub fn tls_configurator(&self) -> Option<&TlsConfigurator<ClientConfig, WantsToBuildClient>> {
        match self {
            UpstreamTransportSocketConfigurator::Tls(tls_conf) => Some(tls_conf),
            UpstreamTransportSocketConfigurator::ProxyProtocol(proxy_conf) => {
                proxy_conf.inner_tls_configurator.as_ref()
            },
            UpstreamTransportSocketConfigurator::None => None,
        }
    }

    pub fn update_secret(&mut self, secret_id: &str, secret: TransportSecret) -> Result<()> {
        match self {
            UpstreamTransportSocketConfigurator::Tls(tls_conf) => {
                let updated_tls =
                    TlsConfigurator::<ClientConfig, WantsToBuildClient>::update(tls_conf.clone(), secret_id, secret)?;
                *tls_conf = updated_tls;
                Ok(())
            },
            UpstreamTransportSocketConfigurator::ProxyProtocol(proxy_conf) => {
                proxy_conf.update_secret(secret_id, secret)
            },
            UpstreamTransportSocketConfigurator::None => Ok(()),
        }
    }
}

impl TryFrom<(Option<UpstreamTransportSocketConfig>, &SecretManager)> for UpstreamTransportSocketConfigurator {
    type Error = crate::Error;

    fn try_from(
        (transport_socket_config, secrets): (Option<UpstreamTransportSocketConfig>, &SecretManager),
    ) -> Result<Self> {
        if let Some(transport_socket_config) = transport_socket_config {
            match transport_socket_config {
                UpstreamTransportSocketConfig::Tls(tls_config) => {
                    let tls_configurator =
                        TlsConfigurator::<ClientConfig, WantsToBuildClient>::try_from((tls_config, secrets))?;
                    Ok(UpstreamTransportSocketConfigurator::Tls(tls_configurator))
                },
                UpstreamTransportSocketConfig::ProxyProtocol(proxy_config) => {
                    let proxy_configurator = ProxyProtocolConfigurator::try_from((proxy_config, secrets))?;
                    Ok(UpstreamTransportSocketConfigurator::ProxyProtocol(proxy_configurator))
                },
                UpstreamTransportSocketConfig::RawBuffer => Ok(UpstreamTransportSocketConfigurator::None),
            }
        } else {
            Ok(UpstreamTransportSocketConfigurator::None)
        }
    }
}
