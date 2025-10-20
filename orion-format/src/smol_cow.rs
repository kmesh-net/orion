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

use core::fmt;
use smol_str::SmolStr;
use std::borrow::Borrow;
use std::ops::Deref;

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SmolCow<'a> {
    Borrowed(&'a str),
    Owned(SmolStr),
}

impl<'a> SmolCow<'a> {
    #[inline]
    pub fn into_owned(self) -> SmolStr {
        match self {
            SmolCow::Borrowed(s) => SmolStr::new(s),
            SmolCow::Owned(s) => s,
        }
    }

    #[inline]
    pub const fn is_borrowed(&self) -> bool {
        match *self {
            SmolCow::Borrowed(_) => true,
            SmolCow::Owned(_) => false,
        }
    }

    #[inline]
    pub const fn is_owned(&self) -> bool {
        !self.is_borrowed()
    }

    pub fn to_mut(&mut self) -> &mut SmolStr {
        match *self {
            SmolCow::Borrowed(borrowed) => {
                *self = SmolCow::Owned(SmolStr::new(borrowed));
                match *self {
                    SmolCow::Borrowed(..) => unreachable!(),
                    SmolCow::Owned(ref mut owned) => owned,
                }
            },
            SmolCow::Owned(ref mut owned) => owned,
        }
    }
}

impl fmt::Display for SmolCow<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            SmolCow::Borrowed(ref b) => fmt::Display::fmt(b, f),
            SmolCow::Owned(ref o) => fmt::Display::fmt(o, f),
        }
    }
}

impl<'a> Deref for SmolCow<'a> {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        match self {
            SmolCow::Borrowed(s) => s,
            SmolCow::Owned(s) => s.deref(),
        }
    }
}

impl<'a> AsRef<str> for SmolCow<'a> {
    fn as_ref(&self) -> &str {
        self.deref()
    }
}

impl<'a> Borrow<str> for SmolCow<'a> {
    fn borrow(&self) -> &str {
        self.deref()
    }
}

impl<'a> From<&'a str> for SmolCow<'a> {
    fn from(s: &'a str) -> Self {
        SmolCow::Borrowed(s)
    }
}

impl<'a> From<&'a String> for SmolCow<'a> {
    fn from(s: &'a String) -> Self {
        SmolCow::Owned(SmolStr::new(s))
    }
}

impl From<SmolStr> for SmolCow<'static> {
    fn from(s: SmolStr) -> Self {
        SmolCow::Owned(s)
    }
}

impl From<String> for SmolCow<'static> {
    fn from(s: String) -> Self {
        SmolCow::Owned(s.into())
    }
}
