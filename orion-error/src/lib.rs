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

use std::{
    any::Any,
    borrow::Cow,
    error::Error as ErrorTrait,
    fmt::{Debug, Display},
    io,
    ops::{Deref, DerefMut},
    result::Result as StdResult,
};
type BoxedErr = Box<dyn ErrorTrait + Send + Sync + 'static>;

// we define two types, Error and ErrorImpl here because we want our exported Error type to both
// implement the Error trait and implement From<E: ErrorTrait>.
// unfortunately, these traits can't be implemented simultaneously, as the From<E: ErrorTrait> would conflict with
// the blanket From<Self> impl.
//
// By introducing two types we can work around this. Error implements From<E: ErrorTrait> but not ErrorTrait
// and ErrorImpl implements ErrorTrait but not From<E: ErrorTrait>.
// additionally, we have Error impl DerefMut ErrorImpl, through which it will still inherit the trait methods defined on
//  ErrorImpl.

pub struct Error(ErrorImpl);
pub type Result<T> = StdResult<T, Error>;

// A trait that extends any error type or Result to provide additional context functionality.
pub trait Context: Sized {
    type Target;

    #[must_use]
    fn with_context(self, ctx: ErrorInfo) -> Self::Target;

    #[must_use]
    fn with_context_fn<F: FnOnce() -> ErrorInfo>(self, context_fn: F) -> Self::Target {
        self.with_context(context_fn())
    }

    #[must_use]
    fn with_context_msg<S: Into<Cow<'static, str>>>(self, msg: S) -> Self::Target {
        let ctx = ErrorInfo { message: Some(msg.into()), any: None };
        self.with_context(ctx)
    }

    #[must_use]
    fn with_context_data<T: Any + Send + Sync + 'static>(self, data: T) -> Self::Target {
        let ctx = ErrorInfo { message: None, any: Some(Box::new(data)) };
        self.with_context(ctx)
    }
}

// A wrapper for a concrete error that carries additional context information (ErrorInfo)
// This is useful for attaching additional information to an error generated with thiserror library.
pub struct WithContext<E: ErrorTrait + Send + Sync + 'static> {
    inner: E,
    context: ErrorInfo,
}

impl<E: ErrorTrait + Send + Sync + 'static> WithContext<E> {
    pub fn new(value: E) -> Self {
        Self { inner: value, context: ErrorInfo::default() }
    }

    pub fn into_inner(self) -> (E, ErrorInfo) {
        (self.inner, self.context)
    }

    pub fn get_context_data<T: 'static>(&self) -> Option<&T> {
        if let ErrorInfo { message: _, any: Some(val) } = &self.context {
            val.downcast_ref::<T>()
        } else {
            None
        }
    }

    pub fn map_into<E2>(self) -> WithContext<E2>
    where
        E2: ErrorTrait + Send + Sync + 'static,
        E: Into<E2>,
    {
        WithContext { inner: self.inner.into(), context: self.context }
    }
}

impl<E: ErrorTrait + Send + Sync + 'static> Context for WithContext<E> {
    type Target = Self;
    fn with_context(self, ctx: ErrorInfo) -> Self::Target {
        Self { inner: self.inner, context: self.context.with_context(ctx) }
    }
}

impl<E: ErrorTrait + Send + Sync + 'static> AsRef<E> for WithContext<E> {
    fn as_ref(&self) -> &E {
        &self.inner
    }
}

impl<E: ErrorTrait + Send + Sync + 'static> Debug for WithContext<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(msg) = self.context.message.as_ref() {
            f.write_str(msg)?
        } else {
            <E as Display>::fmt(&self.inner, f)?
        }

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

impl<E: ErrorTrait + Send + Sync + 'static> Display for WithContext<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(msg) = self.context.message.as_ref() {
            f.write_str(msg)
        } else {
            <E as Display>::fmt(&self.inner, f)
        }
    }
}

impl<E: ErrorTrait + Send + Sync + 'static> ErrorTrait for WithContext<E> {
    fn source(&self) -> Option<&(dyn ErrorTrait + 'static)> {
        self.inner.source()
    }
}

/// A context for an error, which can contain a message and any additional data.
#[derive(Debug, Default)]
pub struct ErrorInfo {
    message: Option<Cow<'static, str>>,
    any: Option<Box<dyn Any + Send + Sync + 'static>>,
}

impl ErrorInfo {
    #[must_use]
    pub fn with_message<T: Into<Cow<'static, str>>>(mut self, msg: T) -> Self {
        self.message = Some(msg.into());
        self
    }

    #[must_use]
    pub fn with_data<T: Any + Send + Sync + 'static>(mut self, data: T) -> Self {
        self.any = Some(Box::new(data));
        self
    }

    #[must_use]
    pub fn with_context(self, ec: ErrorInfo) -> Self {
        Self { message: ec.message.or(self.message), any: ec.any.or(self.any) }
    }
}

/// A custom error type that can be used to represent errors in the application.
/// It implements the type erasure pattern, allowing it to hold any error type that implements the `ErrorTrait`.
impl Error {
    pub fn new<T: Into<Cow<'static, str>>>(msg: T) -> Self {
        let err: BoxedErr = Box::new(std::io::Error::other(msg.into()));
        Self(ErrorImpl::Error(err))
    }

    pub fn into_inner(self) -> impl ErrorTrait + Send + Sync + 'static {
        self.0
    }

    pub fn inner(&self) -> &(impl ErrorTrait + Send + Sync + 'static) {
        &self.0
    }

    pub fn get_context_data<T: 'static>(&self) -> Option<&T> {
        if let ErrorImpl::Context(ErrorInfo { message: _, any: Some(val) }, _) = &self.0 {
            val.downcast_ref::<T>()
        } else {
            None
        }
    }
}

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

impl Context for Error {
    type Target = Self;
    fn with_context(self, ctx: ErrorInfo) -> Self::Target {
        match self.0 {
            ErrorImpl::Error(err) => Self(ErrorImpl::Context(ctx, err)),
            ErrorImpl::Context(prev, err) => Self(ErrorImpl::Context(prev.with_context(ctx), err)),
        }
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

/// Allows to convert an `ErrorCtx` into an Error, using a generic inner error.
impl From<ErrorInfo> for Error {
    fn from(ctx: ErrorInfo) -> Self {
        let ErrorInfo { message, any } = &ctx;
        match (message, any) {
            (Some(msg), _) => {
                let msg = msg.to_string();
                Error(ErrorImpl::Context(ctx, Box::new(io::Error::other(msg))))
            },
            (None, _) => Error(ErrorImpl::Context(ctx, Box::new(std::io::Error::other("an error occurred")))),
        }
    }
}

/// Actual implementation of the Error.
enum ErrorImpl {
    /// an error without any context attached
    Error(BoxedErr),
    // an error message with a parent context
    Context(ErrorInfo, BoxedErr),
}

impl ErrorTrait for ErrorImpl {
    fn source(&self) -> Option<&(dyn ErrorTrait + 'static)> {
        match self {
            Self::Error(err) | Self::Context(_, err) => Some(err.as_ref()),
        }
    }
}

impl Debug for ErrorImpl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Error(err) => <BoxedErr as Display>::fmt(err, f),
            Self::Context(ctx, err) => {
                if let Some(msg) = ctx.message.as_ref() {
                    f.write_str(msg)
                } else {
                    <BoxedErr as Display>::fmt(err, f)
                }
            },
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
            Self::Error(err) => <BoxedErr as Display>::fmt(err, f),
            Self::Context(ctx, err) => {
                if let Some(msg) = ctx.message.as_ref() {
                    f.write_str(msg)
                } else {
                    <BoxedErr as Display>::fmt(err, f)
                }
            },
        }
    }
}

impl<T, E: ErrorTrait + Send + Sync + 'static> Context for StdResult<T, E> {
    type Target = Result<T>;
    fn with_context(self, ctx: ErrorInfo) -> Self::Target {
        self.map_err(|e| Error::from(e).with_context(ctx))
    }
}

// Error does not implement the ErrorTrait, so the previous impl does not apply to it
impl<T> Context for Result<T> {
    type Target = Self;
    fn with_context(self, ctx: ErrorInfo) -> Self::Target {
        self.map_err(|e| e.with_context(ctx))
    }
}

impl<T> Context for Option<T> {
    type Target = Result<T>;
    fn with_context(self, ctx: ErrorInfo) -> Self::Target {
        self.ok_or_else(|| Error::from(ctx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone)]
    #[allow(dead_code)]
    struct MyContext {
        value: u32,
    }

    #[test]
    fn test_error_display() {
        let err = Error::new("an error occurred");
        assert_eq!(err.to_string(), "an error occurred");
    }

    #[test]
    fn test_error_context() {
        let err = Error::new("error").with_context_msg("context error");
        assert!(err.source().is_some());
        assert_eq!(err.to_string(), "context error");
    }

    #[test]
    fn test_any_context() {
        let my_context = MyContext { value: 42 };
        let err = Error::new("an error occurred").with_context_data(my_context);
        assert!(err.source().is_some());
        assert_eq!(err.to_string(), "an error occurred");

        let ctx = err.get_context_data::<MyContext>().expect("Expected context to be present");
        assert_eq!(ctx.value, 42);
    }

    #[test]
    fn test_any_context_fn() {
        let err =
            Error::new("an error occurred").with_context_fn(|| ErrorInfo::default().with_data(MyContext { value: 42 }));
        assert!(err.source().is_some());
        assert_eq!(err.to_string(), "an error occurred");

        let ctx = err.get_context_data::<MyContext>().expect("Expected context to be present");
        assert_eq!(ctx.value, 42);
    }

    #[test]
    fn test_response_with_context() {
        let res: Result<()> = Err(Error::new("generic error"));
        let res = res.with_context_data(MyContext { value: 42 });
        let res = res.with_context_msg("an error occurred");

        if let Err(err) = res {
            assert!(err.source().is_some());
            assert_eq!(err.to_string(), "an error occurred");
            let ctx = err.get_context_data::<MyContext>().expect("Expected context to be present");
            assert_eq!(ctx.value, 42);
        }
    }

    #[test]
    fn test_response_with_context_fn() {
        let res: Result<()> = Err(Error::new("generic error"));
        let res = res.with_context_fn(|| ErrorInfo::default().with_data(MyContext { value: 42 }));
        let res = res.with_context_fn(|| ErrorInfo::default().with_message("an error occurred"));

        if let Err(err) = res {
            assert!(err.source().is_some());
            assert_eq!(err.to_string(), "an error occurred");
            let ctx = err.get_context_data::<MyContext>().expect("Expected context to be present");
            assert_eq!(ctx.value, 42);
        }
    }

    #[derive(Debug)]
    struct ReadConfigError {
        path: String,
    }

    impl Display for ReadConfigError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "unable to read configuration at {}", self.path)
        }
    }

    impl ErrorTrait for ReadConfigError {}

    #[test]
    fn test_concrete_error_with_context() {
        let my_context = MyContext { value: 42 };
        let err = WithContext::new(ReadConfigError { path: "/etc/config.toml".to_owned() });
        let err = err.with_context_data(my_context);
        let ctx = err.get_context_data::<MyContext>().expect("Expected context to be present");
        assert_eq!(ctx.value, 42);
    }
}
