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

use std::hash::{Hash, Hasher};

use ahash::AHasher;
use tokio::sync::mpsc::Sender;

pub(crate) struct LoggerPool<T>(pub Vec<Sender<T>>);

impl<T> LoggerPool<T> {
    #[inline]
    pub(crate) fn get(&self) -> Option<&Sender<T>> {
        match self.0.len() {
            0 => None,
            1 => unsafe { Some(self.0.get_unchecked(0)) },
            n => {
                let idx = Self::hash_thread_id(std::thread::current().id()) % n as u64;
                #[allow(clippy::cast_possible_truncation)]
                unsafe {
                    Some(self.0.get_unchecked(idx as usize))
                }
            },
        }
    }

    #[inline]
    pub(crate) fn get_at(&self, index: usize) -> Option<&Sender<T>> {
        self.0.get(index)
    }

    #[inline]
    pub(crate) fn hash_thread_id(thread_id: std::thread::ThreadId) -> u64 {
        let mut hasher = AHasher::default();
        thread_id.hash(&mut hasher);
        hasher.finish()
    }

    #[inline]
    #[allow(dead_code)]
    pub(crate) fn len(&self) -> usize {
        self.0.len()
    }

    #[inline]
    #[allow(dead_code)]
    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}
