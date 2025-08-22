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

use http::{HeaderValue, Request, Response};
use orion_http_header::X_REQUEST_ID;
use tracing::info;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct RequestIdManager {
    generate_request_id: bool,
    preserve_external_request_id: bool,
    always_set_request_id_in_response: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestId {
    Propagate(HeaderValue),
    Internal(HeaderValue),
}

impl AsRef<HeaderValue> for RequestId {
    fn as_ref(&self) -> &HeaderValue {
        match self {
            RequestId::Propagate(id) | RequestId::Internal(id) => id,
        }
    }
}

impl RequestId {
    pub fn from_request<B>(request: &Request<B>) -> Option<Self> {
        let value = request.headers().get(X_REQUEST_ID).filter(|v| {
            v.to_str()
                .and_then(|s| {
                    Uuid::parse_str(s).map(|_| true).or_else(|_| {
                        info!("Invalid UUID in X-Request-ID header: {}", v.to_str().unwrap_or("invalid"));
                        Ok(false)
                    })
                })
                .unwrap_or(false)
        });
        match value {
            None => None,
            Some(id) if id.is_empty() => None,
            Some(id) => Some(RequestId::Propagate(id.clone())),
        }
    }

    pub fn propagate_ref(&self) -> Option<&HeaderValue> {
        match self {
            RequestId::Propagate(id) => Some(id),
            RequestId::Internal(_) => None,
        }
    }

    pub fn internal_ref(&self) -> Option<&HeaderValue> {
        match self {
            RequestId::Internal(id) => Some(id),
            RequestId::Propagate(_) => None,
        }
    }
}

impl RequestIdManager {
    pub fn new(
        generate_request_id: bool,
        preserve_external_request_id: bool,
        always_set_request_id_in_response: bool,
    ) -> Self {
        Self { generate_request_id, preserve_external_request_id, always_set_request_id_in_response }
    }

    pub fn apply_policy<B>(&self, req: Request<B>) -> (Request<B>, RequestId) {
        let (mut parts, body) = req.into_parts();
        let existing_id = parts.headers.get(X_REQUEST_ID).cloned();
        let (authoritative_id, generated) = match existing_id.as_ref() {
            Some(id) if self.preserve_external_request_id => (id.clone(), false),
            _ => (Self::generate_new_id(), true),
        };

        // 2. Determine if the ID must be propagated...
        let should_propagate_header =
            (existing_id.is_some() && self.preserve_external_request_id) || self.generate_request_id;

        // 3. Apply the changes to the rqeuest...
        if should_propagate_header {
            if generated {
                parts.headers.insert(X_REQUEST_ID, authoritative_id.clone());
            } // if not generated, we keep the existing ID
        } else {
            if existing_id.is_some() {
                parts.headers.remove(X_REQUEST_ID);
            }
        }

        // 4. Create the RequestId...
        let req_id = if should_propagate_header {
            RequestId::Propagate(authoritative_id)
        } else {
            RequestId::Internal(authoritative_id)
        };

        (Request::from_parts(parts, body), req_id)
    }

    #[inline]
    fn generate_new_id() -> HeaderValue {
        let mut buffer = [0u8; 32];
        let new_id_str = uuid::Uuid::new_v4().simple().encode_lower(&mut buffer);
        HeaderValue::from_str(new_id_str).unwrap_or_else(|e| {
            info!("UUID string should be valid HeaderValue: {e}");
            // Fallback in case of an error, though this should not happen with valid UUIDs
            HeaderValue::from_static("unknown-request-id")
        })
    }

    pub fn apply_to<B>(&self, resp: &mut Response<B>, req_id: Option<&HeaderValue>) {
        if self.always_set_request_id_in_response {
            req_id.inspect(|id| {
                resp.headers_mut().insert(X_REQUEST_ID, (*id).clone());
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::Request;

    #[test]
    fn test_request_id_from_request() {
        let request = Request::builder().header(X_REQUEST_ID, "123e4567-e89b-12d3-a456-426614174000").body(()).unwrap();
        let request_id = RequestId::from_request(&request);
        assert!(request_id.is_some());
        if let Some(RequestId::Propagate(id)) = request_id {
            assert_eq!(id.to_str().unwrap(), "123e4567-e89b-12d3-a456-426614174000");
        } else {
            panic!("Expected RequestId::Propagate, got {request_id:?}");
        }
    }

    #[test]
    fn test_broken_request_id_from_request() {
        let request = Request::builder().header(X_REQUEST_ID, "123e4567-invalid-614174").body(()).unwrap();
        let request_id = RequestId::from_request(&request);
        assert!(request_id.is_none());
    }

    #[test]
    fn test_not_avail_request_id_from_request() {
        let request = Request::builder().body(()).unwrap();
        let request_id = RequestId::from_request(&request);
        assert!(request_id.is_none());
    }

    #[test]
    fn test_req_id_manager_apply_poliy() {
        // generate = false, preserve = false, always_set = false
        let manager = RequestIdManager::new(false, false, false);
        let request = Request::builder().header(X_REQUEST_ID, "123e4567-e89b-12d3-a456-426614174000").body(()).unwrap();
        let (modified_request, req_id) = manager.apply_policy(request);
        assert!(!modified_request.headers().contains_key(X_REQUEST_ID));
        assert!(matches!(req_id, RequestId::Internal(_)));

        // generate = true, preserve = false, always_set = false
        let manager = RequestIdManager::new(true, false, false);
        let request = Request::builder().header(X_REQUEST_ID, "123e4567-e89b-12d3-a456-426614174000").body(()).unwrap();
        let (modified_request, req_id) = manager.apply_policy(request);
        assert!(modified_request.headers().contains_key(X_REQUEST_ID));
        assert!(matches!(req_id, RequestId::Propagate(_)));
        assert_ne!(
            modified_request.headers().get(X_REQUEST_ID),
            Some(&HeaderValue::from_static("123e4567-e89b-12d3-a456-426614174000"))
        );

        // generate = true, preserve = true, always_set = false
        let manager = RequestIdManager::new(true, true, false);
        let request = Request::builder().body(()).unwrap();
        let (modified_request, req_id) = manager.apply_policy(request);
        assert!(modified_request.headers().contains_key(X_REQUEST_ID));
        assert!(matches!(req_id, RequestId::Propagate(_)));

        // generate = true, preserve = true, always_set = false (with request already having X-Request-ID)
        let manager = RequestIdManager::new(true, true, false);
        let request = Request::builder().header(X_REQUEST_ID, "123e4567-e89b-12d3-a456-426614174000").body(()).unwrap();
        let (modified_request, req_id) = manager.apply_policy(request);
        assert!(modified_request.headers().contains_key(X_REQUEST_ID));
        assert!(matches!(req_id, RequestId::Propagate(_)));
        assert_eq!(
            modified_request.headers().get(X_REQUEST_ID),
            Some(&HeaderValue::from_static("123e4567-e89b-12d3-a456-426614174000"))
        );
    }
}
