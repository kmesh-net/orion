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

#[cfg(feature = "envoy-conversions")]
pub(crate) use envoy_conversions::*;

#[cfg(feature = "envoy-conversions")]
mod envoy_conversions {
    use crate::{config::common::GenericError, config::WithNodeOnResult};
    use http::StatusCode;
    use orion_data_plane_api::envoy_data_plane_api::{
        envoy::r#type::v3::HttpStatus as EnvoyHttpStatus,
        google::protobuf::{Duration as PbDuration, UInt32Value},
    };
    use std::{fmt::Display, time::Duration};

    pub fn u32_to_u16(value: u32) -> Result<u16, GenericError> {
        u16::try_from(value).map_err(|_| GenericError::from_msg(format!("failed to convert {value} to a u16")))
    }

    #[allow(clippy::needless_pass_by_value)]
    pub fn envoy_u32_to_u16(value: UInt32Value) -> Result<u16, GenericError> {
        u32_to_u16(value.value)
    }

    pub fn http_status_from<T: TryInto<u16> + Display + Copy>(status: T) -> Result<StatusCode, GenericError> {
        StatusCode::from_u16(
            status.try_into().map_err(|_| GenericError::from_msg(format!("invalid status code {status}")))?,
        )
        .map_err(|e| GenericError::from_msg_with_cause(format!("invalid status code {status}"), e))
    }

    pub fn duration_from_envoy(duration: PbDuration) -> Result<Duration, GenericError> {
        match (u64::try_from(duration.seconds), u32::try_from(duration.nanos)) {
            (Ok(seconds), Ok(nanos)) => Ok(Duration::new(seconds, nanos)),
            (_, _) => Err(GenericError::from_msg(format!("Failed to conver {duration:?} into a Duration"))),
        }
    }
    #[allow(clippy::needless_pass_by_value)]
    pub fn http_status_from_envoy(status: EnvoyHttpStatus) -> Result<StatusCode, GenericError> {
        let EnvoyHttpStatus { code } = status;
        super::http_status_from(code).with_node("code")
    }
}
