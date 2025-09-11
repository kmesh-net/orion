// Copyright 2025 The kmesh Authors
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

use std::{fmt::Debug, sync::Arc};

use super::{Balancer, WeightedEndpoint};
use crate::{
    clusters::health::{EndpointHealth, HealthStatus, ValueUpdated},
    Result,
};

#[derive(Clone, Debug)]
pub struct LbItem<E> {
    item: Arc<E>,
    health: HealthStatus,
}

impl<E> LbItem<E> {
    pub fn new(health: HealthStatus, item: Arc<E>) -> Self {
        Self { item, health }
    }
}

#[derive(Clone, Debug)]
pub struct HealthyBalancer<B, E> {
    items: Vec<LbItem<E>>,
    balancer: B,
}

impl<B, E> HealthyBalancer<B, E>
where
    B: Balancer<E> + FromIterator<Arc<E>>,
    E: WeightedEndpoint,
{
    pub fn new(items: impl IntoIterator<Item = LbItem<E>>) -> Self
    where
        B: Default,
    {
        let mut this = Self { items: items.into_iter().collect(), balancer: B::default() };
        this.reload();
        this
    }

    pub fn next_item(&mut self, hash: Option<u64>) -> Option<Arc<E>> {
        self.balancer.next_item(hash)
    }

    pub fn update_health(&mut self, id: &E, health: HealthStatus) -> Result<ValueUpdated>
    where
        E: Debug + PartialEq,
    {
        if let Some(endpoint) = self.items.iter_mut().find(|f| id == f.item.as_ref()) {
            let updated = endpoint.health.update_health(health);
            if updated == ValueUpdated::Updated {
                self.reload();
            }
            Ok(updated)
        } else {
            Err(format!("Can't find endpoint {id:?}").into())
        }
    }

    fn reload(&mut self) {
        self.balancer =
            self.items.iter().filter_map(|item| item.health.is_healthy().then_some(Arc::clone(&item.item))).collect();
    }
}

impl<B, E> HealthyBalancer<B, E>
where
    B: Balancer<E> + FromIterator<Arc<E>>,
    E: EndpointHealth + WeightedEndpoint,
{
    pub fn extend(&mut self, items: impl Iterator<Item = Arc<E>>) {
        self.items.extend(items.map(|i| LbItem::new(i.health(), i)));
        self.reload();
    }
}

impl<B, E> FromIterator<Arc<E>> for HealthyBalancer<B, E>
where
    B: Default + Balancer<E> + FromIterator<Arc<E>>,
    E: EndpointHealth + WeightedEndpoint,
{
    fn from_iter<T: IntoIterator<Item = Arc<E>>>(iter: T) -> Self {
        Self::new(iter.into_iter().map(|item| LbItem::new(item.health(), item)))
    }
}

impl<B, E> Balancer<E> for HealthyBalancer<B, E>
where
    B: Balancer<E> + FromIterator<Arc<E>>,
    E: WeightedEndpoint,
{
    fn next_item(&mut self, hash: Option<u64>) -> Option<Arc<E>> {
        self.next_item(hash)
    }
}

#[cfg(test)]
mod test {
    use std::sync::Arc;

    use crate::clusters::{
        balancers::{healthy::HealthyBalancer, wrr::WeightedRoundRobinBalancer, WeightedEndpoint},
        health::HealthStatus,
    };

    use super::LbItem;

    #[derive(Debug, Clone, PartialEq)]
    struct Blah {
        value: u32,
        weight: u32,
    }

    impl WeightedEndpoint for Blah {
        fn weight(&self) -> u32 {
            self.weight
        }
    }

    type TestBalancer<E> = HealthyBalancer<WeightedRoundRobinBalancer<E>, E>;

    /// Asserts that `balancer` generates the items in `expected_items` in the same sequential order,
    /// but allowing for rotation and cycling. For example, for `expected_items = [a, b, c]` valid
    /// outputs of the balancer are `[a, b, c, a, b]`, `[b, c, a, b, c]` and `[c, a, b, c, a]`.
    /// This makes this test resilient to a Round-Robin that randomizes the start of the sequence.
    fn compare_rotated(balancer: &mut TestBalancer<Blah>, expected_items: Vec<&Arc<Blah>>) {
        let selected_items: Vec<_> = (0..5).map(|_| balancer.next_item(None)).collect();
        let selected: Vec<_> = selected_items.iter().map(|item| item.as_ref()).collect();

        if expected_items.is_empty() {
            assert!(selected_items.iter().all(Option::is_none));
            return;
        }

        let mut expected = expected_items.into_iter().map(Some).cycle().peekable();

        while selected.first() != expected.peek() {
            expected.next();
        }

        assert!(selected.into_iter().zip(expected).all(|(a, b)| a == b));
    }

    #[test]
    pub fn test_healthy_balancer_health_updates() {
        let ab0 = Arc::new(Blah { value: 0, weight: 1 });
        let ab1 = Arc::new(Blah { value: 1, weight: 1 });
        let ab2 = Arc::new(Blah { value: 2, weight: 1 });

        let mut balancer = TestBalancer::new(
            [&ab0, &ab1, &ab2].into_iter().cloned().map(|item| LbItem::new(HealthStatus::Healthy, item)),
        );

        // All items are healthy
        compare_rotated(&mut balancer, vec![&ab0, &ab1, &ab2]);

        // Make item 0 unhealthy
        balancer.update_health(ab0.as_ref(), HealthStatus::Unhealthy).unwrap();
        compare_rotated(&mut balancer, vec![&ab1, &ab2]);

        // Make item 1 unhealthy
        balancer.update_health(ab1.as_ref(), HealthStatus::Unhealthy).unwrap();
        compare_rotated(&mut balancer, vec![&ab2]);

        // Make item 2 unhealthy
        balancer.update_health(ab2.as_ref(), HealthStatus::Unhealthy).unwrap();
        compare_rotated(&mut balancer, vec![]);

        // Make item 0 healthy
        balancer.update_health(ab0.as_ref(), HealthStatus::Healthy).unwrap();
        compare_rotated(&mut balancer, vec![&ab0]);

        // Make item 1 healthy
        balancer.update_health(ab1.as_ref(), HealthStatus::Healthy).unwrap();
        compare_rotated(&mut balancer, vec![&ab0, &ab1]);

        // Make item 2 healthy
        balancer.update_health(ab2.as_ref(), HealthStatus::Healthy).unwrap();
        compare_rotated(&mut balancer, vec![&ab0, &ab1, &ab2]);
    }
}
