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

use std::{
    borrow::Cow,
    error::Error as ErrorTrait,
    fmt::{Debug, Display},
    ops::{Deref, DerefMut},
    result::Result as StdResult,
};
type BoxedErr = Box<dyn ErrorTrait + Send + Sync + 'static>;

// we define two types, Error and ErrorImpl here because we want our exported Error type to both
// implement the Error trait and implement From<E: ErrorTrait>.
// unfortunately, these traits can't be implemented simultaniously, as the From<E: ErrorTrait> would conflict with
// the blanket From<Self> impl.
//
// By introducing two types we can work around this. Error implements From<E: ErrorTrait> but not ErrorTrait
// and ErrorImpl implements ErrorTrait but not From<E: ErrorTrait>.
// additionally, we have Error impl DerefMut ErrorImpl, through which it will still inherit the trait methods defined on
//  ErrorImpl.
pub struct Error(ErrorImpl);
pub type Result<T> = StdResult<T, Error>;

impl Debug for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        <ErrorImpl as Debug>::fmt(&self.0, f)
    }
}

impl Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        <ErrorImpl as Display>::fmt(&self.0, f)
    }
}

impl Error {
    #[must_use]
    pub fn context<T: Into<Cow<'static, str>>>(self, msg: T) -> Self {
        Self(ErrorImpl::Msg(msg.into(), self.0.into_boxed_err()))
    }

    pub fn inner(self) -> impl ErrorTrait + Send + Sync + 'static {
        self.0
    }
}

impl AsRef<(dyn ErrorTrait + Send + Sync + 'static)> for Error {
    fn as_ref(&self) -> &(dyn ErrorTrait + Send + Sync + 'static) {
        &self.0
    }
}

impl Deref for Error {
    type Target = dyn ErrorTrait + 'static;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for Error {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

// allows the use of the try operator (`?`) on Results in functions returning `Result<T, Error>``
impl<E: Into<BoxedErr>> From<E> for Error {
    fn from(value: E) -> Self {
        Self(ErrorImpl::Error(value.into()))
    }
}

enum ErrorImpl {
    /// an error without any context attached
    Error(BoxedErr),
    // an error message with a parent context
    Msg(Cow<'static, str>, BoxedErr),
}

impl ErrorImpl {
    fn into_boxed_err(self) -> BoxedErr {
        if let Self::Error(boxed) = self {
            boxed
        } else {
            self.into()
        }
    }
}

impl ErrorTrait for ErrorImpl {
    fn source(&self) -> Option<&(dyn ErrorTrait + 'static)> {
        match self {
            Self::Error(err) => err.source(),
            Self::Msg(_, err) => Some(err.as_ref()),
        }
    }
}

impl Debug for ErrorImpl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Error(err) => <BoxedErr as Display>::fmt(err, f),
            Self::Msg(msg, _) => f.write_str(msg),
        }?;

        if let Some(first_source) = self.source() {
            let mut level = 0;
            //only print the level if there's at least 2 sources
            let print_level = first_source.source().is_some();
            f.write_str("\n\ncaused by:")?;
            let mut next_source = Some(first_source);
            while let Some(source) = next_source {
                if print_level {
                    f.write_fmt(format_args!("\n{level: >4}: {source}"))
                } else {
                    f.write_fmt(format_args!("\n    {source}"))
                }?;
                next_source = source.source();
                level += 1;
            }
        }
        Ok(())
    }
}

impl Display for ErrorImpl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Msg(msg, _) => <Cow<'static, str> as Display>::fmt(msg, f),
            Self::Error(err) => <BoxedErr as Display>::fmt(err, f),
        }
    }
}

// alows for doing error.context("some description")
pub trait Context {
    type T;
    fn context<Msg: Into<Cow<'static, str>>>(self, context: Msg) -> Result<Self::T>;
    fn with_context<F: FnOnce() -> Msg, Msg: Into<Cow<'static, str>>>(self, context_fn: F) -> Result<Self::T>;
}

impl<T, E: ErrorTrait + Send + Sync + 'static> Context for StdResult<T, E> {
    type T = T;
    fn context<Msg: Into<Cow<'static, str>>>(self, context: Msg) -> Result<Self::T> {
        self.map_err(|e| Error::from(e).context(context))
    }
    fn with_context<F: FnOnce() -> Msg, Msg: Into<Cow<'static, str>>>(self, context_fn: F) -> Result<Self::T> {
        self.map_err(|e| Error::from(e).context(context_fn()))
    }
}

// Error does not implement the ErrorTrait, so the previous impl does not apply to it
impl<T> Context for Result<T> {
    type T = T;
    fn context<Msg: Into<Cow<'static, str>>>(self, context: Msg) -> Result<Self::T> {
        self.map_err(|e| e.context(context))
    }
    fn with_context<F: FnOnce() -> Msg, Msg: Into<Cow<'static, str>>>(self, context_fn: F) -> Result<Self::T> {
        self.map_err(|e| e.context(context_fn()))
    }
}

impl<T> Context for Option<T> {
    type T = T;
    fn context<C: Into<Cow<'static, str>>>(self, msg: C) -> Result<Self::T> {
        self.ok_or_else(|| Error::from(msg.into()))
    }
    fn with_context<F: FnOnce() -> C, C: Into<Cow<'static, str>>>(self, context_fn: F) -> Result<Self::T> {
        self.ok_or_else(|| Error::from(context_fn().into()))
    }
}
