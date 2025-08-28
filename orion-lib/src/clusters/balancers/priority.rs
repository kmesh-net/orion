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

use rustc_hash::FxHashMap as HashMap;

#[derive(Debug, Clone)]
pub(crate) struct PriorityInfo<B> {
    pub balancer: B,
    pub healthy: u32,
    pub total: u32,
}

pub struct Priority;

impl Priority {
    fn calculate_priority_health(healthy: u32, total: u32) -> f64 {
        let x = f64::from(100 * healthy / total);
        f64::min(100.0, 1.4 * x)
    }

    ///
    /// Implementation and test cases taken from Envoy's documentation
    /// <https://www.envoyproxy.io/docs/envoy/latest/intro/arch_overview/upstream/load_balancing/priority>
    ///
    /// health(P_X) = min(100, 1.4 * 100 * healthy_P_X_backends / total_P_X_backends)
    /// normalized_total_health = min(100, Σ(health(P_0)...health(P_X)))
    /// priority_load(P_0) = health(P_0) * 100 / normalized_total_health
    /// priority_load(P_X) = min(100 - Σ(priority_load(P_0)..priority_load(P_X-1)),
    /// health(P_X) * 100 / normalized_total_health)
    ///
    ///
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::similar_names)]
    pub fn calculate_priority_loads<T>(endpoints: &HashMap<u32, PriorityInfo<T>>) -> Vec<(u32, u32)> {
        let mut priority_health = vec![];
        let mut sorted_endpoints = endpoints.iter().collect::<Vec<_>>();
        sorted_endpoints.sort_by(|a, b| a.0.cmp(b.0));
        for (k, v) in &sorted_endpoints {
            priority_health.push((*k, Self::calculate_priority_health(v.healthy, v.total)));
        }

        let normalized_total_health = f64::min(100.0, priority_health.iter().map(|(_, p)| p).sum());
        let (p0_k, p) = priority_health.remove(0);
        let p_0_load = p * 100.0 / normalized_total_health;
        let mut p_load = vec![(p0_k, p_0_load)];

        for (k, x) in priority_health {
            let load = 100.0 - p_load.iter().map(|(_, p)| p).sum::<f64>();
            let normalized_p_x_health = x * 100.0 / normalized_total_health;
            let p_x_load = f64::min(load, normalized_p_x_health);
            p_load.push((k, p_x_load));
        }
        p_load.into_iter().map(|(k, f)| (*k, f64::round(f) as u32)).collect()
    }
}

#[cfg(test)]
mod test {

    use orion_data_plane_api::envoy_data_plane_api::envoy::config::endpoint::v3::LbEndpoint;
    use rustc_hash::FxHashMap as HashMap;

    use super::PriorityInfo;
    use crate::clusters::balancers::{priority::Priority, random::RandomBalancer};

    fn generate_endpoints(
        (p1, h1, e1): (u32, u32, u32),
        (p2, h2, e2): (u32, u32, u32),
        (p3, h3, e3): (u32, u32, u32),
    ) -> HashMap<u32, PriorityInfo<RandomBalancer<LbEndpoint>>> {
        let pi1 = PriorityInfo { balancer: RandomBalancer::new(vec![]), healthy: h1, total: e1 };

        let pi2 = PriorityInfo { balancer: RandomBalancer::new(vec![]), healthy: h2, total: e2 };

        let pi3 = PriorityInfo { balancer: RandomBalancer::new(vec![]), healthy: h3, total: e3 };
        let mut map = HashMap::default();
        map.insert(p1, pi1);
        map.insert(p2, pi2);
        map.insert(p3, pi3);
        map
    }

    #[test]
    pub fn calculate_priority_loads_test() {
        let m = generate_endpoints((0, 100, 100), (1, 100, 100), (2, 100, 100));
        let p: Vec<(_, _)> = Priority::calculate_priority_loads(&m).into_iter().collect();
        assert_eq!(p, [(0, 100), (1, 0), (2, 0)]);

        let m = generate_endpoints((0, 72, 100), (1, 72, 100), (2, 100, 100));
        let p: Vec<(_, _)> = Priority::calculate_priority_loads(&m).into_iter().collect();
        assert_eq!(p, [(0, 100), (1, 0), (2, 0)]);

        let m = generate_endpoints((0, 71, 100), (1, 71, 100), (2, 100, 100));
        let p: Vec<(_, _)> = Priority::calculate_priority_loads(&m).into_iter().collect();
        assert_eq!(p, [(0, 99), (1, 1), (2, 0)]);

        let m = generate_endpoints((0, 50, 100), (1, 50, 100), (2, 100, 100));
        let p: Vec<(_, _)> = Priority::calculate_priority_loads(&m).into_iter().collect();
        assert_eq!(p, [(0, 70), (1, 30), (2, 0)]);

        let m = generate_endpoints((0, 25, 100), (1, 100, 100), (2, 100, 100));
        let p: Vec<(_, _)> = Priority::calculate_priority_loads(&m).into_iter().collect();
        assert_eq!(p, [(0, 35), (1, 65), (2, 0)]);

        let m = generate_endpoints((0, 25, 100), (1, 25, 100), (2, 100, 100));
        let p: Vec<(_, _)> = Priority::calculate_priority_loads(&m).into_iter().collect();
        assert_eq!(p, [(0, 35), (1, 35), (2, 30)]);

        let m = generate_endpoints((0, 25, 100), (1, 25, 100), (2, 20, 100));
        let p: Vec<(_, _)> = Priority::calculate_priority_loads(&m).into_iter().collect();
        assert_eq!(p, [(0, 36), (1, 36), (2, 29)]);
    }

    #[test]
    pub fn calculate_priority_loads_test_non_contiguous_priorities() {
        let m = generate_endpoints((1, 50, 100), (3, 50, 100), (5, 100, 100));
        let p: Vec<(_, _)> = Priority::calculate_priority_loads(&m).into_iter().collect();
        assert_eq!(p, [(1, 70), (3, 30), (5, 0)]);

        let m = generate_endpoints((1, 25, 100), (3, 25, 100), (5, 100, 100));
        let p: Vec<(_, _)> = Priority::calculate_priority_loads(&m).into_iter().collect();
        assert_eq!(p, [(1, 35), (3, 35), (5, 30)]);
    }
}
