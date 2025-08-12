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

use parking_lot::RwLock;
use std::sync::atomic::{AtomicUsize, Ordering};

pub struct CachedWatch<T: Clone> {
    value: RwLock<T>,
    version: AtomicUsize,
}

impl<T: Clone> CachedWatch<T> {
    pub const fn new(value: T) -> Self {
        Self { value: RwLock::new(value), version: AtomicUsize::new(0) }
    }

    pub fn version(&self) -> usize {
        self.version.load(Ordering::Relaxed)
    }

    #[allow(unused)]
    pub fn set(&self, value: T) {
        self.update(move |current| *current = value)
    }

    pub fn update<R, F: FnOnce(&mut T) -> R>(&self, f: F) -> R {
        let mut w_lock = self.value.write();
        let ret = f(&mut w_lock);
        self.version.fetch_add(1, Ordering::Relaxed);
        ret
    }

    pub fn get_clone(&self) -> (T, usize) {
        let r_lock = self.value.read();
        let value = r_lock.clone();
        let version = self.version();
        (value, version)
    }

    pub fn watcher(&self) -> CachedWatcher<'_, T> {
        let (local, version) = self.get_clone();
        CachedWatcher { parent: self, version, local }
    }
}

pub struct CachedWatcher<'a, T: Clone> {
    parent: &'a CachedWatch<T>,
    version: usize,
    local: T,
}

impl<'a, T: Clone> CachedWatcher<'a, T> {
    pub fn cached_or_latest(&mut self) -> &mut T {
        let parent_version = self.parent.version();
        if parent_version != self.version {
            // this read lock might fail if the parent is being updated again
            //  in which case we continue using the old version under the assumption that
            // updates will be infrequent w.r.t. reads.
            // it's possible to add some logic here to stall and force an update if we fail to update too many times
            // but be mindfull that some implementations of RwLock always let writes go first so we can still get starved of updates
            // unless we also tell the parent to wait and let us read first before allowing another write lock to be acquired.
            if let Some(r_lock) = self.parent.value.try_read() {
                self.local = r_lock.clone();
                self.version = self.parent.version();
            }
        }
        &mut self.local
    }
}
