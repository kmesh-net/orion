// SPDX-FileCopyrightText: © 2025 Huawei Cloud Computing Technologies Co., Ltd
// SPDX-License-Identifier: Apache-2.0
//
// Copyright 2025 Huawei Cloud Computing Technologies Co., Ltd
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
    collections::HashMap,
    hash::{BuildHasherDefault, Hash},
    sync::atomic::{AtomicU64, Ordering},
};

use ahash::AHasher;
use dashmap::DashMap;
use opentelemetry::KeyValue;
use std::{collections::hash_map, fmt};

pub struct ShardedU64<S> {
    data: DashMap<S, HashMap<Vec<KeyValue>, AtomicU64>, BuildHasherDefault<AHasher>>,
}

impl<S: Eq + Hash> ShardedU64<S> {
    pub fn new() -> Self {
        ShardedU64 { data: DashMap::default() }
    }

    pub fn add(&self, value: u64, shard_id: S, key: &[KeyValue]) {
        let mut shard = self.data.entry(shard_id).or_default();
        if let Some(counter) = shard.get(key) {
            counter.fetch_add(value, Ordering::Relaxed);
        } else {
            shard.entry(key.to_vec()).or_insert(AtomicU64::new(value));
        }
    }
    pub fn sub(&self, value: u64, shard_id: S, key: &[KeyValue]) {
        if let Some(shard) = self.data.get_mut(&shard_id) {
            if let Some(counter) = shard.get(key) {
                let current_value = counter.load(Ordering::Relaxed);
                let new_value = current_value.saturating_sub(value);
                counter.store(new_value, Ordering::Relaxed);
            }
        }
    }

    pub fn load_all(&self) -> HashMap<Vec<KeyValue>, u64> {
        let mut result = HashMap::new();
        for shard in self.data.iter() {
            for (key, counter) in shard.value().iter() {
                let value = counter.load(Ordering::Relaxed);
                *result.entry(key.clone()).or_insert(0) += value;
            }
        }
        result
    }

    pub fn load(&self, key: &[KeyValue]) -> Option<u64> {
        let mut total = None;
        for shard in self.data.iter() {
            if let Some(counter) = shard.value().get(key) {
                let value = counter.load(Ordering::Relaxed);
                total = Some(total.unwrap_or(0) + value);
            }
        }
        total
    }

    pub fn store(&self, value: u64, shard_id: S, key: &[KeyValue]) {
        let mut shard = self.data.entry(shard_id).or_default();
        if let Some(counter) = shard.get(key) {
            counter.store(value, Ordering::Relaxed);
        } else {
            shard.insert(key.to_vec(), AtomicU64::new(value));
        }
    }

    pub fn shard_count(&self) -> usize {
        self.data.len()
    }

    pub fn clear(&self) {
        self.data.clear();
    }

    pub fn remove(&self, shard_id: S, key: &[KeyValue]) -> Option<u64> {
        self.data.entry(shard_id).or_default().remove(key).map(|counter| counter.into_inner())
    }
}

pub struct ShardedU64IntoIter {
    inner: hash_map::IntoIter<Vec<KeyValue>, u64>,
}

impl Iterator for ShardedU64IntoIter {
    type Item = (Vec<KeyValue>, u64);
    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next()
    }
}

impl<S: Eq + Hash> IntoIterator for &'_ ShardedU64<S> {
    type Item = (Vec<KeyValue>, u64);
    type IntoIter = ShardedU64IntoIter;

    fn into_iter(self) -> Self::IntoIter {
        let snapshot = self.load_all();
        ShardedU64IntoIter { inner: snapshot.into_iter() }
    }
}

impl<S: Eq + Hash> Default for ShardedU64<S> {
    fn default() -> Self {
        Self::new()
    }
}

impl<S: Eq + Hash> fmt::Debug for ShardedU64<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map().entries(self.load_all()).finish()
    }
}

#[cfg(test)]
mod tests {
    use std::{num::NonZeroU64, sync::Arc, thread::ThreadId};

    use super::*;

    fn build_thread_id(n: u64) -> ThreadId {
        let nz = NonZeroU64::new(n).expect("NonZeroU64 should not be zero");
        unsafe { std::mem::transmute(nz) }
    }

    #[test]
    fn test_basic_add_and_load() {
        let s = ShardedU64::new();
        let shard_1 = build_thread_id(1);
        let shard_2 = build_thread_id(2);
        let key = [KeyValue::new("metric", "requests")];

        assert_eq!(s.load(&key), None);

        s.add(10, shard_1, &key);
        assert_eq!(s.load(&key), Some(10));

        s.add(5, shard_2, &key);
        assert_eq!(s.load(&key), Some(15));
    }

    #[test]
    fn test_saturating_sub() {
        let s = ShardedU64::new();
        let shard_1 = build_thread_id(1);
        let key = vec![KeyValue::new("metric", "inventory")];

        s.sub(10, shard_1, &key);
        assert_eq!(s.load(&key), None);

        s.add(20, shard_1, &key);
        assert_eq!(s.load(&key), Some(20));

        s.sub(5, shard_1, &key);
        assert_eq!(s.load(&key), Some(15));

        s.sub(25, shard_1, &key);
        assert_eq!(s.load(&key), Some(0));

        s.sub(100, shard_1, &key);
        assert_eq!(s.load(&key), Some(0));
    }

    #[test]
    fn test_debug() {
        let s = ShardedU64::new();
        let shard_1 = build_thread_id(1);
        let key = vec![KeyValue::new("metric", "value")];

        s.add(100, shard_1, &key);
        assert_eq!(s.load(&key), Some(100));

        s.store(42, shard_1, &key);
        assert_eq!(s.load(&key), Some(42));

        s.store(0, shard_1, &[]);
        println!("{s:?}");
    }

    #[test]
    fn test_store_overwrites_value() {
        let s = ShardedU64::new();
        let shard_1 = build_thread_id(1);
        let key = vec![KeyValue::new("metric", "value")];

        s.add(100, shard_1, &key);
        assert_eq!(s.load(&key), Some(100));

        s.store(42, shard_1, &key);
        assert_eq!(s.load(&key), Some(42));
    }

    #[test]
    fn test_multi_shard_aggregation() {
        let s = ShardedU64::new();
        let (tid1, tid2, tid3) = (build_thread_id(1), build_thread_id(2), build_thread_id(3));

        let key_a = vec![KeyValue::new("metric", "a")];
        let key_b = vec![KeyValue::new("metric", "b")];

        s.add(10, tid1, &key_a); // a = 10
        s.add(20, tid2, &key_a); // a = 10 (tid1) + 20 (tid2) = 30
        s.sub(5, tid1, &key_a); // a = 5 (tid1) + 20 (tid2) = 25

        s.add(100, tid3, &key_b);

        assert_eq!(s.load(&key_a), Some(25));
        assert_eq!(s.load(&key_b), Some(100));

        let all_data = s.load_all();
        assert_eq!(all_data.get(&key_a), Some(&25));
        assert_eq!(all_data.get(&key_b), Some(&100));
        assert_eq!(all_data.len(), 2);
    }

    #[test]
    fn test_clear_and_shard_count() {
        let s = ShardedU64::new();
        let (tid1, tid2) = (build_thread_id(1), build_thread_id(2));
        let key = vec![KeyValue::new("metric", "requests")];

        assert_eq!(s.shard_count(), 0);

        s.add(10, tid1, &key);
        assert_eq!(s.shard_count(), 1);

        s.add(5, tid2, &key);
        assert_eq!(s.shard_count(), 2);

        s.add(1, tid1, &key);
        assert_eq!(s.shard_count(), 2);

        s.clear();
        assert_eq!(s.shard_count(), 0);
        assert_eq!(s.load(&key), None);
    }

    #[test]
    fn test_actual_concurrency() {
        let s = Arc::new(ShardedU64::<ThreadId>::new());
        let mut handles = vec![];
        let num_threads = 10;
        let increments_per_thread = 1000;
        let key = vec![KeyValue::new("metric", "concurrent_counter")];

        for _ in 0..num_threads {
            let s_clone = Arc::clone(&s);
            let key_clone = key.clone();
            handles.push(std::thread::spawn(move || {
                let tid = std::thread::current().id();
                for _ in 0..increments_per_thread {
                    s_clone.add(1, tid, &key_clone);
                }
            }));
        }

        for handle in handles {
            handle.join().unwrap();
        }

        let expected_value = (num_threads * increments_per_thread) as u64;
        assert_eq!(s.load(&key), Some(expected_value));
    }
}
