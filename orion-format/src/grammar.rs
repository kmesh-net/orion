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

use std::sync::LazyLock;

use crate::{
    operator::{Category, HeaderName, Operator, ReqArgument, RespArgument},
    FormatError, Grammar, Template,
};
use ptrie::Trie;
use smol_str::SmolStr;

macro_rules! trie_mapstr {
    ($trie:expr, $lit:literal, $tok:expr, $cat:expr) => {
        $trie.insert($lit.bytes(), ($tok, $cat, $lit.len(), false)); // expand the variadic arguments into the tuple.
    };
    ($trie:expr, $lit:literal, $tok:expr, $cat:expr, $arg:expr ) => {
        $trie.insert($lit.bytes(), ($tok, $cat, $lit.len(), $arg)); // expand the variadic arguments into the tuple.
    };
}

static ENVOY_REQ_ARGS: LazyLock<Trie<u8, (ReqArgument, Category, usize, bool)>> = LazyLock::new(|| {
    let mut trie = Trie::new();
    trie_mapstr!(trie, ":SCHEME", ReqArgument::Scheme, Category::ARGUMENT);
    trie_mapstr!(trie, ":METHOD", ReqArgument::Method, Category::ARGUMENT);
    trie_mapstr!(trie, ":PATH", ReqArgument::Path, Category::ARGUMENT);
    trie_mapstr!(trie, ":AUTHORITY", ReqArgument::Authority, Category::ARGUMENT);
    trie_mapstr!(trie, "X-ENVOY-ORIGINAL-PATH?:PATH", ReqArgument::OriginalPathOrPath, Category::ARGUMENT);
    trie
});

static ENVOY_RESP_ARGS: LazyLock<Trie<u8, (RespArgument, Category, usize, bool)>> = LazyLock::new(|| {
    let mut trie = Trie::new();
    trie_mapstr!(trie, ":STATUS", RespArgument::Status, Category::ARGUMENT);
    trie
});

static ENVOY_PATTERNS: LazyLock<Trie<u8, (Operator, Category, usize, bool)>> = LazyLock::new(|| {
    let mut trie = Trie::new();
    trie_mapstr!(trie, "REQUEST_DURATION", Operator::RequestDuration, Category::REQUEST_DURATION);
    trie_mapstr!(trie, "REQUEST_TX_DURATION", Operator::RequestTxDuration, Category::REQUEST_DURATION);
    trie_mapstr!(trie, "RESPONSE_DURATION", Operator::ResponseDuration, Category::RESPONSE_DURATION);
    trie_mapstr!(trie, "TIME_TO_FIRST_BYTE", Operator::ResponseDuration, Category::RESPONSE_DURATION); // alias for RESPONSE_DURATION
    trie_mapstr!(trie, "RESPONSE_TX_DURATION", Operator::ResponseTxDuration, Category::RESPONSE_DURATION);
    trie_mapstr!(trie, "DOWNSTREAM_HANDSHAKE_DURATION", Operator::DownstreamHandshakeDuration, Category::UNSUPPORTED);
    trie_mapstr!(trie, "ROUNDTRIP_DURATION", Operator::RoundtripDuration, Category::UNSUPPORTED);
    trie_mapstr!(trie, "BYTES_RECEIVED", Operator::BytesReceived, Category::FINISH_CONTEXT);
    trie_mapstr!(trie, "BYTES_RETRANSMITTED", Operator::BytesRetransmitted, Category::UNSUPPORTED);
    trie_mapstr!(trie, "PACKETS_RETRANSMITTED", Operator::PacketsRetransmitted, Category::UNSUPPORTED);
    trie_mapstr!(trie, "UPSTREAM_WIRE_BYTES_RECEIVED", Operator::UpstreamWireBytesReceived, Category::UNSUPPORTED);
    trie_mapstr!(trie, "UPSTREAM_HEADER_BYTES_RECEIVED", Operator::UpstreamHeaderBytesReceived, Category::UNSUPPORTED);
    trie_mapstr!(trie, "DOWNSTREAM_WIRE_BYTES_RECEIVED", Operator::DownstreamWireBytesReceived, Category::UNSUPPORTED);
    trie_mapstr!(
        trie,
        "DOWNSTREAM_HEADER_BYTES_RECEIVED",
        Operator::DownstreamHeaderBytesReceived,
        Category::UNSUPPORTED
    );
    trie_mapstr!(trie, "PROTOCOL", Operator::Protocol, Category::DOWNSTREAM_REQUEST);
    trie_mapstr!(trie, "UPSTREAM_PROTOCOL", Operator::UpstreamProtocol, Category::UPSTREAM_REQUEST);
    trie_mapstr!(trie, "RESPONSE_CODE", Operator::ResponseCode, Category::DOWNSTREAM_RESPONSE);
    trie_mapstr!(trie, "RESPONSE_CODE_DETAILS", Operator::ResponseCodeDetails, Category::FINISH_CONTEXT);
    trie_mapstr!(
        trie,
        "CONNECTION_TERMINATION_DETAILS",
        Operator::ConnectionTerminationDetails,
        Category::FINISH_CONTEXT
    );
    trie_mapstr!(trie, "BYTES_SENT", Operator::BytesSent, Category::FINISH_CONTEXT);
    trie_mapstr!(trie, "UPSTREAM_WIRE_BYTES_SENT", Operator::UpstreamWireBytesSent, Category::UNSUPPORTED);
    trie_mapstr!(trie, "UPSTREAM_HEADER_BYTES_SENT", Operator::UpstreamHeaderBytesSent, Category::UNSUPPORTED);
    trie_mapstr!(trie, "DOWNSTREAM_WIRE_BYTES_SENT", Operator::DownstreamWireBytesSent, Category::UNSUPPORTED);
    trie_mapstr!(trie, "DOWNSTREAM_HEADER_BYTES_SENT", Operator::DownstreamHeaderBytesSent, Category::UNSUPPORTED);
    trie_mapstr!(trie, "DURATION", Operator::Duration, Category::FINISH_CONTEXT);
    trie_mapstr!(trie, "COMMON_DURATION", Operator::CommonDuration, Category::UNSUPPORTED);
    trie_mapstr!(trie, "CUSTOM_FLAGS", Operator::CustomFlags, Category::UNSUPPORTED);
    trie_mapstr!(trie, "RESPONSE_FLAGS", Operator::ResponseFlags, Category::FINISH_CONTEXT);
    trie_mapstr!(trie, "RESPONSE_FLAGS_LONG", Operator::ResponseFlagsLong, Category::FINISH_CONTEXT);
    trie_mapstr!(trie, "UPSTREAM_HOST_NAME", Operator::UpstreamHostName, Category::UPSTREAM_CONTEXT);
    trie_mapstr!(
        trie,
        "UPSTREAM_HOST_NAME_WITHOUT_PORT",
        Operator::UpstreamHostNameWithoutPort,
        Category::UPSTREAM_CONTEXT
    );
    trie_mapstr!(trie, "UPSTREAM_HOST", Operator::UpstreamHost, Category::UPSTREAM_CONTEXT);
    trie_mapstr!(trie, "UPSTREAM_CONNECTION_ID", Operator::UpstreamConnectionId, Category::UPSTREAM_CONTEXT);
    trie_mapstr!(trie, "UPSTREAM_CLUSTER", Operator::UpstreamCluster, Category::UPSTREAM_CONTEXT);
    trie_mapstr!(trie, "UPSTREAM_CLUSTER_RAW", Operator::UpstreamClusterRaw, Category::UPSTREAM_CONTEXT);
    trie_mapstr!(trie, "UPSTREAM_LOCAL_ADDRESS", Operator::UpstreamLocalAddress, Category::UPSTREAM_CONTEXT);
    trie_mapstr!(
        trie,
        "UPSTREAM_LOCAL_ADDRESS_WITHOUT_PORT",
        Operator::UpstreamLocalAddressWithoutPort,
        Category::UPSTREAM_CONTEXT
    );
    trie_mapstr!(trie, "UPSTREAM_LOCAL_PORT", Operator::UpstreamLocalPort, Category::UPSTREAM_CONTEXT);
    trie_mapstr!(trie, "UPSTREAM_REMOTE_ADDRESS", Operator::UpstreamRemoteAddress, Category::UPSTREAM_CONTEXT);
    trie_mapstr!(
        trie,
        "UPSTREAM_REMOTE_ADDRESS_WITHOUT_PORT",
        Operator::UpstreamRemoteAddressWithoutPort,
        Category::UPSTREAM_CONTEXT
    );
    trie_mapstr!(trie, "UPSTREAM_REMOTE_PORT", Operator::UpstreamRemotePort, Category::UPSTREAM_CONTEXT);
    trie_mapstr!(trie, "DOWNSTREAM_LOCAL_ADDRESS", Operator::DownstreamLocalAddress, Category::DOWNSTREAM_CONTEXT);
    trie_mapstr!(
        trie,
        "DOWNSTREAM_LOCAL_ADDRESS_WITHOUT_PORT",
        Operator::DownstreamLocalAddressWithoutPort,
        Category::DOWNSTREAM_CONTEXT
    );
    trie_mapstr!(trie, "DOWNSTREAM_LOCAL_PORT", Operator::DownstreamLocalPort, Category::DOWNSTREAM_CONTEXT);
    trie_mapstr!(trie, "DOWNSTREAM_REMOTE_ADDRESS", Operator::DownstreamRemoteAddress, Category::DOWNSTREAM_CONTEXT);
    trie_mapstr!(
        trie,
        "DOWNSTREAM_REMOTE_ADDRESS_WITHOUT_PORT",
        Operator::DownstreamRemoteAddressWithoutPort,
        Category::DOWNSTREAM_CONTEXT
    );
    trie_mapstr!(trie, "DOWNSTREAM_REMOTE_PORT", Operator::DownstreamRemotePort, Category::DOWNSTREAM_CONTEXT);
    trie_mapstr!(trie, "UPSTREAM_REQUEST_ATTEMPT_COUNT", Operator::UpstreamRequestAttemptCount, Category::UNSUPPORTED);
    trie_mapstr!(trie, "UPSTREAM_TLS_CIPHER", Operator::UpstreamTlsCipher, Category::UNSUPPORTED);
    trie_mapstr!(trie, "UPSTREAM_TLS_VERSION", Operator::UpstreamTlsVersion, Category::UNSUPPORTED);
    trie_mapstr!(trie, "UPSTREAM_TLS_SESSION_ID", Operator::UpstreamTlsSessionId, Category::UNSUPPORTED);
    trie_mapstr!(trie, "UPSTREAM_PEER_ISSUER", Operator::UpstreamPeerIssuer, Category::UNSUPPORTED);
    trie_mapstr!(trie, "UPSTREAM_PEER_CERT", Operator::UpstreamPeerCert, Category::UNSUPPORTED);
    trie_mapstr!(trie, "UPSTREAM_PEER_SUBJECT", Operator::UpstreamPeerSubject, Category::UNSUPPORTED);
    trie_mapstr!(
        trie,
        "DOWNSTREAM_DIRECT_LOCAL_ADDRESS",
        Operator::DownstreamDirectLocalAddress,
        Category::UNSUPPORTED
    );
    trie_mapstr!(
        trie,
        "DOWNSTREAM_DIRECT_LOCAL_ADDRESS_WITHOUT_PORT",
        Operator::DownstreamDirectLocalAddressWithoutPort,
        Category::UNSUPPORTED
    );
    trie_mapstr!(trie, "DOWNSTREAM_DIRECT_LOCAL_PORT", Operator::DownstreamDirectLocalPort, Category::UNSUPPORTED);
    trie_mapstr!(
        trie,
        "DOWNSTREAM_DIRECT_REMOTE_ADDRESS",
        Operator::DownstreamDirectRemoteAddress,
        Category::UNSUPPORTED
    );
    trie_mapstr!(
        trie,
        "DOWNSTREAM_DIRECT_REMOTE_ADDRESS_WITHOUT_PORT",
        Operator::DownstreamDirectRemoteAddressWithoutPort,
        Category::UNSUPPORTED
    );
    trie_mapstr!(trie, "DOWNSTREAM_DIRECT_REMOTE_PORT", Operator::DownstreamDirectRemotePort, Category::UNSUPPORTED);
    trie_mapstr!(trie, "CONNECTION_ID", Operator::ConnectionId, Category::DOWNSTREAM_CONTEXT);
    trie_mapstr!(trie, "REQUEST_HEADERS_BYTES", Operator::RequestHeadersBytes, Category::DOWNSTREAM_REQUEST);
    trie_mapstr!(trie, "RESPONSE_HEADERS_BYTES", Operator::ResponseHeadersBytes, Category::DOWNSTREAM_RESPONSE);
    trie_mapstr!(trie, "REQUESTED_SERVER_NAME", Operator::RequestedServerName, Category::DOWNSTREAM_REQUEST);
    trie_mapstr!(trie, "ROUTE_NAME", Operator::RouteName, Category::UPSTREAM_CONTEXT);
    trie_mapstr!(trie, "UPSTREAM_PEER_URI_SAN", Operator::UpstreamPeerUriSan, Category::UNSUPPORTED);
    trie_mapstr!(trie, "UPSTREAM_PEER_DNS_SAN", Operator::UpstreamPeerDnsSan, Category::UNSUPPORTED);
    trie_mapstr!(trie, "UPSTREAM_PEER_IP_SAN", Operator::UpstreamPeerIpSan, Category::UNSUPPORTED);
    trie_mapstr!(trie, "UPSTREAM_LOCAL_URI_SAN", Operator::UpstreamLocalUriSan, Category::UNSUPPORTED);
    trie_mapstr!(trie, "UPSTREAM_LOCAL_DNS_SAN", Operator::UpstreamLocalDnsSan, Category::UNSUPPORTED);
    trie_mapstr!(trie, "UPSTREAM_LOCAL_IP_SAN", Operator::UpstreamLocalIpSan, Category::UNSUPPORTED);
    trie_mapstr!(trie, "DOWNSTREAM_PEER_URI_SAN", Operator::DownstreamPeerUriSan, Category::UNSUPPORTED);
    trie_mapstr!(trie, "DOWNSTREAM_PEER_DNS_SAN", Operator::DownstreamPeerDnsSan, Category::UNSUPPORTED);
    trie_mapstr!(trie, "DOWNSTREAM_PEER_IP_SAN", Operator::DownstreamPeerIpSan, Category::UNSUPPORTED);
    trie_mapstr!(trie, "DOWNSTREAM_PEER_EMAIL_SAN", Operator::DownstreamPeerEmailSan, Category::UNSUPPORTED);
    trie_mapstr!(trie, "DOWNSTREAM_PEER_OTHERNAME_SAN", Operator::DownstreamPeerOthernameSan, Category::UNSUPPORTED);
    trie_mapstr!(trie, "DOWNSTREAM_LOCAL_URI_SAN", Operator::DownstreamLocalUriSan, Category::UNSUPPORTED);
    trie_mapstr!(trie, "DOWNSTREAM_LOCAL_DNS_SAN", Operator::DownstreamLocalDnsSan, Category::UNSUPPORTED);
    trie_mapstr!(trie, "DOWNSTREAM_LOCAL_IP_SAN", Operator::DownstreamLocalIpSan, Category::UNSUPPORTED);
    trie_mapstr!(trie, "DOWNSTREAM_LOCAL_EMAIL_SAN", Operator::DownstreamLocalEmailSan, Category::UNSUPPORTED);
    trie_mapstr!(trie, "DOWNSTREAM_LOCAL_OTHERNAME_SAN", Operator::DownstreamLocalOthernameSan, Category::UNSUPPORTED);
    trie_mapstr!(trie, "DOWNSTREAM_PEER_SUBJECT", Operator::DownstreamPeerSubject, Category::UNSUPPORTED);
    trie_mapstr!(trie, "DOWNSTREAM_LOCAL_SUBJECT", Operator::DownstreamLocalSubject, Category::UNSUPPORTED);
    trie_mapstr!(trie, "DOWNSTREAM_TLS_SESSION_ID", Operator::DownstreamTlsSessionId, Category::UNSUPPORTED);
    trie_mapstr!(trie, "DOWNSTREAM_TLS_CIPHER", Operator::DownstreamTlsCipher, Category::UNSUPPORTED);
    trie_mapstr!(trie, "DOWNSTREAM_TLS_VERSION", Operator::DownstreamTlsVersion, Category::UNSUPPORTED);
    trie_mapstr!(
        trie,
        "DOWNSTREAM_PEER_FINGERPRINT_256",
        Operator::DownstreamPeerFingerprint256,
        Category::UNSUPPORTED
    );
    trie_mapstr!(trie, "DOWNSTREAM_PEER_FINGERPRINT_1", Operator::DownstreamPeerFingerprint1, Category::UNSUPPORTED);
    trie_mapstr!(trie, "DOWNSTREAM_PEER_SERIAL", Operator::DownstreamPeerSerial, Category::UNSUPPORTED);
    trie_mapstr!(
        trie,
        "DOWNSTREAM_PEER_CHAIN_FINGERPRINTS_256",
        Operator::DownstreamPeerChainFingerprints256,
        Category::UNSUPPORTED
    );
    trie_mapstr!(
        trie,
        "DOWNSTREAM_PEER_CHAIN_FINGERPRINTS_1",
        Operator::DownstreamPeerChainFingerprints1,
        Category::UNSUPPORTED
    );
    trie_mapstr!(trie, "DOWNSTREAM_PEER_CHAIN_SERIALS", Operator::DownstreamPeerChainSerials, Category::UNSUPPORTED);
    trie_mapstr!(trie, "DOWNSTREAM_PEER_ISSUER", Operator::DownstreamPeerIssuer, Category::UNSUPPORTED);
    trie_mapstr!(trie, "DOWNSTREAM_PEER_CERT", Operator::DownstreamPeerCert, Category::UNSUPPORTED);
    trie_mapstr!(
        trie,
        "DOWNSTREAM_TRANSPORT_FAILURE_REASON",
        Operator::DownstreamTransportFailureReason,
        Category::UNSUPPORTED
    );
    trie_mapstr!(
        trie,
        "UPSTREAM_TRANSPORT_FAILURE_REASON",
        Operator::UpstreamTransportFailureReason,
        Category::FINISH_CONTEXT
    );
    trie_mapstr!(trie, "HOSTNAME", Operator::Hostname, Category::UNSUPPORTED);
    trie_mapstr!(trie, "FILTER_CHAIN_NAME", Operator::FilterChainName, Category::UNSUPPORTED);
    trie_mapstr!(trie, "VIRTUAL_CLUSTER_NAME", Operator::VirtualClusterName, Category::UNSUPPORTED);
    trie_mapstr!(trie, "TLS_JA3_FINGERPRINT", Operator::TlsJa3Fingerprint, Category::UNSUPPORTED);
    trie_mapstr!(trie, "UNIQUE_ID", Operator::UniqueId, Category::UPSTREAM_REQUEST);
    trie_mapstr!(trie, "TRACE_ID", Operator::TraceId, Category::DOWNSTREAM_REQUEST);
    trie_mapstr!(trie, "STREAM_ID", Operator::StreamId, Category::UNSUPPORTED);
    trie_mapstr!(trie, "START_TIME", Operator::StartTime, Category::INIT_CONTEXT);
    trie_mapstr!(trie, "START_TIME_LOCAL", Operator::StartTimeLocal, Category::UNSUPPORTED);
    trie_mapstr!(trie, "EMIT_TIME", Operator::EmitTime, Category::UNSUPPORTED);
    trie_mapstr!(trie, "EMIT_TIME_LOCAL", Operator::EmitTimeLocal, Category::UNSUPPORTED);
    trie_mapstr!(trie, "DYNAMIC_METADATA", Operator::DynamicMetadata, Category::UNSUPPORTED);
    trie_mapstr!(trie, "CLUSTER_METADATA", Operator::ClusterMetadata, Category::UNSUPPORTED);
    trie_mapstr!(trie, "UPSTREAM_METADATA", Operator::UpstreamMetadata, Category::UNSUPPORTED);
    trie_mapstr!(trie, "FILTER_STATE", Operator::FilterState, Category::UNSUPPORTED);
    trie_mapstr!(trie, "UPSTREAM_FILTER_STATE", Operator::UpstreamFilterState, Category::UNSUPPORTED);
    trie_mapstr!(trie, "DOWNSTREAM_PEER_CERT_V_START", Operator::DownstreamPeerCertVStart, Category::UNSUPPORTED);
    trie_mapstr!(trie, "DOWNSTREAM_PEER_CERT_V_END", Operator::DownstreamPeerCertVEnd, Category::UNSUPPORTED);
    trie_mapstr!(trie, "UPSTREAM_PEER_CERT_V_START", Operator::UpstreamPeerCertVStart, Category::UNSUPPORTED);
    trie_mapstr!(trie, "UPSTREAM_PEER_CERT_V_END", Operator::UpstreamPeerCertVEnd, Category::UNSUPPORTED);
    trie_mapstr!(trie, "ENVIRONMENT", Operator::Environment, Category::UNSUPPORTED);
    trie_mapstr!(
        trie,
        "UPSTREAM_CONNECTION_POOL_READY_DURATION",
        Operator::UpstreamConnectionPoolReadyDuration,
        Category::UNSUPPORTED
    );
    trie_mapstr!(
        trie,
        "REQ",
        Operator::Request(HeaderName(SmolStr::new_static("name"))),
        Category::DOWNSTREAM_REQUEST,
        true
    ); // %REQ(USER-AGENT)%
    trie_mapstr!(
        trie,
        "RESP",
        Operator::Response(HeaderName(SmolStr::new_static("name"))),
        Category::DOWNSTREAM_RESPONSE,
        true
    ); // %RESP(X-ENVOY-UPSTREAM-SERVICE-TIME)%
    trie
});

pub struct EnvoyGrammar;

impl EnvoyGrammar {
    fn parse_request(arg: &str) -> Result<ReqArgument, FormatError> {
        if let Some((t, _, _, _)) = ENVOY_REQ_ARGS.find_longest_prefix(arg.bytes()) {
            Ok(t.clone())
        } else {
            let valid_name =
                http::HeaderName::from_bytes(arg.as_bytes()).map_err(|_| FormatError::InvalidRequestArg(arg.into()))?;
            Ok(ReqArgument::Header(HeaderName(valid_name.as_str().into())))
        }
    }

    fn parse_response(arg: &str) -> Result<RespArgument, FormatError> {
        if let Some((t, _, _, _)) = ENVOY_RESP_ARGS.find_longest_prefix(arg.bytes()) {
            Ok(t.clone())
        } else {
            let valid_name = http::HeaderName::from_bytes(arg.as_bytes())
                .map_err(|_| FormatError::InvalidResponseArg(arg.into()))?;
            Ok(RespArgument::Header(HeaderName(valid_name.as_str().into())))
        }
    }

    fn extract_operator_arg(input: &str) -> Result<(&str, usize), FormatError> {
        if let Some(rest) = input.strip_prefix('(') {
            if let Some(end) = rest.find(')') {
                let arg = &rest[..end];
                if arg.is_empty() {
                    return Err(FormatError::EmptyArgument(input.into()));
                }
                let total_len = end + 2; // '(' + arg.len() + ')'
                return Ok((arg, total_len));
            }
        }
        Err(FormatError::MissingBracket(input.into()))
    }
}

impl Grammar for EnvoyGrammar {
    #[allow(clippy::too_many_lines)]
    fn parse(input: &str) -> Result<Vec<Template>, FormatError> {
        let mut parts = Vec::new();
        let mut literal_start = 0;
        let mut i = 0;

        while i < input.len() {
            let mut longest_placeholder: Option<(Operator, Category, usize)> = None;
            let mut skip = None;

            // find the longest placeholder starting from the current index i
            //
            if input[i..].starts_with('%') {
                let remainder = &input[i + 1..];
                if remainder.starts_with('%') {
                    skip = Some(2);
                } else if let Some((placeholder, category, placeholder_len, has_arg)) =
                    ENVOY_PATTERNS.find_longest_prefix(remainder.bytes())
                {
                    // ensure the placeholder is properly closed with a '%'
                    //

                    let operator = match remainder.find([' ', '%', '(']) {
                        Some(index) => &remainder[..index], // ...return a slice from the beginning to that index.
                        None => remainder,                  // In that case, return the whole string slice.
                    };

                    if operator.len() != *placeholder_len {
                        return Err(FormatError::InvalidOperator(
                            remainder.split_once('%').map(|(operator, _)| operator).unwrap_or(remainder).into(),
                        ));
                    }

                    let after_placeholder = &remainder[*placeholder_len..];
                    // placeholder found
                    if *has_arg {
                        let (arg_value, arg_len) = Self::extract_operator_arg(after_placeholder)?;

                        if longest_placeholder.as_ref().is_none_or(|(_, _, len)| *placeholder_len > *len) {
                            match placeholder {
                                Operator::Request(_) => {
                                    let arg = Self::parse_request(arg_value)?;
                                    longest_placeholder = match arg {
                                        ReqArgument::Scheme => {
                                            Some((Operator::RequestScheme, *category, *placeholder_len))
                                        },
                                        ReqArgument::Method => {
                                            Some((Operator::RequestMethod, *category, *placeholder_len))
                                        },
                                        ReqArgument::Path => Some((Operator::RequestPath, *category, *placeholder_len)),
                                        ReqArgument::OriginalPathOrPath => {
                                            Some((Operator::RequestOriginalPathOrPath, *category, *placeholder_len))
                                        },
                                        ReqArgument::Authority => {
                                            Some((Operator::RequestAuthority, *category, *placeholder_len))
                                        },
                                        ReqArgument::Header(header_name) => {
                                            Some((Operator::Request(header_name), *category, *placeholder_len))
                                        },
                                    }
                                },
                                Operator::Response(_) => {
                                    let arg = Self::parse_response(arg_value)?;
                                    longest_placeholder = match arg {
                                        RespArgument::Status => {
                                            Some((Operator::ResponseStatus, *category, *placeholder_len))
                                        },
                                        RespArgument::Header(header_name) => {
                                            Some((Operator::Response(header_name), *category, *placeholder_len))
                                        },
                                    }
                                },
                                _ => (),
                            }
                        }

                        if !after_placeholder[arg_len..].starts_with('%') {
                            return Err(FormatError::MissingDelimiter(remainder[..*placeholder_len].into()));
                        }

                        skip = Some(2 + *placeholder_len + arg_len);
                    } else {
                        longest_placeholder = Some((placeholder.clone(), *category, *placeholder_len));
                        if !after_placeholder.starts_with('%') {
                            return Err(FormatError::MissingDelimiter(remainder[..*placeholder_len].into()));
                        }
                        skip = Some(2 + *placeholder_len);
                    }
                } else {
                    return Err(FormatError::InvalidOperator(
                        remainder.split_once('%').map(|(operator, _)| operator).unwrap_or(remainder).into(),
                    ));
                }
            }

            if let Some(placeholder) = longest_placeholder.as_ref() {
                // placeholder found
                if i > literal_start {
                    let literal_text = &input[literal_start..i];
                    if literal_text.chars().count() == 1 {
                        // if the literal is a single char, push it as a char
                        if let Some(c) = literal_text.chars().next() {
                            parts.push(Template::Char(c));
                        } else {
                            // This case should not happen, but just in case
                            parts.push(Template::Literal(literal_text.into()));
                        }
                    } else {
                        parts.push(Template::Literal(literal_text.into()));
                    }
                }

                // Add this placeholder.
                parts.push(Template::Placeholder(
                    // input[i..i + skip.unwrap()].into() <- this is original placeholder
                    placeholder.0.clone(),
                    placeholder.1,
                ));

                // advance the index beyond the current placeholder and possibly its argument.
                i += skip.unwrap_or(0);

                literal_start = i;
            } else {
                /* skip the specified number of bytes, or by default the next char */
                i += skip.unwrap_or(input[i..].chars().next().map(char::len_utf8).unwrap_or(0));
            }
        }

        // if there's some text remaining, it's a literal
        if i > literal_start {
            let literal_text = &input[literal_start..i].replace("%%", "%");
            if literal_text.chars().count() == 1 {
                if let Some(c) = literal_text.chars().next() {
                    // if the literal is a single char, push it as a char
                    parts.push(Template::Char(c));
                } else {
                    // This case should not happen, but just in case
                    parts.push(Template::Literal(literal_text.into()));
                }
            } else {
                parts.push(Template::Literal(literal_text.into()));
            }
        }

        Ok(parts)
    }
}

// Unit tests module
#[cfg(test)]
mod tests {
    use crate::{DEFAULT_ACCESS_LOG_FORMAT, DEFAULT_ISTIO_2_ACCESS_LOG_FORMAT, DEFAULT_ISTIO_ACCESS_LOG_FORMAT};

    use super::*;

    #[test]
    fn test_parse_only_literals() {
        let input = "This is a plain literal string.";
        let expected = vec![Template::Literal("This is a plain literal string.".into())];
        let actual = EnvoyGrammar::parse(input).unwrap();
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_parse_only_placeholders() {
        let input = "%START_TIME%%PROTOCOL%";
        let expected = vec![
            Template::Placeholder(Operator::StartTime, Category::INIT_CONTEXT),
            Template::Placeholder(Operator::Protocol, Category::DOWNSTREAM_REQUEST),
        ];

        let actual = EnvoyGrammar::parse(input).unwrap();
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_parse_mixed_literal_and_placeholders() {
        let input = "Start %REQ(:METHOD)% middle %PROTOCOL% end.";
        let expected = vec![
            Template::Literal("Start ".into()),
            Template::Placeholder(Operator::RequestMethod, Category::DOWNSTREAM_REQUEST),
            Template::Literal(" middle ".into()),
            Template::Placeholder(Operator::Protocol, Category::DOWNSTREAM_REQUEST),
            Template::Literal(" end.".into()),
        ];
        let actual = EnvoyGrammar::parse(input).unwrap();
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_parse_starts_with_placeholder() {
        let input = "%START_TIME% literal after.";
        let expected = vec![
            Template::Placeholder(Operator::StartTime, Category::INIT_CONTEXT),
            Template::Literal(" literal after.".into()),
        ];
        let actual = EnvoyGrammar::parse(input).unwrap();
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_parse_ends_with_placeholder() {
        let input = "Literal before %PROTOCOL%";
        let expected = vec![
            Template::Literal("Literal before ".into()),
            Template::Placeholder(Operator::Protocol, Category::DOWNSTREAM_REQUEST),
        ];
        let actual = EnvoyGrammar::parse(input).unwrap();
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_parse_empty_string() {
        let input = "";
        let expected: Vec<Template> = vec![]; // Expect an empty vector
        let actual = EnvoyGrammar::parse(input).unwrap();
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_parse_with_special_chars_in_literal() {
        let input = "Literal with \"quotes\" and %%percent signs%% not placeholders.";
        let expected = vec![Template::Literal("Literal with \"quotes\" and %percent signs% not placeholders.".into())];
        let actual = EnvoyGrammar::parse(input).unwrap();
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_parse_complex_envoy_string() {
        let input = r#"[%START_TIME%] "%REQ(:METHOD)% %REQ(:PATH)% %PROTOCOL%""#;
        let expected = vec![
            Template::Char('['),
            Template::Placeholder(Operator::StartTime, Category::INIT_CONTEXT),
            Template::Literal("] \"".into()),
            Template::Placeholder(Operator::RequestMethod, Category::DOWNSTREAM_REQUEST),
            Template::Char(' '),
            Template::Placeholder(Operator::RequestPath, Category::DOWNSTREAM_REQUEST),
            Template::Char(' '),
            Template::Placeholder(Operator::Protocol, Category::DOWNSTREAM_REQUEST),
            Template::Char('"'),
        ];
        let actual = EnvoyGrammar::parse(input).unwrap();
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_parse_request_header_bytes() {
        let input = "%REQUEST_HEADERS_BYTES%";
        let actual = EnvoyGrammar::parse(input).unwrap();
        let expected = vec![Template::Placeholder(Operator::RequestHeadersBytes, Category::DOWNSTREAM_REQUEST)];
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_parse_response_header_bytes() {
        let input = "%RESPONSE_HEADERS_BYTES%";
        let actual = EnvoyGrammar::parse(input).unwrap();
        let expected = vec![Template::Placeholder(Operator::ResponseHeadersBytes, Category::DOWNSTREAM_RESPONSE)];
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_default_fmt() {
        _ = EnvoyGrammar::parse(DEFAULT_ACCESS_LOG_FORMAT).unwrap();
    }

    #[test]
    fn test_istio_fmt() {
        _ = EnvoyGrammar::parse(DEFAULT_ISTIO_ACCESS_LOG_FORMAT).unwrap();
        _ = EnvoyGrammar::parse(DEFAULT_ISTIO_2_ACCESS_LOG_FORMAT).unwrap();
    }

    // bad patters..

    #[test]
    fn test_parse_unsupported_operator() {
        let input = "%UNSUPPORTED%";
        let result = EnvoyGrammar::parse(input);
        assert!(matches!(result, Err(FormatError::InvalidOperator(_))));
    }

    #[test]
    fn test_parse_unsupported_operator_with_partial_match() {
        let input = "%RESPONSE_CODE_UNSUPPORTED%";
        let result = EnvoyGrammar::parse(input);
        assert!(matches!(result, Err(FormatError::InvalidOperator(_))));
    }

    #[test]
    fn test_parse_error_empty_argument() {
        let input = "%REQ()%";
        let result = EnvoyGrammar::parse(input);
        assert!(matches!(result, Err(FormatError::EmptyArgument(_))));
    }

    #[test]
    fn test_parse_error_missing_bracket_1() {
        let input = "%REQ";
        let result = EnvoyGrammar::parse(input);
        assert!(matches!(result, Err(FormatError::MissingBracket(_))));
    }

    #[test]
    fn test_parse_error_missing_bracket_2() {
        let input = "%REQ(USER_AGENT%";
        let result = EnvoyGrammar::parse(input);
        assert!(matches!(result, Err(FormatError::MissingBracket(_))));
    }

    #[test]
    fn test_parse_error_missing_delimiter_1() {
        let input = "%UPSTREAM_HOST";
        let result = EnvoyGrammar::parse(input);
        assert!(matches!(result, Err(FormatError::MissingDelimiter(_))));
    }

    #[test]
    fn test_parse_error_missing_delimiter_2() {
        let input = "%REQ(USER_AGENT)";
        let result = EnvoyGrammar::parse(input);
        assert!(matches!(result, Err(FormatError::MissingDelimiter(_))));
    }

    #[test]
    fn test_parse_error_missing_delimiter_3() {
        let input = "%REQ(USER_AGENT) ";
        let result = EnvoyGrammar::parse(input);
        assert!(matches!(result, Err(FormatError::MissingDelimiter(_))));
    }

    #[test]
    fn test_parse_invalid_req_argument() {
        let input = "%REQ(<BAD>)%";
        let result = EnvoyGrammar::parse(input);
        assert!(matches!(result, Err(FormatError::InvalidRequestArg(_))));
    }

    #[test]
    fn test_parse_invalid_resp_argument() {
        let input = "%RESP(<BAD>)%";
        let result = EnvoyGrammar::parse(input);
        assert!(matches!(result, Err(FormatError::InvalidResponseArg(_))));
    }
}
