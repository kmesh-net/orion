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

use rand::{
    distributions::{Distribution, WeightedIndex},
    rngs::SmallRng,
    SeedableRng,
};

use super::{default_balancer::LbItem, Balancer, WeightedEndpoint};

#[derive(Debug, Clone)]
pub struct RandomBalancer<E> {
    items: Vec<LbItem<E>>,
    weighted_index: Option<WeightedIndex<u32>>,
    rng: SmallRng,
}

impl<E> RandomBalancer<E> {
    #[allow(clippy::expect_used)]
    pub fn new(items: impl IntoIterator<Item = LbItem<E>>) -> Self {
        let rng = SmallRng::from_rng(rand::thread_rng()).expect("RNG must be valid");
        RandomBalancer::new_with_rng(items, rng)
    }

    fn new_with_rng(items: impl IntoIterator<Item = LbItem<E>>, rng: SmallRng) -> Self {
        let mut balancer = RandomBalancer { items: collect_checked(items), weighted_index: None, rng };
        balancer.weighted_index = WeightedIndex::new(balancer.items.iter().map(|item| item.weight)).ok();
        balancer
    }
}

/// Returns a valid subset of the items whose total weight fits in [u32].
fn collect_checked<E>(items: impl IntoIterator<Item = LbItem<E>>) -> Vec<LbItem<E>> {
    let mut total = 0_u32;
    items
        .into_iter()
        .take_while(|item| {
            let result = total.checked_add(item.weight);
            if let Some(new_total) = result {
                total = new_total;
            } else {
                tracing::warn!("Endpoint weight overflow in random load balancer, will only use the endpoints whose weight sum fits in 32 bits");
            }
            result.is_some()
        })
        .collect()
}

impl<E: WeightedEndpoint> Default for RandomBalancer<E> {
    fn default() -> Self {
        Self::from_iter([])
    }
}

impl<E> Balancer<E> for RandomBalancer<E> {
    fn next_item(&mut self, _hash: Option<u64>) -> Option<Arc<E>> {
        self.items.get(self.weighted_index.as_ref()?.sample(&mut self.rng)).map(|item| &item.item).cloned()
    }
}

impl<E: WeightedEndpoint> FromIterator<Arc<E>> for RandomBalancer<E> {
    fn from_iter<T: IntoIterator<Item = Arc<E>>>(iter: T) -> Self {
        RandomBalancer::new(iter.into_iter().map(|item| LbItem::new(item.weight(), item)))
    }
}

#[cfg(test)]
mod test {
    use std::sync::Arc;

    use rand::{rngs::SmallRng, SeedableRng};

    use crate::clusters::balancers::{
        random::{LbItem, RandomBalancer},
        Balancer,
    };

    #[test]
    pub fn test_random_balancer_1() {
        #[derive(Debug, Clone, PartialEq)]
        struct Blah {
            value: u32,
        }
        let b1 = Blah { value: 0 };
        let b2 = Blah { value: 1 };
        let b3 = Blah { value: 2 };

        let ab1 = Arc::new(b1);
        let ab2 = Arc::new(b2);
        let ab3 = Arc::new(b3);

        let expected_b1 = Arc::clone(&ab1);
        let expected_b2 = Arc::clone(&ab2);
        let expected_b3 = Arc::clone(&ab3);

        let items = [LbItem::new(1, ab1), LbItem::new(1, ab2), LbItem::new(1, ab3)];

        let gen = SmallRng::seed_from_u64(1);
        let mut random_lb = RandomBalancer::new_with_rng(items, gen);
        let mut selected_items = vec![];
        for _n in 0..10 {
            selected_items.push(random_lb.next_item(None));
        }
        let selected_items: Vec<_> = selected_items.into_iter().flatten().collect();
        println!("{selected_items:?}");

        assert_eq!(
            selected_items,
            vec![
                Arc::clone(&expected_b3),
                Arc::clone(&expected_b1),
                Arc::clone(&expected_b1),
                Arc::clone(&expected_b2),
                Arc::clone(&expected_b2),
                Arc::clone(&expected_b3),
                Arc::clone(&expected_b1),
                Arc::clone(&expected_b2),
                Arc::clone(&expected_b1),
                Arc::clone(&expected_b3)
            ]
        );
    }

    #[test]
    pub fn test_random_balancer_2() {
        let items = [LbItem::new(1, Arc::new(0)), LbItem::new(1, Arc::new(1)), LbItem::new(2, Arc::new(2))];
        let mut counts = vec![0_u32; items.len()];

        let gen = SmallRng::seed_from_u64(1);
        let mut random_lb = RandomBalancer::new_with_rng(items, gen);

        for _n in 0..20 {
            counts[random_lb.next_item(None).map(|item| *item).unwrap()] += 1;
        }

        assert_eq!(counts, vec![5, 6, 9]);
    }

    #[test]
    pub fn test_random_balancer_3() {
        let items = [LbItem::new(1, Arc::new(0)), LbItem::new(2, Arc::new(1)), LbItem::new(4, Arc::new(2))];
        let mut counts = vec![0_u32; items.len()];

        let gen = SmallRng::seed_from_u64(1);
        let mut random_lb = RandomBalancer::new_with_rng(items, gen);
        let mut selected_items = vec![];
        for _n in 0..20 {
            selected_items.push(random_lb.next_item(None));
        }
        let selected_items: Vec<_> = selected_items.into_iter().flatten().collect();

        for i in selected_items {
            counts[*i] += 1;
        }

        assert_eq!(counts, vec![2, 6, 12]);
    }

    #[test]
    fn random_balancer_weight_overflow() {
        let items = [
            LbItem::new(u32::MAX / 2, Arc::new(0)),
            LbItem::new(u32::MAX / 2, Arc::new(1)),
            LbItem::new(1, Arc::new(2)),
        ];
        let mut counts = vec![0_u32; items.len()];
        let gen = SmallRng::seed_from_u64(1);
        let mut random_lb = RandomBalancer::new_with_rng(items, gen);
        for _ in 0..20 {
            counts[*random_lb.next_item(None).unwrap()] += 1;
        }
        assert!(counts[0] > 0, "expected first item to be selected");
        assert!(counts[1] > 0, "expected second item to be selected");
        assert_eq!(counts[2], 0, "expected third item to not be selected");
    }
}
