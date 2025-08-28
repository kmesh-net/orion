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

use http::{Request, Response, Version};

pub fn request_head_size<T>(req: &Request<T>) -> usize {
    let version_len = http_version_len(req.version());
    let mut size = req.method().as_str().len()                // (es. "GET")
                   + 1                                        // space
                   + req.uri().path_and_query()
                        .map_or(1, |p| p.as_str().len())      // Path (or "/" by default)
                   + 1                                        // space
                   + version_len                              // version (e.g. "HTTP/1.1")
                   + 2; // \r\n

    size += request_headers_size(req);
    size += 2; // \r\n
    size
}

pub fn request_headers_size<T>(req: &Request<T>) -> usize {
    let mut size = 0;
    for (name, value) in req.headers() {
        size += name.as_str().len()                           // header name
                + 2                                           // ": "
                + value.as_bytes().len()                      // header value
                + 2; // \r\n
    }
    size
}

pub fn response_head_size<T>(res: &Response<T>) -> usize {
    let version_len = http_version_len(res.version());
    let status_code_len = 3;
    let mut size = version_len                                    // version
                   + 1                                            // space
                   + status_code_len                              // status code (e.g. "200")
                   + 1                                            // space
                   + res.status().canonical_reason()
                        .map_or(0, str::len)                   // reason phrase (es. "OK")
                   + 2; // \r\n

    size += response_headers_size(res);
    size += 2; // \r\n
    size
}

pub fn response_headers_size<T>(res: &Response<T>) -> usize {
    let mut size = 0;
    for (name, value) in res.headers() {
        size += name.as_str().len()
                + 2 // ": "
                + value.as_bytes().len()
                + 2; // \r\n
    }
    size
}

#[inline]
pub fn http_version_len(version: Version) -> usize {
    match version {
        Version::HTTP_11 | Version::HTTP_10 | Version::HTTP_09 => 8, // "HTTP/1.1"
        Version::HTTP_2 | Version::HTTP_3 => 6,                      // "HTTP/2"
        _ => format!("{version:?}").len(),                           // unreachable, but just in case
    }
}
