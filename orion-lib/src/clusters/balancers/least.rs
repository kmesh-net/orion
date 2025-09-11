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

use rand::{rngs::SmallRng, seq::IteratorRandom, Rng, SeedableRng};

use super::{default_balancer::EndpointWithLoad, Balancer, WeightedEndpoint};

#[derive(Clone, Debug)]
pub struct LbItem<E> {
    weight: u32,
    current_weight: f64,
    item: Arc<E>,
}

impl<E: EndpointWithLoad> LbItem<E> {
    fn new(weight: u32, item: Arc<E>) -> Self {
        Self { item, weight, current_weight: 0.0 }
    }

    fn adjust_current_weight(&mut self, value: f64) {
        self.current_weight += value;
    }

    fn load(&self) -> u32 {
        self.item.http_load()
    }

    fn weight(&self, active_request_bias: f32) -> f64 {
        if active_request_bias == 0.0 {
            return f64::from(self.weight);
        }
        f64::from(self.weight) / f64::from(self.load() + 1).powf(active_request_bias.into())
    }
}

#[derive(Clone, Debug)]
pub struct WeightedLeastRequestBalancer<E> {
    items: Vec<LbItem<E>>,
    active_request_bias: f32,
    p2c_choice_count: u32,
    all_weights_equal: bool,
    rng: SmallRng,
}

const DEFAULT_ACTIVE_REQUEST_BIAS: f32 = 1.0;
const DEFAULT_P2C_CHOICE_COUNT: u16 = 2; // u16 so it can safely be converted to u32 and usize

impl<E: EndpointWithLoad> WeightedLeastRequestBalancer<E> {
    fn new(items: impl IntoIterator<Item = LbItem<E>>) -> Self {
        Self::new_with_settings(items, DEFAULT_ACTIVE_REQUEST_BIAS, u32::from(DEFAULT_P2C_CHOICE_COUNT))
    }

    #[allow(clippy::expect_used)]
    fn new_with_settings(
        items: impl IntoIterator<Item = LbItem<E>>,
        active_request_bias: f32,
        p2c_choice_count: u32,
    ) -> Self {
        let rng = SmallRng::from_rng(rand::thread_rng()).expect("RNG must be valid");
        Self::new_with_settings_and_rng(items, active_request_bias, p2c_choice_count, rng)
    }

    fn new_with_settings_and_rng(
        items: impl IntoIterator<Item = LbItem<E>>,
        active_request_bias: f32,
        p2c_choice_count: u32,
        rng: SmallRng,
    ) -> Self {
        let items: Vec<_> = items.into_iter().collect();
        let all_weights_equal = all_equal(&items);
        WeightedLeastRequestBalancer { items, active_request_bias, p2c_choice_count, all_weights_equal, rng }
    }

    fn next_item_wrr(&mut self) -> Option<Arc<E>> {
        if self.items.len() <= 1 {
            self.items.first().map(|item| &item.item).cloned()
        } else {
            // Increase the current weight of all the endpoints and calculate the total
            let total: f64 = self
                .items
                .iter_mut()
                .map(|item| {
                    let weight = item.weight(self.active_request_bias);
                    item.adjust_current_weight(weight);
                    weight
                })
                .sum();
            // Find the item with the highest weight
            // Note: not using `max_by` here because it returns the last element for equal weights
            let best_item =
                self.items
                    .iter_mut()
                    .reduce(|best, item| if item.current_weight > best.current_weight { item } else { best });
            // Adjust its weight and return it
            best_item.map(|item| {
                item.adjust_current_weight(-total);
                Arc::clone(&item.item)
            })
        }
    }

    /// Choose one item using the Power Of Two Choice (P2C) algorithm
    fn next_item_p2c(&mut self) -> Option<Arc<E>> {
        if self.items.len() <= 1 {
            self.items.first().map(|item| &item.item).cloned()
        } else {
            // In Rust there is no safe conversion from u32 to usize
            let choice_count = usize::try_from(self.p2c_choice_count).unwrap_or(usize::from(DEFAULT_P2C_CHOICE_COUNT));

            // Randomly choose `choice_count` distinct healthy items
            let chosen_items = self.items.iter().choose_multiple(&mut self.rng, choice_count);
            let random_offset = self.rng.gen_range(0..chosen_items.len());

            // Randomly rotate the list of chosen items, because `choose_multiple()`
            // chooses random items but the order in which they are produced is not.
            let mut chosen_items = chosen_items.into_iter().cycle().skip(random_offset).take(choice_count);

            // Select the one with least load
            let first_item = chosen_items.next()?;
            let best_item = chosen_items
                .fold((first_item, first_item.load()), |(best_item, best_item_load), item| {
                    let load = item.load();
                    if load < best_item_load {
                        (item, load)
                    } else {
                        (best_item, best_item_load)
                    }
                })
                .0;
            Some(Arc::clone(&best_item.item))
        }
    }
}

impl<E: WeightedEndpoint + EndpointWithLoad> Default for WeightedLeastRequestBalancer<E> {
    fn default() -> Self {
        Self::from_iter([])
    }
}

impl<E: WeightedEndpoint + EndpointWithLoad> FromIterator<Arc<E>> for WeightedLeastRequestBalancer<E> {
    fn from_iter<T: IntoIterator<Item = Arc<E>>>(iter: T) -> Self {
        Self::new(iter.into_iter().map(|item| LbItem::new(item.weight(), item)))
    }
}

impl<E: EndpointWithLoad> Balancer<E> for WeightedLeastRequestBalancer<E> {
    fn next_item(&mut self, _hash: Option<u64>) -> Option<Arc<E>> {
        if self.all_weights_equal {
            // If all weights are equal, Least Load balancer falls back to P2C
            self.next_item_p2c()
        } else {
            // If not all weights are equal, Least Load balancer uses WRR based on load
            self.next_item_wrr()
        }
    }
}

fn all_equal<E>(items: &[LbItem<E>]) -> bool {
    let mut iter = items.iter();
    if let Some(first) = iter.next() {
        iter.all(|item| item.weight == first.weight)
    } else {
        true
    }
}

#[cfg(test)]
mod test {
    use std::sync::Arc;

    use rand::{rngs::SmallRng, SeedableRng};

    use crate::clusters::balancers::{
        least::{DEFAULT_ACTIVE_REQUEST_BIAS, DEFAULT_P2C_CHOICE_COUNT},
        Balancer, EndpointWithLoad, WeightedEndpoint,
    };

    use super::{LbItem, WeightedLeastRequestBalancer};

    #[derive(Clone, Debug, PartialEq)]
    struct TestEndpoint {
        load: Arc<usize>,
        weight: u32,
    }

    impl TestEndpoint {
        fn new(value: usize, weight: u32) -> Self {
            Self { load: Arc::new(value), weight }
        }

        pub fn value(&self) -> usize {
            *self.load
        }

        pub fn load_reference(&self) -> Arc<usize> {
            Arc::clone(&self.load)
        }
    }

    impl WeightedEndpoint for TestEndpoint {
        fn weight(&self) -> u32 {
            self.weight
        }
    }

    impl EndpointWithLoad for TestEndpoint {
        #[allow(clippy::cast_possible_truncation)]
        fn http_load(&self) -> u32 {
            Arc::strong_count(&self.load) as u32
        }
    }

    fn endpoints_from_weights<I: IntoIterator<Item = u32>>(weights: I) -> Vec<TestEndpoint> {
        weights.into_iter().enumerate().map(|(index, weight)| TestEndpoint::new(index, weight)).collect()
    }

    #[test]
    /// Adds 3 items with equal weight, expects the balancer to select them at least once after 20 requests.
    /// Then it increases the load of each item and expect it to not be selected.
    pub fn test_least_request_balancer_simple() {
        let items = endpoints_from_weights(vec![1; 3]);
        let lb_items = items.iter().cloned().map(|item| LbItem::new(item.weight(), Arc::new(item)));

        let rng = SmallRng::seed_from_u64(1);
        let mut balancer = WeightedLeastRequestBalancer::new_with_settings_and_rng(
            lb_items,
            DEFAULT_ACTIVE_REQUEST_BIAS,
            u32::from(DEFAULT_P2C_CHOICE_COUNT),
            rng,
        );
        let mut counts = vec![0; items.len()];
        (0..20).filter_map(|_| balancer.next_item(None).map(|item| item.value())).for_each(|item| counts[item] += 1);
        assert!(counts.into_iter().all(|count| count > 0));

        for busy_item in &items {
            let _load = busy_item.load_reference();
            let selected: Vec<_> = (0..10).filter_map(|_| balancer.next_item(None)).collect();
            assert!(selected.iter().all(|item| item.as_ref() != busy_item), "Found {busy_item:?} in {selected:?}");
        }
    }

    #[test]
    /// Add items with equal weight, and force the P2C algorithm to choose among all of them.
    /// Then increase the load of the second half of the items and expect them to never
    /// be selected.
    pub fn test_least_request_balancer_p2c_choice() {
        let items: Vec<_> = endpoints_from_weights(vec![1; 10]);
        let lb_items = items.iter().map(|item| LbItem::new(item.weight(), Arc::new(item.clone())));

        let rng = SmallRng::seed_from_u64(1);
        let mut balancer =
            WeightedLeastRequestBalancer::new_with_settings_and_rng(lb_items, DEFAULT_ACTIVE_REQUEST_BIAS, 10, rng);

        let _load: Vec<_> = items.iter().take(5).map(TestEndpoint::load_reference).collect();
        assert!((0..10).filter_map(|_| balancer.next_item(None).map(|item| item.value())).all(|item| item >= 5));
    }

    #[test]
    /// Expect the balancer to act as a simple weighted round-robin.
    pub fn test_least_request_balancer_weights() {
        let items: Vec<_> = endpoints_from_weights([1, 1, 2]);
        let mut counts: Vec<u32> = items.iter().map(|_| 0).collect();

        let mut balancer: WeightedLeastRequestBalancer<TestEndpoint> = items.into_iter().map(Arc::new).collect();
        for _n in 0..20 {
            // Get the content and drop the Arc reference to not increase the load factor
            if let Some(index) = balancer.next_item(None).map(|item| item.value()) {
                counts[index] += 1;
            }
        }

        assert_eq!(counts, vec![5, 5, 10]);
    }

    #[test]
    /// Just make sure that the balancer does not panic.
    pub fn test_least_request_weight_overflow() {
        let items: Vec<_> = endpoints_from_weights([u32::MAX - 1, 1, 2]);
        let mut balancer: WeightedLeastRequestBalancer<TestEndpoint> = items.into_iter().map(Arc::new).collect();
        for _n in 0..20 {
            balancer.next_item(None);
        }

        let items: Vec<_> = endpoints_from_weights(vec![u32::MAX / 4 + 1; 5]);
        let mut balancer: WeightedLeastRequestBalancer<TestEndpoint> = items.into_iter().map(Arc::new).collect();
        for _n in 0..20 {
            balancer.next_item(None);
        }
    }

    #[test]
    /// Expect items to be selected less frequently when its load is increased.
    pub fn test_least_request_balancer_weights_load() {
        fn expect_balancing(
            balancer: &mut WeightedLeastRequestBalancer<TestEndpoint>,
            requests: usize,
            expected_counts: &[usize],
        ) {
            let mut counts = vec![0; expected_counts.len()];
            for _ in 0..requests {
                // Map the Arc to its content to not increase the load factor
                if let Some(index) = balancer.next_item(None).map(|item| item.value()) {
                    counts[index] += 1;
                }
            }
            assert_eq!(counts, expected_counts);
        }
        let items = endpoints_from_weights([1, 1, 2]);
        let mut balancer: WeightedLeastRequestBalancer<TestEndpoint> = items.iter().cloned().map(Arc::new).collect();

        // No load
        expect_balancing(&mut balancer, 20, &[5, 5, 10]);

        // Increase load factor of item 0
        {
            let _load = [items[0].load_reference(), items[0].load_reference(), items[0].load_reference()];
            expect_balancing(&mut balancer, 20, &[3, 6, 11]);
        }

        // Increase load factor of item 1
        {
            let _load = [items[1].load_reference(), items[1].load_reference(), items[1].load_reference()];
            expect_balancing(&mut balancer, 20, &[5, 3, 12]);
        }

        // Increase load factor of item 2
        {
            let _load = [items[2].load_reference(), items[2].load_reference(), items[2].load_reference()];
            expect_balancing(&mut balancer, 20, &[7, 6, 7]);
        }
    }
}
