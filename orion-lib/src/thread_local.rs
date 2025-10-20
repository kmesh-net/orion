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

use thread_local::ThreadLocal;

pub trait LocalBuilder<A, T> {
    fn build(&self, arg: A) -> T;
}

/// Provides a thread-local instance of an object. When a thread requests the local copy
/// through [`LocalObject::get()`], the first time constructs it using the builder
/// and arguments provided in [`LocalObject::new()`]. In subsequent requests a reference
/// to this thread-local object is provided.
#[derive(Debug)]
pub struct LocalObject<T, B, A>
where
    T: Sync + Send,
    B: LocalBuilder<A, T>,
    A: Clone,
{
    tls: ThreadLocal<T>,
    builder: B,
    arg: A,
}

impl<T, A, B> LocalObject<T, B, A>
where
    T: Sync + Send,
    B: LocalBuilder<A, T>,
    A: Clone,
{
    pub fn new(builder: B, arg: A) -> Self {
        let tls = ThreadLocal::new();
        LocalObject { tls, builder, arg }
    }

    pub fn get_or_build(&self) -> &T {
        self.tls.get_or(|| self.builder.build(self.arg.clone()))
    }
}
