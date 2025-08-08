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

use crate::HttpBody;
use bytes::Bytes;
use http::uri::{InvalidUri, InvalidUriParts};
use http::{HeaderValue, Version as HttpVersion};
use http::{Response, StatusCode};
use http_body_util::Full;

#[derive(Debug, thiserror::Error)]
// TODO: Error type for synthetic response validation - implement validation logic or remove if unused
pub enum InvalidSyntheticResponse {
    #[error(transparent)]
    InvalidHttpResponse(#[from] http::Error),
    #[error(transparent)]
    InvalidHeaderValue(#[from] http::header::InvalidHeaderValue),
    #[error(transparent)]
    InvalidUri(#[from] InvalidUri),
    #[error(transparent)]
    InvalidUriParts(#[from] InvalidUriParts),
}

#[derive(Clone, Debug)]
pub struct SyntheticHttpResponse {
    http_status: StatusCode,
    body: Bytes,
    close_connection: bool,
}

// === impl SyntheticHttpResponse ===

impl SyntheticHttpResponse {
    pub fn internal_error() -> Self {
        Self { http_status: StatusCode::INTERNAL_SERVER_ERROR, body: Bytes::default(), close_connection: true }
    }

    pub fn bad_gateway() -> Self {
        Self { http_status: StatusCode::BAD_GATEWAY, body: Bytes::default(), close_connection: true }
    }

    // TODO: Implement forbidden response functionality - used for access control features
    pub fn forbidden(msg: &str) -> Self {
        Self {
            http_status: StatusCode::FORBIDDEN,
            body: Bytes::copy_from_slice(msg.as_bytes()),
            //should this close actually? the connection seems to stay open since it's only triggered for a single http
            close_connection: true,
        }
    }

    // TODO: Implement unavailable response - used for service health checks
    pub fn unavailable() -> Self {
        Self { http_status: StatusCode::SERVICE_UNAVAILABLE, body: Bytes::default(), close_connection: true }
    }

    pub fn gateway_timeout() -> Self {
        Self { http_status: StatusCode::GATEWAY_TIMEOUT, body: Bytes::default(), close_connection: true }
    }

    pub fn not_found() -> Self {
        Self { http_status: StatusCode::NOT_FOUND, body: Bytes::default(), close_connection: false }
    }

    // TODO: Implement custom error responses - used for flexible error handling
    pub fn custom_error(http_status: StatusCode) -> Self {
        Self { http_status, body: Bytes::default(), close_connection: false }
    }

    // #[inline]
    // fn header_error_message(&self) -> Option<HeaderValue> {
    //     match self.header_error_message {
    //         Some(Cow::Borrowed(msg)) => Some(HeaderValue::from_static(msg)),
    //         Some(Cow::Owned(ref msg)) => {
    //             Some(HeaderValue::from_str(msg).unwrap_or_else(|_| HeaderValue::from_static("unexpected error")))
    //         },
    //         None => None,
    //     }
    // }

    #[inline]
    pub fn into_response(self, version: http::Version) -> Response<HttpBody> {
        let mut rsp = Response::new(Full::from(self.body).into());
        *rsp.status_mut() = self.http_status;
        *rsp.version_mut() = version;
        if self.close_connection && (version == HttpVersion::HTTP_10 || version == HttpVersion::HTTP_11) {
            // Notify the (proxy or non-proxy) client that the connection will be closed.
            rsp.headers_mut().insert(http::header::CONNECTION, HeaderValue::from_static("close"));
        }
        rsp
    }
}

#[cfg(test)]
mod tests {
    // use http::{uri::Scheme, Uri};
    // use orion_configuration::config::network_filters::http_connection_manager::route::RedirectResponseCode;

    // use super::*;

    // #[test]
    // fn test_basic_redirect() -> Result<(), InvalidSyntheticResponse> {
    //     let uri = "http://www.test.com/foo/bar?baz".parse::<Uri>().unwrap();

    //     let ra = RedirectAction {
    //         response_code: RedirectResponseCode::TemporaryRedirect,
    //         authority_redirect: Some(AuthorityRedirect::AuthorityRedirect(Authority::from_static(
    //             "www.redirected.com:81",
    //         ))),
    //         strip_query: false,
    //         scheme_rewrite_specifier: None,
    //         path_rewrite_specifier: None,
    //     };

    //     let res = SyntheticHttpResponse::redirect(StatusCode::TEMPORARY_REDIRECT, ra).into_response(
    //         http::Version::HTTP_11,
    //         Some(uri),
    //         None,
    //     )?;

    //     let expected = &HeaderValue::from_str("http://www.redirected.com:81/foo/bar?baz")?;

    //     assert_eq!(res.headers().get(http::header::LOCATION), Some(expected));

    //     Ok(())
    // }

    // #[test]
    // fn test_redirect_strip_query() -> Result<(), InvalidSyntheticResponse> {
    //     let uri = "http://www.test.com/foo/bar?baz".parse::<Uri>().unwrap();

    //     let ra = RedirectAction {
    //         response_code: RedirectResponseCode::TemporaryRedirect,
    //         authority_redirect: Some(AuthorityRedirect::AuthorityRedirect(Authority::from_static(
    //             "www.redirected.com:81",
    //         ))),
    //         strip_query: true,
    //         scheme_rewrite_specifier: None,
    //         path_rewrite_specifier: None,
    //     };

    //     let res = SyntheticHttpResponse::redirect(StatusCode::TEMPORARY_REDIRECT, ra).into_response(
    //         http::Version::HTTP_11,
    //         Some(uri),
    //         None,
    //     )?;

    //     let expected = &HeaderValue::from_str("http://www.redirected.com:81/foo/bar")?;

    //     assert_eq!(res.headers().get(http::header::LOCATION), Some(expected));

    //     Ok(())
    // }

    // #[test]
    // fn test_http2_redirect_1() -> Result<(), InvalidSyntheticResponse> {
    //     let uri = "http://www.test.com:80/foo/bar?baz".parse::<Uri>().unwrap();

    //     let ra = RedirectAction {
    //         response_code: RedirectResponseCode::TemporaryRedirect,
    //         authority_redirect: Some(AuthorityRedirect::HostRedirect(Authority::from_static("www.redirected.com"))),
    //         strip_query: true,
    //         scheme_rewrite_specifier: Some(Scheme::HTTPS),
    //         path_rewrite_specifier: None,
    //     };

    //     let res = SyntheticHttpResponse::redirect(StatusCode::TEMPORARY_REDIRECT, ra).into_response(
    //         http::Version::HTTP_11,
    //         Some(uri),
    //         None,
    //     )?;

    //     let expected = &HeaderValue::from_str("https://www.redirected.com/foo/bar")?;

    //     assert_eq!(res.headers().get(http::header::LOCATION), Some(expected));

    //     Ok(())
    // }

    // #[test]
    // fn test_https_redirect_2() -> Result<(), InvalidSyntheticResponse> {
    //     let uri = "http://www.test.com:80/foo/bar?baz".parse::<Uri>().unwrap();

    //     let ra = RedirectAction {
    //         response_code: RedirectResponseCode::TemporaryRedirect,
    //         authority_redirect: Some(AuthorityRedirect::HostRedirect(Authority::from_static("www.redirected.com"))),
    //         strip_query: true,
    //         scheme_rewrite_specifier: Some(Scheme::HTTPS),
    //         path_rewrite_specifier: None,
    //     };

    //     let res = SyntheticHttpResponse::redirect(StatusCode::TEMPORARY_REDIRECT, ra).into_response(
    //         http::Version::HTTP_11,
    //         Some(uri),
    //         None,
    //     )?;

    //     let expected = &HeaderValue::from_str("https://www.redirected.com/foo/bar")?;

    //     assert_eq!(res.headers().get(http::header::LOCATION), Some(expected));

    //     Ok(())
    // }

    // #[test]
    // fn test_https_redirect_3() -> Result<(), InvalidSyntheticResponse> {
    //     let uri = "http://www.test.com/foo/bar?baz".parse::<Uri>().unwrap();

    //     let ra = RedirectAction {
    //         response_code: RedirectResponseCode::TemporaryRedirect,
    //         authority_redirect: Some(AuthorityRedirect::AuthorityRedirect(Authority::from_static(
    //             "www.redirected.com:8080",
    //         ))),
    //         strip_query: true,
    //         scheme_rewrite_specifier: Some(Scheme::HTTPS),
    //         path_rewrite_specifier: None,
    //     };

    //     let res = SyntheticHttpResponse::redirect(StatusCode::TEMPORARY_REDIRECT, ra).into_response(
    //         http::Version::HTTP_11,
    //         Some(uri),
    //         None,
    //     )?;

    //     let expected = &HeaderValue::from_str("https://www.redirected.com:8080/foo/bar")?;

    //     assert_eq!(res.headers().get(http::header::LOCATION), Some(expected));

    //     Ok(())
    // }

    // #[test]
    // fn test_scheme_redirect_1() -> Result<(), InvalidSyntheticResponse> {
    //     let uri = "http://www.test.com/foo/bar?baz".parse::<Uri>().unwrap();

    //     let ra = RedirectAction {
    //         response_code: RedirectResponseCode::TemporaryRedirect,
    //         authority_redirect: Some(AuthorityRedirect::AuthorityRedirect(Authority::from_static(
    //             "www.redirected.com:80",
    //         ))),
    //         strip_query: true,
    //         scheme_rewrite_specifier: Some(Scheme::HTTPS),
    //         path_rewrite_specifier: None,
    //     };

    //     let res = SyntheticHttpResponse::redirect(StatusCode::TEMPORARY_REDIRECT, ra).into_response(
    //         http::Version::HTTP_11,
    //         Some(uri),
    //         None,
    //     )?;

    //     let expected = &HeaderValue::from_str("https://www.redirected.com:80/foo/bar")?;

    //     assert_eq!(res.headers().get(http::header::LOCATION), Some(expected));

    //     Ok(())
    // }

    // #[test]
    // fn test_scheme_redirect_2() -> Result<(), InvalidSyntheticResponse> {
    //     let uri = "https://www.test.com:443/foo/bar?baz".parse::<Uri>().unwrap();

    //     let ra = RedirectAction {
    //         response_code: RedirectResponseCode::TemporaryRedirect,
    //         authority_redirect: Some(AuthorityRedirect::AuthorityRedirect(Authority::from_static(
    //             "www.redirected.com",
    //         ))),
    //         strip_query: true,
    //         scheme_rewrite_specifier: Some(Scheme::HTTP),
    //         path_rewrite_specifier: None,
    //     };

    //     let res = SyntheticHttpResponse::redirect(StatusCode::TEMPORARY_REDIRECT, ra).into_response(
    //         http::Version::HTTP_11,
    //         Some(uri),
    //         None,
    //     )?;

    //     let expected = &HeaderValue::from_str("http://www.redirected.com/foo/bar")?;

    //     assert_eq!(res.headers().get(http::header::LOCATION), Some(expected));

    //     Ok(())
    // }

    // #[test]
    // fn test_scheme_redirect_3() -> Result<(), InvalidSyntheticResponse> {
    //     let uri = "https://www.test.com:443/foo/bar?baz".parse::<Uri>().unwrap();

    //     let ra = RedirectAction {
    //         response_code: RedirectResponseCode::TemporaryRedirect,
    //         authority_redirect: None,
    //         strip_query: true,
    //         scheme_rewrite_specifier: Some(Scheme::HTTP),
    //         path_rewrite_specifier: None,
    //     };

    //     let res = SyntheticHttpResponse::redirect(StatusCode::TEMPORARY_REDIRECT, ra).into_response(
    //         http::Version::HTTP_11,
    //         Some(uri),
    //         None,
    //     )?;

    //     let expected = &HeaderValue::from_str("http://www.test.com/foo/bar")?;

    //     assert_eq!(res.headers().get(http::header::LOCATION), Some(expected));

    //     Ok(())
    // }

    // #[test]
    // fn test_scheme_redirect_4() -> Result<(), InvalidSyntheticResponse> {
    //     let uri = "https://www.test.com/foo/bar?baz".parse::<Uri>().unwrap();

    //     let ra = RedirectAction {
    //         response_code: RedirectResponseCode::TemporaryRedirect,
    //         authority_redirect: Some(AuthorityRedirect::AuthorityRedirect(Authority::from_static(
    //             "www.redirected.com:443",
    //         ))),
    //         strip_query: true,
    //         scheme_rewrite_specifier: Some(Scheme::HTTP),
    //         path_rewrite_specifier: None,
    //     };

    //     let res = SyntheticHttpResponse::redirect(StatusCode::TEMPORARY_REDIRECT, ra).into_response(
    //         http::Version::HTTP_11,
    //         Some(uri),
    //         None,
    //     )?;

    //     let expected = &HeaderValue::from_str("http://www.redirected.com:443/foo/bar")?;

    //     assert_eq!(res.headers().get(http::header::LOCATION), Some(expected));

    //     Ok(())
    // }

    // #[test]
    // fn test_path_rewrite_1() -> Result<(), InvalidSyntheticResponse> {
    //     let uri = "http://www.test.com/foo/bar?baz".parse::<Uri>().unwrap();

    //     let ra = RedirectAction {
    //         response_code: RedirectResponseCode::TemporaryRedirect,
    //         authority_redirect: Some(AuthorityRedirect::AuthorityRedirect(Authority::from_static(
    //             "www.redirected.com:80",
    //         ))),
    //         strip_query: true,
    //         scheme_rewrite_specifier: None,
    //         path_rewrite_specifier: Some(PathRewriteSpecifier::Path(PathAndQuery::from_str("/hello/world")?)),
    //     };

    //     let res = SyntheticHttpResponse::redirect(StatusCode::TEMPORARY_REDIRECT, ra).into_response(
    //         http::Version::HTTP_11,
    //         Some(uri),
    //         None,
    //     )?;

    //     let expected = &HeaderValue::from_str("http://www.redirected.com:80/hello/world")?;

    //     assert_eq!(res.headers().get(http::header::LOCATION), Some(expected));

    //     Ok(())
    // }

    // #[test]
    // fn test_path_rewrite_2() -> Result<(), InvalidSyntheticResponse> {
    //     let uri = "http://www.test.com/foo/bar?baz".parse::<Uri>().unwrap();

    //     let ra = RedirectAction {
    //         response_code: RedirectResponseCode::TemporaryRedirect,
    //         authority_redirect: Some(AuthorityRedirect::AuthorityRedirect(Authority::from_static(
    //             "www.redirected.com:80",
    //         ))),
    //         strip_query: false,
    //         scheme_rewrite_specifier: None,
    //         path_rewrite_specifier: Some(PathRewriteSpecifier::Path(PathAndQuery::from_str("/hello/world")?)),
    //     };

    //     let res = SyntheticHttpResponse::redirect(StatusCode::TEMPORARY_REDIRECT, ra).into_response(
    //         http::Version::HTTP_11,
    //         Some(uri),
    //         None,
    //     )?;

    //     let expected = &HeaderValue::from_str("http://www.redirected.com:80/hello/world?baz")?;

    //     assert_eq!(res.headers().get(http::header::LOCATION), Some(expected));

    //     Ok(())
    // }

    // #[test]
    // fn test_path_rewrite_3() -> Result<(), InvalidSyntheticResponse> {
    //     let uri = "http://www.test.com/foo/bar?baz".parse::<Uri>().unwrap();

    //     let ra = RedirectAction {
    //         response_code: RedirectResponseCode::TemporaryRedirect,
    //         authority_redirect: Some(AuthorityRedirect::AuthorityRedirect(Authority::from_static(
    //             "www.redirected.com:80",
    //         ))),
    //         strip_query: false,
    //         scheme_rewrite_specifier: None,
    //         path_rewrite_specifier: Some(PathRewriteSpecifier::Path(PathAndQuery::from_str("/hello/world?foobar")?)),
    //     };

    //     let res = SyntheticHttpResponse::redirect(StatusCode::TEMPORARY_REDIRECT, ra).into_response(
    //         http::Version::HTTP_11,
    //         Some(uri),
    //         None,
    //     )?;

    //     let expected = &HeaderValue::from_str("http://www.redirected.com:80/hello/world?foobar")?;

    //     assert_eq!(res.headers().get(http::header::LOCATION), Some(expected));

    //     Ok(())
    // }

    // #[test]
    // fn redirect_unexpected_status_code() {
    //     let uri = "http://www.test.com/foo/bar?baz".parse::<Uri>().unwrap();

    //     let ra = RedirectAction {
    //         response_code: RedirectResponseCode::TemporaryRedirect,
    //         authority_redirect: Some(AuthorityRedirect::AuthorityRedirect(Authority::from_static(
    //             "www.redirected.com:81",
    //         ))),
    //         strip_query: true,
    //         scheme_rewrite_specifier: None,
    //         path_rewrite_specifier: None,
    //     };

    //     let result =
    //         SyntheticHttpResponse::redirect(StatusCode::OK, ra).into_response(http::Version::HTTP_11, Some(uri), None);

    //     assert!(matches!(result, Err(InvalidSyntheticResponse::RedirectUnexpectedStatusCode)));
    // }

    // #[test]
    // fn redirect_missing_uri() {
    //     let ra = RedirectAction {
    //         response_code: RedirectResponseCode::TemporaryRedirect,
    //         authority_redirect: Some(AuthorityRedirect::AuthorityRedirect(Authority::from_static(
    //             "www.redirected.com:81",
    //         ))),
    //         strip_query: false,
    //         scheme_rewrite_specifier: None,
    //         path_rewrite_specifier: None,
    //     };

    //     let result = SyntheticHttpResponse::redirect(StatusCode::TEMPORARY_REDIRECT, ra).into_response(
    //         http::Version::HTTP_11,
    //         None,
    //         None,
    //     );

    //     assert!(matches!(result, Err(InvalidSyntheticResponse::RedirectMissingUri)));
    // }
}
