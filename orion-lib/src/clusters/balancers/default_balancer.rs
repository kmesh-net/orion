// SPDX-FileCopyrightText: © 2025 kmesh authors
// SPDX-License-Identifier: Apache-2.0
//
// Copyright 2025 kmesh authors
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

use std::{fmt::Debug, marker::PhantomData, sync::Arc};

use http::uri::Authority;
use rustc_hash::FxHashMap as HashMap;
use tracing::debug;

use super::{
    healthy::HealthyBalancer,
    priority::{Priority, PriorityInfo},
    wrr::{self, WeightedRoundRobinBalancer},
    Balancer,
};
use crate::{
    clusters::{
        health::{EndpointHealth, HealthStatus, ValueUpdated},
        load_assignment::{LbEndpoint, LocalityLbEndpoints},
    },
    Result,
};

pub trait WeightedEndpoint {
    fn weight(&self) -> u32;
}

pub trait EndpointWithLoad {
    fn http_load(&self) -> u32;
}

pub trait EndpointWithAuthority {
    fn authority(&self) -> &Authority;
}

#[derive(Clone, Debug)]
pub struct LbItem<E> {
    pub item: Arc<E>,
    pub weight: u32,
}

impl<E> LbItem<E> {
    pub fn new(weight: u32, item: Arc<E>) -> Self {
        Self { item, weight }
    }
}

#[derive(Debug, Clone)]
pub struct DefaultBalancer<B, E>
where
    B: Balancer<E>,
{
    priority_level_lb: WeightedRoundRobinBalancer<u32>,
    priorities: HashMap<u32, PriorityInfo<HealthyBalancer<B, E>>>,
    _type: PhantomData<E>,
}

impl<B, E> DefaultBalancer<B, E>
where
    B: Balancer<E> + FromIterator<Arc<E>>,
    E: WeightedEndpoint,
{
    pub fn update_health(&mut self, id: &E, health: HealthStatus) -> Result<ValueUpdated>
    where
        E: Clone + Debug + PartialEq,
    {
        for priority_info in self.priorities.values_mut() {
            if let Ok(updated) = priority_info.balancer.update_health(id, health) {
                if updated == ValueUpdated::NotUpdated {
                    return Ok(ValueUpdated::NotUpdated);
                }

                if health.is_healthy() {
                    priority_info.healthy += 1;
                } else {
                    priority_info.healthy -= 1;
                }

                self.priority_level_lb = Self::recalculate_priority_load_factors(&self.priorities);
                return Ok(ValueUpdated::Updated);
            }
        }
        Err(format!("Can't find endpoint {id:?}").into())
    }

    fn recalculate_priority_load_factors(
        priorities: &HashMap<u32, PriorityInfo<HealthyBalancer<B, E>>>,
    ) -> WeightedRoundRobinBalancer<u32> {
        let priority_load_weights = Priority::calculate_priority_loads(priorities);
        let items = priority_load_weights.into_iter().map(|f| wrr::LbItem::new(f.1, Arc::new(f.0)));
        WeightedRoundRobinBalancer::new(items)
    }
}

impl<B, E> Balancer<E> for DefaultBalancer<B, E>
where
    B: Debug + Balancer<E> + FromIterator<Arc<E>>,
    E: Debug + WeightedEndpoint,
{
    fn next_item(&mut self, hash: Option<u64>) -> Option<Arc<E>> {
        let priority = self.priority_level_lb.next_item(None);
        debug!("Selecting priority {priority:?} based on {:?}", self.priority_level_lb);
        let priority = priority?;
        let priority_info = self.priorities.get_mut(&priority)?;
        let endpoint = priority_info.balancer.next_item(hash);
        debug!("Selecting endpoint {endpoint:?} based on {priority_info:?}");
        let endpoint = endpoint?;
        Some(endpoint)
    }
}

impl<B> DefaultBalancer<B, LbEndpoint>
where
    B: Balancer<LbEndpoint> + FromIterator<Arc<LbEndpoint>> + Default,
{
    pub fn from_slice(endpoints: &[LocalityLbEndpoints]) -> Self {
        let mut priorities = HashMap::default();
        for endpoint in endpoints {
            let total = endpoint.total_endpoints;
            let healthy = endpoint.healthy_endpoints;
            priorities
                .entry(endpoint.priority)
                .and_modify(|priority_info: &mut PriorityInfo<HealthyBalancer<B, LbEndpoint>>| {
                    priority_info.healthy += endpoint.healthy_endpoints;
                    priority_info.total += endpoint.total_endpoints;
                    priority_info.balancer.extend(endpoint.endpoints.iter().cloned());
                    debug!("Priority info {} {} {}", endpoint.priority, priority_info.healthy, priority_info.total);
                })
                .or_insert(PriorityInfo { balancer: endpoint.endpoints.iter().cloned().collect(), healthy, total });
        }

        Self { priority_level_lb: Self::recalculate_priority_load_factors(&priorities), priorities, _type: PhantomData }
    }
}

#[cfg(test)]
mod test {
    use orion_configuration::config::{cluster::HttpProtocolOptions, core::envoy_conversions::Address, transport::BindDeviceOptions};
    use std::sync::Arc;

    use super::DefaultBalancer;
    use crate::{
        clusters::{
            balancers::{wrr::WeightedRoundRobinBalancer, Balancer},
            health::HealthStatus,
            load_assignment::{LbEndpoint, LocalityLbEndpoints},
        },
        transport::UpstreamTransportSocketConfigurator,
    };
    type TestpointData = (u32, u32, Vec<(http::uri::Authority, u32, HealthStatus)>);

    fn get_locality_endpoints(data: Vec<TestpointData>) -> Vec<LocalityLbEndpoints> {
        let mut loc_lb_endpoints = vec![];
        for (_, priority, endpoints) in data {
            let mut lb_endpoints = vec![];
            let len = endpoints.len();
            let mut healthy = 0;
            for (auth, weight, health_status) in endpoints {
                if health_status == HealthStatus::Healthy {
                    healthy += 1;
                }
                let address = Address::Socket(auth.host().to_owned(), auth.port_u16().unwrap());
                lb_endpoints.push(Arc::new(LbEndpoint::new(
                    auth,
                    address,
                    "test_cluster",
                    BindDeviceOptions::default(),
                    weight,
                    health_status,
                )));
            }

            loc_lb_endpoints.push(LocalityLbEndpoints {
                name: "Cluster1",
                endpoints: lb_endpoints,
                priority,
                healthy_endpoints: healthy,
                total_endpoints: u32::try_from(len).expect("Too many endpoints"),
                transport_socket: UpstreamTransportSocketConfigurator::None,
                http_protocol_options: HttpProtocolOptions::default(),
                connection_timeout: None,
            });
        }
        loc_lb_endpoints
    }

    #[test]
    fn test_default_loadbalancer_with_wrr_and_all_healthy_priority_not_contigous() {
        let data = vec![
            (
                1,
                1,
                vec![
                    ("endpoint11:8000".parse().unwrap(), 1, HealthStatus::Healthy),
                    ("endpoint12:8000".parse().unwrap(), 1, HealthStatus::Healthy),
                ],
            ),
            (
                1,
                3,
                vec![
                    ("endpoint21:8000".parse().unwrap(), 1, HealthStatus::Healthy),
                    ("endpoint22:8000".parse().unwrap(), 1, HealthStatus::Healthy),
                ],
            ),
            (
                1,
                5,
                vec![
                    ("endpoint31:8000".parse().unwrap(), 1, HealthStatus::Healthy),
                    ("endpoint32:8000".parse().unwrap(), 1, HealthStatus::Healthy),
                ],
            ),
        ];

        let endpoints = get_locality_endpoints(data);
        let mut default_balancer: DefaultBalancer<WeightedRoundRobinBalancer<LbEndpoint>, LbEndpoint> =
            DefaultBalancer::from_slice(&endpoints);
        let mut results = vec![];
        for _ in 0..10 {
            let next = default_balancer.next_item(None);
            println!("{next:?}");
            results.push(next);
        }

        let results: Vec<_> = results.into_iter().filter_map(|r| r.map(|f| f.authority.to_string())).collect();
        let expected = [
            "endpoint11:8000",
            "endpoint12:8000",
            "endpoint11:8000",
            "endpoint12:8000",
            "endpoint11:8000",
            "endpoint12:8000",
            "endpoint11:8000",
            "endpoint12:8000",
            "endpoint11:8000",
            "endpoint12:8000",
        ];
        assert_eq!(results, expected);
    }

    #[test]
    fn test_default_loadbalancer_with_wrr_and_all_healthy_one_priority() {
        let data = vec![
            (
                1,
                0,
                vec![
                    ("endpoint11:8000".parse().unwrap(), 1, HealthStatus::Healthy),
                    ("endpoint12:8000".parse().unwrap(), 1, HealthStatus::Healthy),
                ],
            ),
            (
                1,
                1,
                vec![
                    ("endpoint21:8000".parse().unwrap(), 1, HealthStatus::Healthy),
                    ("endpoint22:8000".parse().unwrap(), 1, HealthStatus::Healthy),
                ],
            ),
            (
                1,
                2,
                vec![
                    ("endpoint31:8000".parse().unwrap(), 1, HealthStatus::Healthy),
                    ("endpoint32:8000".parse().unwrap(), 1, HealthStatus::Healthy),
                ],
            ),
        ];

        let endpoints = get_locality_endpoints(data);
        let mut default_balancer: DefaultBalancer<WeightedRoundRobinBalancer<LbEndpoint>, LbEndpoint> =
            DefaultBalancer::from_slice(&endpoints);
        let mut results = vec![];
        for _ in 0..10 {
            let next = default_balancer.next_item(None);
            println!("{next:?}");
            results.push(next);
        }

        let results: Vec<_> = results.into_iter().filter_map(|r| r.map(|f| f.authority.to_string())).collect();
        let expected = [
            "endpoint11:8000",
            "endpoint12:8000",
            "endpoint11:8000",
            "endpoint12:8000",
            "endpoint11:8000",
            "endpoint12:8000",
            "endpoint11:8000",
            "endpoint12:8000",
            "endpoint11:8000",
            "endpoint12:8000",
        ];
        assert_eq!(results, expected);
    }

    #[test]
    fn test_default_loadbalancer_with_wrr_and_all_healthy_one_priority_two_groups() {
        let data = vec![
            (
                1,
                0,
                vec![
                    ("endpoint11:8000".parse().unwrap(), 1, HealthStatus::Healthy),
                    ("endpoint12:8000".parse().unwrap(), 1, HealthStatus::Healthy),
                ],
            ),
            (
                1,
                0,
                vec![
                    ("endpoint21:8000".parse().unwrap(), 1, HealthStatus::Healthy),
                    ("endpoint22:8000".parse().unwrap(), 1, HealthStatus::Healthy),
                ],
            ),
            (
                1,
                2,
                vec![
                    ("endpoint31:8000".parse().unwrap(), 1, HealthStatus::Healthy),
                    ("endpoint32:8000".parse().unwrap(), 1, HealthStatus::Healthy),
                ],
            ),
        ];

        let endpoints = get_locality_endpoints(data);
        let mut default_balancer: DefaultBalancer<WeightedRoundRobinBalancer<LbEndpoint>, LbEndpoint> =
            DefaultBalancer::from_slice(&endpoints);
        let mut results = vec![];
        for _ in 0..10 {
            let next = default_balancer.next_item(None);
            println!("{next:?}");
            results.push(next);
        }

        let results: Vec<_> = results.into_iter().filter_map(|r| r.map(|f| f.authority.to_string())).collect();
        let expected = [
            "endpoint11:8000",
            "endpoint12:8000",
            "endpoint21:8000",
            "endpoint22:8000",
            "endpoint11:8000",
            "endpoint12:8000",
            "endpoint21:8000",
            "endpoint22:8000",
            "endpoint11:8000",
            "endpoint12:8000",
        ];
        assert_eq!(results, expected);
    }

    #[test]
    fn test_default_loadbalancer_with_wrr_and_all_healthy_one_priority_two_different_groups() {
        let data = vec![
            (
                1,
                0,
                vec![
                    ("endpoint11:8000".parse().unwrap(), 1, HealthStatus::Healthy),
                    ("endpoint12:8000".parse().unwrap(), 1, HealthStatus::Healthy),
                ],
            ),
            (
                2,
                0,
                vec![
                    ("endpoint21:8000".parse().unwrap(), 2, HealthStatus::Healthy),
                    ("endpoint22:8000".parse().unwrap(), 2, HealthStatus::Healthy),
                ],
            ),
            (
                1,
                2,
                vec![
                    ("endpoint31:8000".parse().unwrap(), 1, HealthStatus::Healthy),
                    ("endpoint32:8000".parse().unwrap(), 1, HealthStatus::Healthy),
                ],
            ),
        ];

        let endpoints = get_locality_endpoints(data);
        let mut default_balancer: DefaultBalancer<WeightedRoundRobinBalancer<LbEndpoint>, LbEndpoint> =
            DefaultBalancer::from_slice(&endpoints);
        let mut results = vec![];
        for _ in 0..10 {
            let next = default_balancer.next_item(None);
            println!("{next:?}");
            results.push(next);
        }

        let results: Vec<_> = results.into_iter().filter_map(|r| r.map(|f| f.authority.to_string())).collect();
        let expected = [
            "endpoint21:8000",
            "endpoint22:8000",
            "endpoint11:8000",
            "endpoint12:8000",
            "endpoint21:8000",
            "endpoint22:8000",
            "endpoint21:8000",
            "endpoint22:8000",
            "endpoint11:8000",
            "endpoint12:8000",
        ];
        assert_eq!(results, expected);
    }

    #[test]
    fn test_default_loadbalancer_with_wrr_and_all_healthy_one_priority_two_different_groups_2() {
        let data = vec![
            (
                1,
                0,
                vec![
                    ("endpoint11:8000".parse().unwrap(), 1, HealthStatus::Healthy),
                    ("endpoint12:8000".parse().unwrap(), 1, HealthStatus::Healthy),
                ],
            ),
            (
                2,
                0,
                vec![
                    ("endpoint21:8000".parse().unwrap(), 4, HealthStatus::Healthy),
                    ("endpoint22:8000".parse().unwrap(), 2, HealthStatus::Healthy),
                ],
            ),
            (
                1,
                2,
                vec![
                    ("endpoint31:8000".parse().unwrap(), 1, HealthStatus::Healthy),
                    ("endpoint32:8000".parse().unwrap(), 1, HealthStatus::Healthy),
                ],
            ),
        ];

        let endpoints = get_locality_endpoints(data);
        let mut default_balancer: DefaultBalancer<WeightedRoundRobinBalancer<LbEndpoint>, LbEndpoint> =
            DefaultBalancer::from_slice(&endpoints);
        let mut results = vec![];
        for _ in 0..10 {
            let next = default_balancer.next_item(None);
            println!("{next:?}");
            results.push(next);
        }

        let results: Vec<_> = results.into_iter().filter_map(|r| r.map(|f| f.authority.to_string())).collect();
        let expected = [
            "endpoint21:8000",
            "endpoint22:8000",
            "endpoint21:8000",
            "endpoint11:8000",
            "endpoint12:8000",
            "endpoint21:8000",
            "endpoint22:8000",
            "endpoint21:8000",
            "endpoint21:8000",
            "endpoint22:8000",
        ];
        assert_eq!(results, expected);
    }

    #[test]
    fn test_default_loadbalancer_with_wrr_and_unhealthy_two_priority_groups() {
        let data = vec![
            (
                1,
                0,
                vec![
                    ("endpoint11:8000".parse().unwrap(), 1, HealthStatus::Unhealthy),
                    ("endpoint12:8000".parse().unwrap(), 1, HealthStatus::Unhealthy),
                ],
            ),
            (
                2,
                1,
                vec![
                    ("endpoint21:8000".parse().unwrap(), 1, HealthStatus::Healthy),
                    ("endpoint22:8000".parse().unwrap(), 1, HealthStatus::Healthy),
                ],
            ),
            (
                1,
                2,
                vec![
                    ("endpoint31:8000".parse().unwrap(), 1, HealthStatus::Healthy),
                    ("endpoint32:8000".parse().unwrap(), 1, HealthStatus::Healthy),
                ],
            ),
        ];

        let endpoints = get_locality_endpoints(data);
        let mut default_balancer: DefaultBalancer<WeightedRoundRobinBalancer<LbEndpoint>, LbEndpoint> =
            DefaultBalancer::from_slice(&endpoints);
        let mut results = vec![];
        for _ in 0..10 {
            let next = default_balancer.next_item(None);
            println!("{next:?}");
            results.push(next);
        }

        let results: Vec<_> = results.into_iter().filter_map(|r| r.map(|f| f.authority.to_string())).collect();
        let expected = [
            "endpoint21:8000",
            "endpoint22:8000",
            "endpoint21:8000",
            "endpoint22:8000",
            "endpoint21:8000",
            "endpoint22:8000",
            "endpoint21:8000",
            "endpoint22:8000",
            "endpoint21:8000",
            "endpoint22:8000",
        ];
        assert_eq!(results, expected);
    }

    #[test]
    fn test_default_loadbalancer_with_wrr_and_unhealthy_three_priority_groups() {
        let data = vec![
            (
                1,
                0,
                vec![
                    ("endpoint11:8000".parse().unwrap(), 1, HealthStatus::Unhealthy),
                    ("endpoint12:8000".parse().unwrap(), 1, HealthStatus::Unhealthy),
                ],
            ),
            (
                1,
                1,
                vec![
                    ("endpoint21:8000".parse().unwrap(), 1, HealthStatus::Healthy),
                    ("endpoint22:8000".parse().unwrap(), 1, HealthStatus::Unhealthy),
                ],
            ),
            (
                1,
                2,
                vec![
                    ("endpoint31:8000".parse().unwrap(), 1, HealthStatus::Healthy),
                    ("endpoint32:8000".parse().unwrap(), 1, HealthStatus::Healthy),
                ],
            ),
        ];

        let endpoints = get_locality_endpoints(data);
        let mut default_balancer: DefaultBalancer<WeightedRoundRobinBalancer<LbEndpoint>, LbEndpoint> =
            DefaultBalancer::from_slice(&endpoints);
        let mut results = vec![];
        for _ in 0..10 {
            let next = default_balancer.next_item(None);
            println!("{next:?}");
            results.push(next);
        }

        let results: Vec<_> = results.into_iter().filter_map(|r| r.map(|f| f.authority.to_string())).collect();
        let expected = [
            "endpoint21:8000",
            "endpoint31:8000",
            "endpoint21:8000",
            "endpoint21:8000",
            "endpoint21:8000",
            "endpoint32:8000",
            "endpoint21:8000",
            "endpoint21:8000",
            "endpoint31:8000",
            "endpoint21:8000",
        ];
        assert_eq!(results, expected);
    }
}
