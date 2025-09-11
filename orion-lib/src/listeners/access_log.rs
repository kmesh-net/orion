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

use orion_format::{context::Context, LogFormatterLocal};

pub trait AccessLogContext {
    type Type;
    fn with_context<C: Context>(&mut self, ctx: &C);

    fn with_context_fn<F, Ctx>(&mut self, f: F)
    where
        F: FnOnce() -> Ctx,
        Ctx: Context;
}

impl AccessLogContext for Vec<LogFormatterLocal> {
    type Type = Self;

    /// Applies the given context to each `LogFormatter` in the vector.
    #[inline]
    fn with_context<C: Context>(&mut self, ctx: &C) {
        for f in self {
            f.with_context(ctx);
        }
    }

    /// Build the context and applies it to each `LogFormatter` in the vector.
    #[inline]
    fn with_context_fn<F, Ctx>(&mut self, f: F)
    where
        F: FnOnce() -> Ctx,
        Ctx: Context,
    {
        if !self.is_empty() {
            let context = f();
            for f in self {
                f.with_context(&context);
            }
        }
    }
}
