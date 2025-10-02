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

use std::{fmt::Debug, sync::Arc};

use http::uri::Authority;
use rand::Rng;

use super::{
    Balancer, WeightedEndpoint,
    default_balancer::{EndpointWithAuthority, LbItem},
    hash_policy::DeterministicBuildHasher,
};

/// A consistent balancer based on the this paper:
/// "Maglev: A Fast and Reliable Software Network Load Balancer" by Danielle E. Eisenbud et al.
///
/// The basic idea is to build a lookup table with a number of entries per each endpoint
/// proportional to its weight. Maglev tries to guarantee that each endpoint at least has one
/// entry in the table, and to distribute the entries evenly so the load balancing is
/// consistent but fair.
#[derive(Debug, Clone)]
pub struct MaglevBalancer<E> {
    items: Vec<LbItem<E>>,
    table: Vec<usize>,
}

// The building of the table requires that the table size is a prime number.
// In this case, `(1 << 16) + 1`. See section 5.3 in the original paper.
const DEFAULT_TABLE_SIZE: usize = 65537;

impl<E> MaglevBalancer<E>
where
    E: EndpointWithAuthority,
{
    pub fn new(items: impl IntoIterator<Item = LbItem<E>>) -> Self {
        Self::with_size::<DEFAULT_TABLE_SIZE>(items)
    }

    fn with_size<const TABLE_SIZE: usize>(items: impl IntoIterator<Item = LbItem<E>>) -> Self {
        let () = <Prime<TABLE_SIZE> as IsPrime>::IS_PRIME;

        let (mut items, total_weight) = collect_checked(items);

        // Sort the items so rebuilding the table will yield consistent results.
        items.sort_by(|a, b| authority_sorting_key(a.item.authority()).cmp(authority_sorting_key(b.item.authority())));

        let total_weight = f64::from(total_weight);
        let max_normalized_weight = items.iter().map(|item| f64::from(item.weight) / total_weight).reduce(f64::max);

        let table = match (items.len(), max_normalized_weight) {
            (_, None) => Vec::new(), // No items
            (1, Some(_)) => vec![0], // A single item
            (_, Some(max_normalized_weight)) => {
                let mut permutations: Vec<Permutation<TABLE_SIZE>> = items
                    .iter()
                    .map(|item| {
                        Permutation::new(
                            item.item.authority(),
                            f64::from(item.weight) / total_weight,
                            max_normalized_weight,
                        )
                    })
                    .collect();

                let mut table_builder = TableBuilder::<TABLE_SIZE>::new();

                // This implements the pseudocode from section 3.4 of the paper
                // If there is an error in the code, it could enter an infinite loop.
                // This is why there are debug assertions.
                // In production, the loop will not panic and will struggle to produce something
                // that works, even though not remotely close to the desired table.
                'table_loop: for _ in 0..TABLE_SIZE {
                    'item_loop: for (item_index, permutation) in permutations.iter_mut().enumerate() {
                        if table_builder.is_full() {
                            break 'table_loop;
                        }

                        // Skip this entry until it has accumulated enough weight
                        // The effect of this is that an item with `max_normalized_weight` will allocate
                        // an entry each iteration, while others less frequently.
                        if !permutation.has_enough_weight_after_iterating() {
                            continue;
                        }

                        // This loop is guaranteed to walk the whole table in TABLE_SIZE steps if it is a prime number
                        for _ in 0..TABLE_SIZE {
                            if table_builder.try_update(permutation.next(), item_index).is_ok() {
                                continue 'item_loop;
                            }
                        }

                        debug_assert!(false, "Maglev lookup table generator could enter an infinite loop");
                    }
                }

                table_builder.build()
            },
        };

        MaglevBalancer { items, table }
    }
}

/// Get a sorting key for [Authority] that implements [Ord]. In case you wonder,
/// this implementation is how [Authority] implements [`PartialOrd`].
fn authority_sorting_key(authority: &Authority) -> impl Iterator<Item = u8> + '_ {
    authority.as_str().as_bytes().iter().map(u8::to_ascii_lowercase)
}

struct TableBuilder<const TABLE_SIZE: usize> {
    table: Vec<Option<usize>>,
    size: usize,
}

impl<const TABLE_SIZE: usize> TableBuilder<TABLE_SIZE> {
    fn new() -> Self {
        TableBuilder { table: vec![None; TABLE_SIZE], size: 0 }
    }

    fn try_update(&mut self, index: usize, value: usize) -> Result<(), ()> {
        if let Some(entry) = self.table.get_mut(index) {
            if entry.is_none() {
                *entry = Some(value);
                self.size += 1;
                Ok(())
            } else {
                Err(())
            }
        } else {
            debug_assert!(false, "Unexpected invalid index while constructing Maglev lookup table");
            Err(())
        }
    }

    fn is_full(&self) -> bool {
        self.size >= TABLE_SIZE
    }

    fn build(&mut self) -> Vec<usize> {
        let table = self
            .table
            .iter()
            .map(|entry| {
                debug_assert!(entry.is_some(), "Incomplete Maglev lookup table");
                entry.unwrap_or_default()
            })
            .collect();
        table
    }
}

struct Permutation<const TABLE_SIZE: usize> {
    offset: usize,
    skip: usize,
    weight: f64,
    current_weight: f64,
    target_weight: f64,
    next: usize,
}

impl<const TABLE_SIZE: usize> Permutation<TABLE_SIZE> {
    const OFFSET_SEED: u64 = 0;
    const SKIP_SEED: u64 = 1;

    fn new(authority: &Authority, weight: f64, target_weight: f64) -> Self {
        let offset = usize::try_from(DeterministicBuildHasher::hash_one_with_seed(authority, Self::OFFSET_SEED))
            .unwrap_or(0)
            % TABLE_SIZE;
        let skip = (usize::try_from(DeterministicBuildHasher::hash_one_with_seed(authority, Self::SKIP_SEED))
            .unwrap_or(0)
            % (TABLE_SIZE - 1))
            + 1;
        debug_assert!((0..TABLE_SIZE).contains(&offset), "Offset is expected to be in 0..TABLE_SIZE");
        debug_assert!((1..TABLE_SIZE).contains(&skip), "Offset is expected to be in 1..TABLE_SIZE");
        Self { offset, skip, weight, current_weight: target_weight, target_weight, next: 0 }
    }

    fn has_enough_weight_after_iterating(&mut self) -> bool {
        self.current_weight += self.weight;
        let has_reached_target = self.current_weight >= self.target_weight;
        if has_reached_target {
            self.current_weight -= self.target_weight;
        }
        has_reached_target
    }

    fn next(&mut self) -> usize {
        let index = (self.offset + self.skip * self.next) % TABLE_SIZE;
        self.next += 1;
        index
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

impl<E: EndpointWithAuthority> Default for MaglevBalancer<E> {
    fn default() -> Self {
        Self::new([])
    }
}

impl<T> Balancer<T> for MaglevBalancer<T> {
    fn next_item(&mut self, hash: Option<u64>) -> Option<Arc<T>> {
        if self.items.len() <= 1 {
            return self.items.first().map(|lb_item| &lb_item.item).cloned();
        }

        // If no hash is provided, a random one is generated
        let hash = hash.unwrap_or(rand::thread_rng().r#gen());

        let table_index = usize::try_from(hash).unwrap_or(usize::MAX) % self.table.len();

        let index = self.table.get(table_index);
        index.and_then(|index| self.items.get(*index)).map(|lb_item| &lb_item.item).cloned()
    }
}

impl<E: WeightedEndpoint + EndpointWithAuthority> FromIterator<Arc<E>> for MaglevBalancer<E> {
    fn from_iter<T: IntoIterator<Item = Arc<E>>>(iter: T) -> Self {
        Self::new(iter.into_iter().map(|item| LbItem::new(item.weight(), item)))
    }
}

trait IsPrime {
    const IS_PRIME: ();
}

struct Prime<const N: usize>;

impl<const N: usize> IsPrime for Prime<N> {
    const IS_PRIME: () = assert!(is_prime(N), "Maglev lookup table size is not a prime number");
}

const fn is_prime(n: usize) -> bool {
    if n <= 1 {
        return false;
    } else if n <= 3 {
        return true;
    }

    // round up to power of 2, then take the sqrt which is simply halving the power
    // and the trailing zeros are equal to n in 2^n
    let next_square = (n as u128).next_power_of_two().trailing_zeros() + 1;
    let upper_bound = 1 << (next_square / 2);
    // if you make a sieve of multiples of 2 and multiples of 3 starting from 0 you get a pattern like
    // x_xxx_x_xxx_x_xxx_
    // so jumps by 2. jumps by 4 alternating, which is incr xor 6
    let mut incr = 2;

    if n % 2 == 0 || n % 3 == 0 {
        return false;
    }

    let mut current = 5;
    while current <= upper_bound {
        if n % current == 0 {
            return false;
        }
        current += incr;
        incr ^= 6;
    }

    true
}

#[cfg(test)]
mod test {
    use std::sync::Arc;

    use http::uri::Authority;
    use rand::{Rng, SeedableRng, rngs::SmallRng};

    use crate::clusters::balancers::{Balancer, EndpointWithAuthority};

    use super::{LbItem, MaglevBalancer};

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

    fn get_authority(port: u32) -> Authority {
        Authority::try_from(format!("example.com:{port}")).unwrap()
    }

    fn balancer_from_distribution<const TABLE_SIZE: usize>(
        distribution: &[(u32, u32)],
    ) -> MaglevBalancer<TestEndpoint> {
        let (values, weights): (Vec<_>, Vec<_>) = distribution.iter().copied().unzip();

        let items: Vec<_> =
            values.iter().map(|value| Arc::new(TestEndpoint::new(*value, get_authority(8000 + value)))).collect();

        let lb_items = items.iter().cloned().zip(weights.iter()).map(|(item, weight)| LbItem::new(*weight, item));

        MaglevBalancer::with_size::<TABLE_SIZE>(lb_items)
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
    fn maglev_balancer_empty() {
        let mut empty_balancer: MaglevBalancer<TestEndpoint> = MaglevBalancer::default();

        assert!(empty_balancer.next_item(None).is_none(), "unexpected result in empty balancer");
    }

    #[test]
    fn maglev_balancer_weight_overflow() {
        const TABLE_SIZE: usize = 257;

        // Weights of 0 + 1 fit, weight of 2 overflows
        let distribution: Vec<_> = vec![(0, u32::MAX - 2), (1, 1), (2, 2)];

        let balancer = balancer_from_distribution::<TABLE_SIZE>(&distribution);

        assert_eq!(balancer.items.len(), 2, "balancer did not detect a weight overflow");
    }

    #[test]
    fn maglev_balancer_distribution() {
        const TABLE_SIZE: usize = 257;

        let distributions = [
            (vec![(0, 1), (1, 1), (2, 1), (3, 1), (4, 1), (5, 1), (6, 1), (7, 1), (8, 1), (9, 1)], 0.05),
            (vec![(0, 1), (1, 2), (2, 3), (3, 1), (4, 2), (5, 3), (6, 1), (7, 2), (8, 3), (9, 1)], 0.05),
            (vec![(0, 1), (1, 300)], 0.2),
            (vec![(0, 300), (1, 1)], 0.2),
            (vec![(0, 1), (1, 2), (2, 3), (3, 4), (4, 5), (5, 6), (6, 7), (7, 8), (8, 9), (9, 10)], 0.1),
        ];

        for (distribution, error_margin) in distributions {
            let weights: Vec<_> = distribution.iter().map(|(_value, weight)| *weight).collect();
            let total_weight: u32 = weights.iter().sum();
            let normalized_weights: Vec<_> =
                weights.iter().map(|weight| f64::from(*weight) / f64::from(total_weight)).collect();

            let balancer = balancer_from_distribution::<TABLE_SIZE>(&distribution);

            let mut counts = vec![0_u32; distribution.len()];

            balancer.table.iter().for_each(|index| counts[balancer.items[*index].item.value as usize] += 1);
            let ring_size: u32 = counts.iter().sum();

            assert_eq!(ring_size as usize, TABLE_SIZE, "wrong table size");
            assert!(
                counts.iter().all(|count| *count > 0),
                "item is zero in {counts:?} for distribution {distribution:?}"
            );

            // Check that the ring has a distribution that fits the expected weights within margin of error
            distribution_within_margin(
                &counts.iter().map(|count| f64::from(*count) / f64::from(ring_size)).collect::<Vec<_>>(),
                &normalized_weights,
                error_margin,
            );
        }
    }

    #[test]
    fn maglev_balancer_overflow() {
        const TABLE_SIZE: usize = 47;

        // When trying to insert 100 endpoints into a 10-item ring, expect that only 10 endpoints are allocated
        let distribution: Vec<_> = (0..100).map(|value| (value, 1)).collect();

        let balancer = balancer_from_distribution::<TABLE_SIZE>(&distribution);

        let mut counts = vec![0_u32; distribution.len()];

        balancer.table.iter().for_each(|index| counts[balancer.items[*index].item.value as usize] += 1);
        let ring_size: u32 = counts.iter().sum();

        assert_eq!(ring_size as usize, TABLE_SIZE, "wrong table size");

        assert_eq!(
            counts.iter().filter(|count| **count > 0).count(),
            TABLE_SIZE,
            "item is zero in {counts:?} for distribution {distribution:?}"
        );
    }

    #[test]
    fn maglev_balancer_consistent() {
        const TABLE_SIZE: usize = 47;

        // 10 endpoints with different weights and addresses `example.com:800X`, where X is 0..10
        let distributions = [
            [(0, 1), (1, 1), (2, 1), (3, 1), (4, 1), (5, 1), (6, 1), (7, 1), (8, 1), (9, 1)],
            [(0, 1), (1, 2), (2, 3), (3, 1), (4, 2), (5, 3), (6, 1), (7, 2), (8, 3), (9, 1)],
            [(0, 1), (1, 2), (2, 3), (3, 4), (4, 5), (5, 6), (6, 7), (7, 8), (8, 9), (9, 10)],
        ];

        for distribution in distributions {
            let mut balancer = balancer_from_distribution::<TABLE_SIZE>(&distribution);

            // Test 100 requests with a random hash each one
            let mut rng = SmallRng::seed_from_u64(1);
            for _ in 0..100 {
                let hash = rng.r#gen();

                // Check that load balancing is consistent for this request
                (0..10).map(|_| balancer.next_item(Some(hash)).unwrap().value).reduce(|initial, current| {
                    assert_eq!(initial, current, "Maglev result is not consistent");
                    initial
                });
            }
        }
    }
}
