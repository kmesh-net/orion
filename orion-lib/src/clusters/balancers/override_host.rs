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

use std::{str::FromStr, sync::Arc};

use http::{uri::Authority, HeaderName, HeaderValue};
use rustc_hash::FxHashMap as HashMap;
use tracing::debug;

use super::Balancer;
use crate::clusters::{
    health::{EndpointHealth, HealthStatus, ValueUpdated},
    load_assignment::{BalancerType, LbEndpoint, LocalityLbEndpoints},
};
use crate::transport::HttpChannels;
use crate::Result;

#[derive(Debug, Clone)]
struct OverrideEndpoint {
    endpoint: Arc<LbEndpoint>,
    healthy: bool,
}

#[derive(Debug, Clone)]
pub struct OverrideHostLoadBalancer {
    header: HeaderName,
    fallback: Box<BalancerType>,
    endpoints: HashMap<String, OverrideEndpoint>,
}

impl OverrideHostLoadBalancer {
    pub fn new(header: HeaderName, fallback: BalancerType, endpoints: &[LocalityLbEndpoints]) -> Self {
        Self::build(header, fallback, endpoints, None)
    }

    pub fn rebuild(self, endpoints: &[LocalityLbEndpoints]) -> Self {
        let OverrideHostLoadBalancer { header, fallback, endpoints: previous, .. } = self;
        let fallback = *fallback;
        Self::build(header, fallback, endpoints, Some(&previous))
    }

    pub fn select_override(&mut self, header_value: &HeaderValue, fallback_hash: Option<u64>) -> Result<HttpChannels> {
        debug!("Selecting override endpoints for header value: {header_value:?}");
        let Ok(value_str) = header_value.to_str() else {
            debug!("Invalid header value: {header_value:?}");
            let endpoint =
                self.fallback.next_item(fallback_hash).ok_or_else(|| crate::Error::from("No active endpoint"))?;
            let channel = endpoint.http_channel().ok_or("No HTTP channel available for this endpoint")?;
            return Ok(HttpChannels::Single(channel.clone()));
        };
        let endpoints: Vec<Arc<LbEndpoint>> = value_str
            .split(',')
            .filter_map(|raw| {
                let trimmed = raw.trim();
                if trimmed.is_empty() {
                    return None;
                }

                let authority = Authority::from_str(trimmed).ok()?;
                self.endpoints
                    .get(authority.as_str())
                    .filter(|entry| entry.healthy)
                    .map(|entry| Arc::clone(&entry.endpoint))
            })
            .collect();

        if let Some((primary, rest)) = endpoints.split_first() {
            debug!("Found {} healthy override endpoints for header value: {value_str}", endpoints.len());
            let channel = primary.http_channel().ok_or("No HTTP channel available for this endpoint")?.clone();
            let failover_channels = rest
                .into_iter()
                .map(|endpoint| {
                    endpoint
                        .http_channel()
                        .cloned()
                        .ok_or("No HTTP failover channels available for this endpoint".into())
                })
                .collect::<Result<Vec<_>>>()?;

            Ok(HttpChannels::MultiWithFailover { channel, failover_channels })
        } else {
            let endpoint =
                self.fallback.next_item(fallback_hash).ok_or_else(|| crate::Error::from("No active endpoint"))?;
            debug!(
                "No healthy override endpoints found for header {value_str} (fallback using the endpoint {endpoint})"
            );
            let channel = endpoint.http_channel().ok_or("No HTTP channel available for this endpoint")?;
            Ok(HttpChannels::Single(channel.clone()))
        }
    }

    pub fn update_health(&mut self, endpoint: &LbEndpoint, health: HealthStatus) -> Result<ValueUpdated> {
        if let Some(entry) = self.endpoints.get_mut(endpoint.authority().as_str()) {
            entry.healthy = health.is_healthy();
        }
        self.fallback.update_health(endpoint, health)
    }

    pub fn header(&self) -> HeaderName {
        self.header.clone()
    }

    pub fn fallback_requires_hash(&self) -> bool {
        self.fallback.requires_hash()
    }

    fn build(
        header: HeaderName,
        fallback: BalancerType,
        endpoints: &[LocalityLbEndpoints],
        previous: Option<&HashMap<String, OverrideEndpoint>>,
    ) -> Self {
        let fallback = Box::new(fallback);
        let mut override_map = HashMap::default();
        for locality in endpoints {
            for endpoint in &locality.endpoints {
                let key = endpoint.authority().as_str().to_owned();
                let healthy = previous
                    .and_then(|prev| prev.get(&key).map(|entry| entry.healthy))
                    .unwrap_or_else(|| endpoint.health_status.is_healthy());
                override_map.insert(key, OverrideEndpoint { endpoint: Arc::clone(endpoint), healthy });
            }
        }
        Self { header, fallback, endpoints: override_map }
    }
}

impl Balancer<LbEndpoint> for OverrideHostLoadBalancer {
    fn next_item(&mut self, hash: Option<u64>) -> Option<Arc<LbEndpoint>> {
        self.fallback.next_item(hash)
    }
}
