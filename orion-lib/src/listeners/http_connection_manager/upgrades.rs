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

use super::{RequestHandler, TransactionHandler};
use crate::{
    body::{body_with_metrics::BodyWithMetrics, response_flags::ResponseFlags},
    listeners::synthetic_http_response::SyntheticHttpResponse,
    transport::{policy::RequestExt, HttpChannel},
    PolyBody, Result,
};
use orion_format::types::ResponseFlags as FmtResponseFlags;

use http::{header, HeaderMap, HeaderValue, StatusCode, Version};
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;
use orion_configuration::config::network_filters::http_connection_manager::UpgradeType;
use tokio::io::copy_bidirectional;
use tracing::error;

const UPGRADE: &str = "upgrade";
const WEBSOCKET: &str = "websocket";

pub fn is_upgrade_connection(header_value: &str) -> bool {
    header_value.to_lowercase() == UPGRADE
}

pub fn is_websocket_upgrade(header_value: &str) -> bool {
    header_value.to_lowercase() == WEBSOCKET
}

pub fn is_valid_header(header_value: &HeaderValue) -> std::result::Result<&str, http::header::ToStrError> {
    header_value.to_str()
}

pub fn is_valid_websocket_upgrade_request(headers: &HeaderMap) -> std::result::Result<bool, String> {
    match (headers.get(header::CONNECTION), headers.get(header::UPGRADE)) {
        (Some(connection_header), Some(upgrade_header)) => {
            let connection_header = is_valid_header(connection_header)
                .map_err(|e| format!("Connection header value is not USASCII {e}"))?;
            let upgrade_header =
                is_valid_header(upgrade_header).map_err(|e| format!("Upgrade header value is not USASCII {e}"))?;
            let is_upgrade = is_upgrade_connection(connection_header);
            let is_websocket = is_websocket_upgrade(upgrade_header);
            match (is_upgrade, is_websocket) {
                (true, true) => Ok(true),
                (true, false) => Err(format!("Upgrade header value is not valid: {upgrade_header}")),
                (false, _) => Ok(false),
            }
        },
        _ => Ok(false),
    }
}

pub fn is_websocket_enabled_by_hcm(hcm_enabled_upgrades: &[UpgradeType]) -> bool {
    hcm_enabled_upgrades.iter().any(|upgrade| matches!(upgrade, UpgradeType::Websocket))
}

pub async fn handle_websocket_upgrade(
    trans_handler: &TransactionHandler,
    mut request: Request<BodyWithMetrics<PolyBody>>,
    svc_channel: &HttpChannel,
) -> Result<Response<PolyBody>> {
    let version = request.version();
    match version {
        Version::HTTP_11 => {
            let request_upgrade = hyper::upgrade::on(&mut request);
            match svc_channel.to_response(trans_handler, RequestExt::new(request)).await {
                Ok(mut upstream_response) if upstream_response.status() == StatusCode::SWITCHING_PROTOCOLS => {
                    let response_upgrade = hyper::upgrade::on(&mut upstream_response);
                    tokio::spawn(async move {
                        match (request_upgrade.await, response_upgrade.await) {
                            (Ok(request_upgraded), Ok(response_upgraded)) => {
                                let _ = copy_bidirectional(
                                    &mut TokioIo::new(request_upgraded),
                                    &mut TokioIo::new(response_upgraded),
                                )
                                .await
                                .map_err(|err| {
                                    error!("Upgrade failure, bidi copy failed for websocket {:?}", err);
                                    err
                                });
                            },
                            (req_state, resp_state) => {
                                error!(
                                    "Upgrade attempt failure, occurred during connection upgrade {:?},{:?}",
                                    req_state, resp_state
                                );
                            },
                        }
                    });
                    Ok(upstream_response)
                },
                Ok(response) => {
                    error!(
                        "Upgrade attempt failure, upstream did not accept websocket upgrade, returned status code {:?}",
                        response.status()
                    );
                    Ok(SyntheticHttpResponse::not_allowed(ResponseFlags(FmtResponseFlags::UPSTREAM_CONNECTION_FAILURE))
                        .into_response(version))
                },
                Err(err) => {
                    error!("Upgrade failed in attempting to establish upstream websocket {:?}", err);
                    Ok(SyntheticHttpResponse::bad_gateway(ResponseFlags(FmtResponseFlags::UPSTREAM_CONNECTION_FAILURE))
                        .into_response(version))
                },
            }
        },
        _ => Ok(SyntheticHttpResponse::bad_request().into_response(version)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_valid_websocket_upgrade_request() {
        let mut header_map = HeaderMap::new();
        header_map.insert("connection", "upgrade".parse().unwrap());
        header_map.insert("upgrade", "websocket".parse().unwrap());
        is_valid_websocket_upgrade_request(&header_map).unwrap();

        let mut header_map = HeaderMap::new();
        header_map.insert("connection", "dfkdjkfjk".parse().unwrap());
        header_map.insert("upgrade", "websocket".parse().unwrap());
        assert_eq!(Ok(false), is_valid_websocket_upgrade_request(&header_map));

        let mut header_map = HeaderMap::new();
        header_map.insert("connection", "upgrade".parse().unwrap());
        header_map.insert("upgrade", "websocketsdklkd".parse().unwrap());

        if let Err(e) = is_valid_websocket_upgrade_request(&header_map) {
            assert!(e.starts_with("Upgrade header value is not valid"));
        } else {
            unreachable!();
        }

        let mut header_map = HeaderMap::new();
        header_map.insert("connection", "无效的".parse().unwrap());
        header_map.insert("upgrade", "websocket".parse().unwrap());

        if let Err(e) = is_valid_websocket_upgrade_request(&header_map) {
            assert!(e.starts_with("Connection header value is not USASCII"));
        } else {
            unreachable!();
        }

        let mut header_map = HeaderMap::new();
        header_map.insert("connection", "upgrade".parse().unwrap());
        header_map.insert("upgrade", "无效的".parse().unwrap());

        if let Err(e) = is_valid_websocket_upgrade_request(&header_map) {
            assert!(e.starts_with("Upgrade header value is not USASCII"));
        } else {
            unreachable!();
        }
    }
}
