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

use crate::decode::decode_any_type;
use std::{collections::HashSet, fs, path};

use crate::envoy_validation::FilterValidation;
use envoy_data_plane_api::envoy::{
    config::{
        bootstrap::v3::{Bootstrap, bootstrap::DynamicResources},
        cluster::v3::Cluster,
        core::v3::{
            ApiConfigSource, SocketAddress, address, config_source::ConfigSourceSpecifier,
            grpc_service::TargetSpecifier,
        },
        endpoint::v3::{Endpoint, lb_endpoint::HostIdentifier},
        listener::v3::{Listener, filter::ConfigType},
        route::v3::RouteConfiguration,
    },
    extensions::filters::network::http_connection_manager::v3::{
        HttpConnectionManager, http_connection_manager::RouteSpecifier,
    },
};

use crate::xds::model::TypeUrl;

const EXT_HTTP_CONN_MANAGER: &str =
    "type.googleapis.com/envoy.extensions.filters.network.http_connection_manager.v3.HttpConnectionManager";

#[derive(Clone)]
pub struct BootstrapLoader {
    bootstrap: Option<Bootstrap>,
}

impl BootstrapLoader {
    pub fn load<P: AsRef<path::Path>>(config_path: P) -> BootstrapLoader {
        let mut loader = BootstrapLoader { bootstrap: None };
        let config_contents = match fs::read_to_string(config_path) {
            Ok(content) => content,
            Err(_) => return loader,
        };

        match crate::decode::from_yaml::<Bootstrap>(&config_contents) {
            Ok(bootstrap) => {
                loader.bootstrap = Some(bootstrap.clone());
            },
            Err(_) => return loader,
        };

        loader
    }
}

impl From<Bootstrap> for BootstrapLoader {
    fn from(value: Bootstrap) -> Self {
        BootstrapLoader { bootstrap: Some(value) }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct XdsConfig(pub XdsType, pub SocketAddress);

#[derive(Debug, Clone, PartialEq)]
pub enum XdsType {
    Aggregated(HashSet<TypeUrl>),
    Individual(TypeUrl),
}

impl From<TypeUrl> for XdsType {
    fn from(url: TypeUrl) -> Self {
        XdsType::Individual(url)
    }
}

#[derive(Debug)]
pub enum BootstrapResolveErr {
    InvalidEndpoint,
    InvalidListener,
    InvalidHttpManager,
    InvalidYaml,
    InvalidSocketAddress,
    MissingRdsConfigSource,
}

pub trait BootstrapResolver {
    fn get_static_listener_configs(&self) -> Result<Vec<Listener>, BootstrapResolveErr>;
    fn get_static_route_configs(&self) -> Result<Vec<RouteConfiguration>, BootstrapResolveErr>;
    fn get_static_cluster_configs(&self) -> Result<Vec<Cluster>, BootstrapResolveErr>;
    #[deprecated]
    fn get_xds_configs(&self) -> Result<Vec<XdsConfig>, BootstrapResolveErr>;
    fn get_ads_configs(&self) -> Result<Vec<String>, BootstrapResolveErr>;
}

impl BootstrapLoader {
    fn get_endpoint_by_name(&self, name: String) -> Result<Endpoint, BootstrapResolveErr> {
        let host_identifier = self
            .bootstrap
            .as_ref()
            .and_then(|b| b.static_resources.as_ref())
            .and_then(|static_resources| static_resources.clusters.iter().find(|&cluster| cluster.name == name))
            .and_then(|cluster| cluster.load_assignment.as_ref())
            // TODO: support multi endpoints with weights through load balancing
            .and_then(|load_assignment| load_assignment.endpoints.first())
            .and_then(|endpoints| endpoints.lb_endpoints.first())
            .and_then(|endpoint| endpoint.host_identifier.as_ref());

        let Some(HostIdentifier::Endpoint(endpoint_any)) = host_identifier else {
            return Err(BootstrapResolveErr::InvalidEndpoint);
        };
        Ok(endpoint_any.clone())
    }

    fn resolve_api_config(&self, api_config_source: &ApiConfigSource) -> Option<SocketAddress> {
        let target_specifier =
            api_config_source.grpc_services.first().and_then(|grpc_service| grpc_service.target_specifier.as_ref());

        if let Some(TargetSpecifier::EnvoyGrpc(grpc_conf)) = target_specifier {
            self.get_endpoint_by_name(grpc_conf.cluster_name.clone())
                .ok()
                .and_then(|endpoint| endpoint.address)
                .and_then(|address| address.address)
                .and_then(|address| match address {
                    address::Address::SocketAddress(socket) => Some(socket),
                    _ => None,
                })
        } else {
            None
        }
    }

    fn resolve_config_source(&self, config_source: Option<&ConfigSourceSpecifier>) -> Option<SocketAddress> {
        if let Some(ConfigSourceSpecifier::ApiConfigSource(api_config_source)) = config_source {
            self.resolve_api_config(api_config_source)
        } else {
            None
        }
    }

    fn get_dynamic_resource(&self) -> Option<&DynamicResources> {
        self.bootstrap.as_ref().and_then(|bootstrap| bootstrap.dynamic_resources.as_ref())
    }

    fn get_lds_source(&self) -> Option<&ConfigSourceSpecifier> {
        self.get_dynamic_resource()
            .and_then(|dynamic_resources| dynamic_resources.lds_config.as_ref())
            .and_then(|config_source| config_source.config_source_specifier.as_ref())
    }

    fn get_cds_source(&self) -> Option<&ConfigSourceSpecifier> {
        self.bootstrap
            .as_ref()
            .and_then(|bootstrap| bootstrap.dynamic_resources.as_ref())
            .and_then(|dynamic_resources| dynamic_resources.cds_config.as_ref())
            .and_then(|config_source| config_source.config_source_specifier.as_ref())
    }

    fn get_rds_sources(&self) -> Result<Vec<ConfigSourceSpecifier>, BootstrapResolveErr> {
        self.bootstrap
            .as_ref()
            .and_then(|bootstrap| bootstrap.static_resources.as_ref())
            .map(|static_resource| static_resource.listeners.as_slice())
            .unwrap_or(&[])
            .iter()
            .flat_map(|listener| {
                listener.filter_chains.iter().flat_map(|filter_chain| {
                    filter_chain.filters.iter().filter_map(
                        |filter| -> Option<Result<ConfigSourceSpecifier, BootstrapResolveErr>> {
                            filter
                                .get_http_connection_manager()
                                .map_err(|_e| BootstrapResolveErr::InvalidYaml)
                                .ok()?
                                .and_then(
                                    |http_connection_manager: HttpConnectionManager| match http_connection_manager
                                        .route_specifier
                                    {
                                        Some(RouteSpecifier::Rds(rds)) => rds
                                            .config_source
                                            .and_then(|config_source| config_source.config_source_specifier.clone())
                                            .ok_or(BootstrapResolveErr::MissingRdsConfigSource)
                                            .into(),
                                        _ => None,
                                    },
                                )
                                .ok_or(())
                                .ok()
                        },
                    )
                })
            })
            .collect::<Result<Vec<ConfigSourceSpecifier>, BootstrapResolveErr>>()
    }

    fn parse_ads(&self, ads_config: &ApiConfigSource) -> Result<XdsConfig, BootstrapResolveErr> {
        let ads_host = self.resolve_api_config(ads_config).ok_or(BootstrapResolveErr::InvalidSocketAddress)?;
        let mut ads_types = HashSet::new();

        if let Some(ConfigSourceSpecifier::Ads(_)) = self.get_lds_source() {
            ads_types.insert(TypeUrl::Listener);
        }
        if let Some(ConfigSourceSpecifier::Ads(_)) = self.get_cds_source() {
            ads_types.insert(TypeUrl::Cluster);
        }

        for conf_source in self.get_rds_sources()? {
            if let ConfigSourceSpecifier::Ads(_) = conf_source {
                ads_types.insert(TypeUrl::RouteConfiguration);
                break;
            }
        }

        Ok(XdsConfig(XdsType::Aggregated(ads_types), ads_host))
    }
}

impl BootstrapResolver for BootstrapLoader {
    fn get_static_listener_configs(&self) -> Result<Vec<Listener>, BootstrapResolveErr> {
        self.bootstrap
            .as_ref()
            .and_then(|bootstrap| bootstrap.static_resources.as_ref())
            .map(|static_resources| static_resources.listeners.clone())
            .ok_or(BootstrapResolveErr::InvalidListener)
    }

    #[allow(clippy::unwrap_used)]
    fn get_static_route_configs(&self) -> Result<Vec<RouteConfiguration>, BootstrapResolveErr> {
        let mut res = Vec::new();
        for listener in self
            .bootstrap
            .as_ref()
            .unwrap()
            .static_resources
            .as_ref()
            .map(|static_resource| static_resource.listeners.as_slice())
            .unwrap_or(&[])
        {
            for filter_chain in &listener.filter_chains {
                for filter in &filter_chain.filters {
                    let config_type = filter.config_type.as_ref().unwrap();
                    if let ConfigType::TypedConfig(filter_any) = config_type {
                        if filter_any.type_url != EXT_HTTP_CONN_MANAGER {
                            continue;
                        }
                        let http_manager: HttpConnectionManager = decode_any_type(filter_any, EXT_HTTP_CONN_MANAGER)
                            .map_err(|_e| BootstrapResolveErr::InvalidHttpManager)?;
                        if let Some(RouteSpecifier::RouteConfig(route_config)) = http_manager.route_specifier {
                            res.push(route_config);
                        }
                    }
                }
            }
        }
        Ok(res)
    }

    fn get_static_cluster_configs(&self) -> Result<Vec<Cluster>, BootstrapResolveErr> {
        Ok(self
            .bootstrap
            .as_ref()
            .and_then(|bootstrap| bootstrap.static_resources.as_ref())
            .map(|static_resources| static_resources.clusters.as_slice())
            .unwrap_or(&[])
            .to_vec())
    }

    fn get_xds_configs(&self) -> Result<Vec<XdsConfig>, BootstrapResolveErr> {
        let mut res = Vec::new();
        if let Some(ads_config) =
            self.get_dynamic_resource().and_then(|dynamic_resources| dynamic_resources.ads_config.as_ref())
        {
            res.push(self.parse_ads(ads_config)?);
        }
        if let Some(socket) = self.resolve_config_source(self.get_lds_source()) {
            res.push(XdsConfig(XdsType::from(TypeUrl::Listener), socket));
        }
        if let Some(socket) = self.resolve_config_source(self.get_cds_source()) {
            res.push(XdsConfig(XdsType::from(TypeUrl::Cluster), socket));
        }
        for conf_source in self.get_rds_sources()? {
            if let Some(socket) = self.resolve_config_source(Some(&conf_source)) {
                res.push(XdsConfig(XdsType::from(TypeUrl::RouteConfiguration), socket));
            }
        }
        Ok(res)
    }

    fn get_ads_configs(&self) -> Result<Vec<String>, BootstrapResolveErr> {
        let mut res = Vec::new();
        if let Some(ads_config) =
            self.get_dynamic_resource().and_then(|dynamic_resources| dynamic_resources.ads_config.as_ref())
        {
            ads_config.grpc_services.iter().for_each(|service| {
                if let Some(TargetSpecifier::EnvoyGrpc(envoy_grpc)) = service.clone().target_specifier {
                    res.push(envoy_grpc.cluster_name);
                }
            })
        }
        Ok(res)
    }
}
