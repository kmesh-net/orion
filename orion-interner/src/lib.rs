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

use std::sync::OnceLock;

use compact_str::CompactString;
use http::Version;
use lasso::ThreadedRodeo;

static GLOBAL_INTERNER: OnceLock<ThreadedRodeo> = OnceLock::new();

pub trait StringInterner {
    fn to_static_str(&self) -> &'static str;
}

#[inline]
fn intern_str(s: &str) -> &'static str {
    let interner = GLOBAL_INTERNER.get_or_init(ThreadedRodeo::new);
    let key = interner.get_or_intern(s);
    let static_ref = interner.resolve(&key);

    // SAFETY: The `GLOBAL_INTERNER` is a `static` variable, meaning it has a `'static`
    // lifetime and is never dropped. Therefore, the string slices stored within it
    // are also valid for the `'static` lifetime. This transmute is safe because
    // we are extending a lifetime that is already effectively `'static`.
    unsafe { std::mem::transmute::<&str, &'static str>(static_ref) }
}

impl StringInterner for &str {
    fn to_static_str(&self) -> &'static str {
        intern_str(self)
    }
}

impl StringInterner for String {
    fn to_static_str(&self) -> &'static str {
        intern_str(self)
    }
}

impl StringInterner for CompactString {
    fn to_static_str(&self) -> &'static str {
        intern_str(self)
    }
}

impl StringInterner for Version {
    fn to_static_str(&self) -> &'static str {
        match *self {
            Version::HTTP_09 => "HTTP/0.9",
            Version::HTTP_10 => "HTTP/1.0",
            Version::HTTP_11 => "HTTP/1.1",
            Version::HTTP_2 => "HTTP/2",
            Version::HTTP_3 => "HTTP/3",
            _ => "HTTP/unknown",
        }
    }
}
