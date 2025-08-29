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

use std::cell::OnceCell;

pub struct DeferredInit<T> {
    cell: OnceCell<T>,
    init_fn: Option<Box<dyn FnOnce() -> T + Send + 'static>>,
}

impl<T> DeferredInit<T> {
    pub fn new<F>(f: F) -> Self
    where
        F: 'static + Send + FnOnce() -> T,
    {
        Self { cell: OnceCell::new(), init_fn: Some(Box::new(f)) }
    }

    pub fn get_mut(&mut self) -> &mut T {
        if self.cell.get().is_none() {
            if let Some(init) = self.init_fn.take() {
                _ = self.cell.set(init());
            }
        }

        match self.cell.get_mut() {
            Some(value) => value,
            None => {
                unreachable!("OnceCell should always contain a value after initialization");
            },
        }
    }
}
