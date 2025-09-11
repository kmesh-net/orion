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

use bitflags::{bitflags, bitflags_match};
use smol_str::{SmolStr, SmolStrBuilder, ToSmolStr};

bitflags! {
    #[derive(PartialEq, Clone, Debug)]
    pub struct ResponseFlags: u32 {
        const NO_HEALTHY_UPSTREAM                   = 0b00_0000_0000_0000_0000_0000_0000_0001;
        const UPSTREAM_CONNECTION_FAILURE           = 0b00_0000_0000_0000_0000_0000_0000_0010;
        const UPSTREAM_OVERFLOW                     = 0b00_0000_0000_0000_0000_0000_0000_0100;
        const NO_ROUTE_FOUND                        = 0b00_0000_0000_0000_0000_0000_0000_1000;
        const UPSTREAM_RETRY_LIMIT_EXCEEDED         = 0b00_0000_0000_0000_0000_0000_0001_0000;
        const NO_CLUSTER_FOUND                      = 0b00_0000_0000_0000_0000_0000_0010_0000;
        const DURATION_TIMEOUT                      = 0b00_0000_0000_0000_0000_0000_0100_0000;
        const DOWNSTREAM_CONNECTION_TERMINATION     = 0b00_0000_0000_0000_0000_0000_1000_0000;
        const FAILED_LOCAL_HEALTH_CHECK             = 0b00_0000_0000_0000_0000_0001_0000_0000;
        const UPSTREAM_REQUEST_TIMEOUT              = 0b00_0000_0000_0000_0000_0010_0000_0000;
        const LOCAL_RESET                           = 0b00_0000_0000_0000_0000_0100_0000_0000;
        const UPSTREAM_REMOTE_RESET                 = 0b00_0000_0000_0000_0000_1000_0000_0000;
        const UPSTREAM_CONNECTION_TERMINATION       = 0b00_0000_0000_0000_0001_0000_0000_0000;
        const DELAY_INJECTED                        = 0b00_0000_0000_0000_0010_0000_0000_0000;
        const FAULT_INJECTED                        = 0b00_0000_0000_0000_0100_0000_0000_0000;
        const RATE_LIMITED                          = 0b00_0000_0000_0000_1000_0000_0000_0000;
        const UNAUTHORIZED_EXTERNAL_SERVICE         = 0b00_0000_0000_0001_0000_0000_0000_0000;
        const RATE_LIMIT_SERVICE_ERROR              = 0b00_0000_0000_0010_0000_0000_0000_0000;
        const INVALID_ENVOY_REQUEST_HEADERS         = 0b00_0000_0000_0100_0000_0000_0000_0000;
        const STREAM_IDLE_TIMEOUT                   = 0b00_0000_0000_1000_0000_0000_0000_0000;
        const DOWNSTREAM_PROTOCOL_ERROR             = 0b00_0000_0001_0000_0000_0000_0000_0000;
        const UPSTREAM_PROTOCOL_ERROR               = 0b00_0000_0010_0000_0000_0000_0000_0000;
        const UPSTREAM_MAX_STREAM_DURATION_REACHED  = 0b00_0000_0100_0000_0000_0000_0000_0000;
        const RESPONSE_FROM_CACHE_FILTER            = 0b00_0000_1000_0000_0000_0000_0000_0000;
        const NO_FILTER_CONFIG_FOUND                = 0b00_0001_0000_0000_0000_0000_0000_0000;
        const OVERLOAD_MANAGER_TERMINATED           = 0b00_0010_0000_0000_0000_0000_0000_0000;
        const DNS_RESOLUTION_FAILED                 = 0b00_0100_0000_0000_0000_0000_0000_0000;
        const DROP_OVERLOAD                         = 0b00_1000_0000_0000_0000_0000_0000_0000;
        const DOWNSTREAM_REMOTE_RESET               = 0b01_0000_0000_0000_0000_0000_0000_0000;
        const UNCONDITIONAL_DROP_OVERLOAD           = 0b10_0000_0000_0000_0000_0000_0000_0000;
    }
}

pub struct ResponseFlagsShort<'a>(pub &'a ResponseFlags);
pub struct ResponseFlagsLong<'a>(pub &'a ResponseFlags);

impl ToSmolStr for ResponseFlagsShort<'_> {
    fn to_smolstr(&self) -> SmolStr {
        if self.0.is_empty() {
            return SmolStr::new_static("-");
        }
        let mut flags = SmolStrBuilder::new();

        for (i, f) in self.0.clone().iter().enumerate() {
            if i > 0 {
                flags.push(',');
            }
            bitflags_match!(f, {
                ResponseFlags::NO_HEALTHY_UPSTREAM                   => flags.push_str("UH"),
                ResponseFlags::UPSTREAM_CONNECTION_FAILURE           => flags.push_str("UF"),
                ResponseFlags::UPSTREAM_OVERFLOW                     => flags.push_str("UO"),
                ResponseFlags::NO_ROUTE_FOUND                        => flags.push_str("NR"),
                ResponseFlags::UPSTREAM_RETRY_LIMIT_EXCEEDED         => flags.push_str("URX"),
                ResponseFlags::NO_CLUSTER_FOUND                      => flags.push_str("NC"),
                ResponseFlags::DURATION_TIMEOUT                      => flags.push_str("DT"),
                ResponseFlags::DOWNSTREAM_CONNECTION_TERMINATION     => flags.push_str("DC"),
                ResponseFlags::FAILED_LOCAL_HEALTH_CHECK             => flags.push_str("LH"),
                ResponseFlags::UPSTREAM_REQUEST_TIMEOUT              => flags.push_str("UT"),
                ResponseFlags::LOCAL_RESET                           => flags.push_str("LR"),
                ResponseFlags::UPSTREAM_REMOTE_RESET                 => flags.push_str("UR"),
                ResponseFlags::UPSTREAM_CONNECTION_TERMINATION       => flags.push_str("UC"),
                ResponseFlags::DELAY_INJECTED                        => flags.push_str("DI"),
                ResponseFlags::FAULT_INJECTED                        => flags.push_str("FI"),
                ResponseFlags::RATE_LIMITED                          => flags.push_str("RL"),
                ResponseFlags::UNAUTHORIZED_EXTERNAL_SERVICE         => flags.push_str("UAEX"),
                ResponseFlags::RATE_LIMIT_SERVICE_ERROR              => flags.push_str("RLSE"),
                ResponseFlags::INVALID_ENVOY_REQUEST_HEADERS         => flags.push_str("IH"),
                ResponseFlags::STREAM_IDLE_TIMEOUT                   => flags.push_str("SI"),
                ResponseFlags::DOWNSTREAM_PROTOCOL_ERROR             => flags.push_str("DPE"),
                ResponseFlags::UPSTREAM_PROTOCOL_ERROR               => flags.push_str("UPE"),
                ResponseFlags::UPSTREAM_MAX_STREAM_DURATION_REACHED  => flags.push_str("UMSDR"),
                ResponseFlags::RESPONSE_FROM_CACHE_FILTER            => flags.push_str("RFCF"),
                ResponseFlags::NO_FILTER_CONFIG_FOUND                => flags.push_str("NFCF"),
                ResponseFlags::OVERLOAD_MANAGER_TERMINATED           => flags.push_str("OM"),
                ResponseFlags::DNS_RESOLUTION_FAILED                 => flags.push_str("DF"),
                ResponseFlags::DROP_OVERLOAD                         => flags.push_str("DO"),
                ResponseFlags::DOWNSTREAM_REMOTE_RESET               => flags.push_str("DR"),
                ResponseFlags::UNCONDITIONAL_DROP_OVERLOAD           => flags.push_str("UDO"),
                _ => flags.push_str("?"),
            });
        }

        flags.finish()
    }
}

impl ToSmolStr for ResponseFlagsLong<'_> {
    fn to_smolstr(&self) -> SmolStr {
        if self.0.is_empty() {
            return SmolStr::new_static("-");
        }
        let mut flags = SmolStrBuilder::new();

        for (i, f) in self.0.clone().iter().enumerate() {
            if i > 0 {
                flags.push(',');
            }
            bitflags_match!(f, {
                ResponseFlags::NO_HEALTHY_UPSTREAM                   => flags.push_str("NoHealthyUpstream"),
                ResponseFlags::UPSTREAM_CONNECTION_FAILURE           => flags.push_str("UpstreamConnectionFailure"),
                ResponseFlags::UPSTREAM_OVERFLOW                     => flags.push_str("UpstreamOverflow"),
                ResponseFlags::NO_ROUTE_FOUND                        => flags.push_str("NoRouteFound"),
                ResponseFlags::UPSTREAM_RETRY_LIMIT_EXCEEDED         => flags.push_str("UpstreamRetryLimitExceeded"),
                ResponseFlags::NO_CLUSTER_FOUND                      => flags.push_str("NoClusterFound"),
                ResponseFlags::DURATION_TIMEOUT                      => flags.push_str("DurationTimeout"),
                ResponseFlags::DOWNSTREAM_CONNECTION_TERMINATION     => flags.push_str("DownstreamConnectionTermination"),
                ResponseFlags::FAILED_LOCAL_HEALTH_CHECK             => flags.push_str("FailedLocalHealthCheck"),
                ResponseFlags::UPSTREAM_REQUEST_TIMEOUT              => flags.push_str("UpstreamRequestTimeout"),
                ResponseFlags::LOCAL_RESET                           => flags.push_str("LocalReset"),
                ResponseFlags::UPSTREAM_REMOTE_RESET                 => flags.push_str("UpstreamRemoteReset"),
                ResponseFlags::UPSTREAM_CONNECTION_TERMINATION       => flags.push_str("UpstreamConnectionTermination"),
                ResponseFlags::DELAY_INJECTED                        => flags.push_str("DelayInjected"),
                ResponseFlags::FAULT_INJECTED                        => flags.push_str("FaultInjected"),
                ResponseFlags::RATE_LIMITED                          => flags.push_str("RateLimited"),
                ResponseFlags::UNAUTHORIZED_EXTERNAL_SERVICE         => flags.push_str("UnauthorizedExternalService"),
                ResponseFlags::RATE_LIMIT_SERVICE_ERROR              => flags.push_str("RateLimitServiceError"),
                ResponseFlags::INVALID_ENVOY_REQUEST_HEADERS         => flags.push_str("InvalidEnvoyRequestHeaders"),
                ResponseFlags::STREAM_IDLE_TIMEOUT                   => flags.push_str("StreamIdleTimeout"),
                ResponseFlags::DOWNSTREAM_PROTOCOL_ERROR             => flags.push_str("DownstreamProtocolError"),
                ResponseFlags::UPSTREAM_PROTOCOL_ERROR               => flags.push_str("UpstreamProtocolError"),
                ResponseFlags::UPSTREAM_MAX_STREAM_DURATION_REACHED  => flags.push_str("UpstreamMaxStreamDurationReached"),
                ResponseFlags::RESPONSE_FROM_CACHE_FILTER            => flags.push_str("ResponseFromCacheFilter"),
                ResponseFlags::NO_FILTER_CONFIG_FOUND                => flags.push_str("NoFilterConfigFound"),
                ResponseFlags::OVERLOAD_MANAGER_TERMINATED           => flags.push_str("OverloadManagerTerminated"),
                ResponseFlags::DNS_RESOLUTION_FAILED                 => flags.push_str("DnsResolutionFailed"),
                ResponseFlags::DROP_OVERLOAD                         => flags.push_str("DropOverload"),
                ResponseFlags::DOWNSTREAM_REMOTE_RESET               => flags.push_str("DownstreamRemoteReset"),
                ResponseFlags::UNCONDITIONAL_DROP_OVERLOAD           => flags.push_str("UnconditionalDropOverload"),
                _ => flags.push_str("?"),
            });
        }

        flags.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_response_flags_short() {
        let f = ResponseFlags::empty();
        assert_eq!(ResponseFlagsShort(&f).to_smolstr(), SmolStr::new_static("-"));
        let f = ResponseFlags::NO_ROUTE_FOUND;
        assert_eq!(ResponseFlagsShort(&f).to_smolstr(), SmolStr::new_static("NR"));
        let f = ResponseFlags::NO_HEALTHY_UPSTREAM | ResponseFlags::NO_ROUTE_FOUND;
        assert_eq!(ResponseFlagsShort(&f).to_smolstr(), SmolStr::new_static("UH,NR"));
    }

    #[test]
    fn test_response_flags_long() {
        let f = ResponseFlags::empty();
        assert_eq!(ResponseFlagsLong(&f).to_smolstr(), SmolStr::new_static("-"));
        let f = ResponseFlags::NO_ROUTE_FOUND;
        assert_eq!(ResponseFlagsLong(&f).to_smolstr(), SmolStr::new_static("NoRouteFound"));
        let f = ResponseFlags::NO_HEALTHY_UPSTREAM | ResponseFlags::NO_ROUTE_FOUND;
        assert_eq!(ResponseFlagsLong(&f).to_smolstr(), SmolStr::new_static("NoHealthyUpstream,NoRouteFound"));
    }
}
