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

use std::{
    fmt::Debug,
    hash::{Hash, Hasher},
    sync::Arc,
};

use rand::Rng;

use super::{
    default_balancer::{EndpointWithAuthority, LbItem},
    hash_policy::DeterministicBuildHasher,
    Balancer, WeightedEndpoint,
};

/// A consistent balancer based on the "ketama hash" algorithm.
///
/// A ring contains slots proportional to the weight of each endpoint. When doing load balancing,
/// one slot is selected based on the closest hash of the request.
///
/// When there are not enough slots in the ring, some endpoints don't get one. This can happen
/// because there are more endpoints than slots, or because one endpoint has an exaggerated weight
/// over the others. Slots are assigned on a FIFO basis, so for a ring size of 20,
/// `[(endpoint0, 50), (endpoint1, 1)]` should only contain one endpoint, while
/// `[(endpoint1, 1), (endpoint0, 50)]` should contain both.
#[derive(Debug, Clone)]
pub struct RingHashBalancer<E> {
    items: Vec<LbItem<E>>,
    ring: Vec<(u64, usize)>,
}

const DEFAULT_MIN_RING_SIZE: u32 = 1024;
const DEFAULT_MAX_RING_SIZE: u32 = 1024 * 1024 * 8;

impl<E> RingHashBalancer<E>
where
    E: EndpointWithAuthority,
{
    pub fn new(items: impl IntoIterator<Item = LbItem<E>>) -> Self {
        Self::new_with_settings(items, DEFAULT_MIN_RING_SIZE, DEFAULT_MAX_RING_SIZE)
    }

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn new_with_settings(items: impl IntoIterator<Item = LbItem<E>>, min_ring_size: u32, max_ring_size: u32) -> Self {
        let (items, total_weight) = collect_checked(items);

        // Calculate the size of the ring so all endpoints have at least one slot
        let total_weight = f64::from(total_weight);
        let normalized_weights: Vec<f64> = items.iter().map(|item| f64::from(item.weight) / total_weight).collect();

        let mut ring = Vec::new();
        if let Some(minimum_weight) = normalized_weights.iter().copied().reduce(f64::min) {
            let scale =
                ((minimum_weight * f64::from(min_ring_size)).ceil() / minimum_weight).min(f64::from(max_ring_size));

            let ring_size = scale.ceil() as usize; // there is no usize::try_from::<f64>() yet
            ring.reserve(ring_size);

            // Add entries to the ring until it reaches the target number according to
            // the weight and the scale factor for this item.
            let mut current_slots = 0.0;
            let mut target_slots = 0.0;
            for (index, (item, weight)) in items.iter().zip(normalized_weights).enumerate() {
                let item_key = item.item.authority();

                let item_slots = weight * scale;
                target_slots += item_slots;

                let mut slot_index = 0;
                while current_slots < target_slots {
                    let mut hasher = DeterministicBuildHasher::build_hasher();
                    item_key.hash(&mut hasher);
                    slot_index.hash(&mut hasher);
                    let hash = hasher.finish();

                    ring.push((hash, index));
                    current_slots += 1.0;
                    slot_index += 1;
                }
            }

            // The ring has to be sorted for the binary search in `next_item()` to work
            ring.sort_by_key(|(hash, _)| *hash);
        }

        RingHashBalancer { items, ring }
    }
}

/// Returns a valid subset of the items whose total weight fits in [u32].
fn collect_checked<E>(items: impl IntoIterator<Item = LbItem<E>>) -> (Vec<LbItem<E>>, u32) {
    let mut total = 0_u32;
    let sanitized_subset = items
        .into_iter()
        .take_while(|item| {
            let result = total.checked_add(item.weight);
            if let Some(new_total) = result {
                total = new_total;
            } else {
                tracing::warn!("Endpoint weight overflow in ring hash load balancer, will only use the endpoints whose weight sum fits in 32 bits");
            }
            result.is_some()
        })
        .collect();
    (sanitized_subset, total)
}

impl<E: EndpointWithAuthority> Default for RingHashBalancer<E> {
    fn default() -> Self {
        Self::new([])
    }
}

impl<T> Balancer<T> for RingHashBalancer<T> {
    fn next_item(&mut self, hash: Option<u64>) -> Option<Arc<T>> {
        if self.items.len() <= 1 {
            return self.items.first().map(|lb_item| &lb_item.item).cloned();
        }

        // If no hash is provided, a random one is generated
        let hash = hash.unwrap_or(rand::rng().random());

        // Find the closest entry doing a binary search of the hash
        let ring_index = match self.ring.binary_search_by_key(&hash, |(hash, _)| *hash) {
            Ok(matching_index) => matching_index,
            Err(closest_index) => closest_index.min(self.ring.len() - 1),
        };

        let (_, index) = self.ring.get(ring_index)?;

        self.items.get(*index).map(|lb_item| &lb_item.item).cloned()
    }
}

impl<E: WeightedEndpoint + EndpointWithAuthority> FromIterator<Arc<E>> for RingHashBalancer<E> {
    fn from_iter<T: IntoIterator<Item = Arc<E>>>(iter: T) -> Self {
        Self::new(iter.into_iter().map(|item| LbItem::new(item.weight(), item)))
    }
}

#[cfg(test)]
mod test {
    use std::{ops::ControlFlow, sync::Arc};

    use http::uri::Authority;
    use rand::{rngs::SmallRng, Rng, SeedableRng};

    use crate::clusters::balancers::{Balancer, EndpointWithAuthority};

    use super::{LbItem, RingHashBalancer};

    struct TestEndpoint {
        value: u32,
        authority: Authority,
    }

    impl TestEndpoint {
        fn new(value: u32, authority: Authority) -> Self {
            Self { value, authority }
        }
    }

    impl EndpointWithAuthority for TestEndpoint {
        fn authority(&self) -> &Authority {
            &self.authority
        }
    }

    fn is_sorted<T: Ord>(mut iter: impl Iterator<Item = T>) -> bool {
        if let Some(first) = iter.next() {
            iter.try_fold(
                first,
                |prev, current| if prev <= current { ControlFlow::Continue(current) } else { ControlFlow::Break(()) },
            )
            .is_continue()
        } else {
            true
        }
    }

    fn get_authority(port: u32) -> Authority {
        Authority::try_from(format!("example.com:{port}")).unwrap()
    }

    fn balancer_from_distribution(
        distribution: &[(u32, u32)],
        ring_min: u32,
        ring_max: u32,
    ) -> RingHashBalancer<TestEndpoint> {
        let (values, weights): (Vec<_>, Vec<_>) = distribution.iter().copied().unzip();

        let items: Vec<_> =
            values.iter().map(|value| Arc::new(TestEndpoint::new(*value, get_authority(8000 + value)))).collect();

        let lb_items = items.iter().cloned().zip(weights.iter()).map(|(item, weight)| LbItem::new(*weight, item));

        let balancer = RingHashBalancer::new_with_settings(lb_items, ring_min, ring_max);

        assert!(is_sorted(balancer.ring.iter().map(|(hash, _weight)| hash)), "ring is not sorted");

        balancer
    }

    fn distribution_within_margin(a: &[f64], b: &[f64], error_margin: f64) {
        for (value_a, value_b) in a.iter().zip(b.iter()) {
            assert!(
                (value_a - value_b).abs() / value_b < error_margin,
                "Value {} is {:+.1}% of {} which is not within {:.1}%, when comparing [{}] to [{}]",
                value_a,
                (value_a - value_b) * 100. / value_b,
                value_b,
                error_margin * 100.0,
                a.iter().map(|count| format!("{:.1}%", count * 100.)).collect::<Vec<_>>().join(", "),
                b.iter().map(|count| format!("{:.1}%", count * 100.)).collect::<Vec<_>>().join(", "),
            );
        }
    }

    #[test]
    fn ring_balancer_empty() {
        let mut empty_balancer: RingHashBalancer<TestEndpoint> = RingHashBalancer::default();

        assert!(empty_balancer.next_item(None).is_none(), "unexpected output from empty load balancer");
    }

    #[test]
    fn ring_balancer_weight_overflow() {
        const RING_MIN: u32 = 1;
        const RING_MAX: u32 = 10;

        // Weights of 0 + 1 fit, weight of 2 overflows
        let distribution: Vec<_> = vec![(0, u32::MAX - 2), (1, 1), (2, 2)];

        let balancer = balancer_from_distribution(&distribution, RING_MIN, RING_MAX);

        assert_eq!(balancer.items.len(), 2, "balancer did not detect a weight overflow");
    }

    #[test]
    fn ring_balancer_distribution() {
        const RING_MIN: u32 = 10;
        const RING_MAX: u32 = 50;

        let distributions = [
            // Should only need 10 slots
            (vec![(0, 1), (1, 1), (2, 1), (3, 1), (4, 1), (5, 1), (6, 1), (7, 1), (8, 1), (9, 1)], 0.05),
            // Should only need 16 slots
            (vec![(0, 1), (1, 2), (2, 3), (3, 1), (4, 2), (5, 3), (6, 1), (7, 2), (8, 3), (9, 1)], 0.05),
            // Should need 51 slots, which should be capped at `ring_max_size`
            // Beware: [(0, 50), (1, 1)] would fail, because slots are assigned on a FIFO basis,
            // so the 50 weight would consume all the slots, even after normalizing the weights.
            (vec![(0, 1), (1, 50)], 0.05),
            // Should need 55 slots, which should be capped at `ring_max_size`
            (vec![(0, 1), (1, 2), (2, 3), (3, 4), (4, 5), (5, 6), (6, 7), (7, 8), (8, 9), (9, 10)], 0.15),
        ];

        for (distribution, error_margin) in distributions {
            let weights: Vec<_> = distribution.iter().map(|(_value, weight)| *weight).collect();
            let total_weight: u32 = weights.iter().sum();
            let normalized_weights: Vec<_> =
                weights.iter().map(|weight| f64::from(*weight) / f64::from(total_weight)).collect();

            let balancer = balancer_from_distribution(&distribution, RING_MIN, RING_MAX);

            let mut counts = vec![0_u32; distribution.len()];

            balancer.ring.iter().for_each(|(_hash, index)| counts[balancer.items[*index].item.value as usize] += 1);
            let ring_size: u32 = counts.iter().sum();

            assert!((RING_MIN..=RING_MAX).contains(&ring_size));
            assert!(
                counts.iter().all(|count| *count > 0),
                "item is zero in {counts:?} for distribution {distribution:?}"
            );

            // Check that the ring has a distribution that fits the expected weights within 5% of error
            distribution_within_margin(
                &counts.iter().map(|count| f64::from(*count) / f64::from(ring_size)).collect::<Vec<_>>(),
                &normalized_weights,
                error_margin,
            );
        }
    }

    #[test]
    fn ring_balancer_overflow() {
        const RING_MIN: u32 = 1;
        const RING_MAX: u16 = 10;

        // When trying to insert 20 endpoints into a 10-item ring, expect that only 10 endpoints are allocated
        let distribution: Vec<_> = (0..20).map(|value| (value, 1)).collect();

        let balancer = balancer_from_distribution(&distribution, RING_MIN, u32::from(RING_MAX));

        let mut counts = vec![0_u32; distribution.len()];

        balancer.ring.iter().for_each(|(_hash, index)| counts[balancer.items[*index].item.value as usize] += 1);
        let ring_size: u32 = counts.iter().sum();

        assert!(
            (RING_MIN..=u32::from(RING_MAX)).contains(&ring_size),
            "ring size {ring_size} outside the {RING_MIN}..={RING_MAX} bounds"
        );

        assert_eq!(
            counts.iter().filter(|count| **count > 0).count(),
            usize::from(RING_MAX),
            "item is zero in {counts:?} for distribution {distribution:?}"
        );
    }

    #[test]
    fn ring_balancer_consistent() {
        const RING_MIN: u32 = 10;
        const RING_MAX: u32 = 20;

        // 10 endpoints with different weights and addresses `example.com:800X`, where X is 0..10
        let distributions = [
            [(0, 1), (1, 1), (2, 1), (3, 1), (4, 1), (5, 1), (6, 1), (7, 1), (8, 1), (9, 1)],
            [(0, 1), (1, 2), (2, 3), (3, 1), (4, 2), (5, 3), (6, 1), (7, 2), (8, 3), (9, 1)],
            [(0, 1), (1, 2), (2, 3), (3, 4), (4, 5), (5, 6), (6, 7), (7, 8), (8, 9), (9, 10)],
        ];

        for distribution in distributions {
            let mut balancer = balancer_from_distribution(&distribution, RING_MIN, RING_MAX);

            // Test 100 requests with a random hash each one
            let mut rng = SmallRng::seed_from_u64(1);
            for _ in 0..100 {
                let hash = rng.random();

                // Check that load balancing is consistent for this request
                (0..10).map(|_| balancer.next_item(Some(hash)).unwrap().value).reduce(|initial, current| {
                    assert_eq!(initial, current, "different endpoint for the same request");
                    initial
                });
            }
        }
    }
}
