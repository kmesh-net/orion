use crate::operator::{Category, Operator};
use crate::{FormatError, Grammar, Template};
use http::HeaderName;
use lazy_static::lazy_static;
use ptrie::Trie;

macro_rules! trie_mapstr {
    ($trie:expr, $lit:literal, $tok:expr, $cat:expr) => {
        $trie.insert($lit.bytes(), ($tok, $cat, $lit.len(), false)); // expand the variadic arguments into the tuple.
    };
    ($trie:expr, $lit:literal, $tok:expr, $cat:expr, $arg:expr ) => {
        $trie.insert($lit.bytes(), ($tok, $cat, $lit.len(), $arg)); // expand the variadic arguments into the tuple.
    };
}

#[allow(clippy::non_std_lazy_statics)]
lazy_static! {
    static ref ENVOY_REQ_ARGS: Trie<u8, (Operator, Category, usize, bool)> = {
        let mut trie = Trie::new();
        trie_mapstr!(trie, ":SCHEME",Operator::RequestScheme, Category::Argument);
        trie_mapstr!(trie, ":METHOD",Operator::RequestMethod, Category::Argument);
        trie_mapstr!(trie, ":PATH", Operator::RequestPath, Category::Argument);
        trie_mapstr!(trie, ":AUTHORITY", Operator::RequestAuthority, Category::Argument);
        trie_mapstr!(trie, "X-ENVOY-ORIGINAL-PATH?:PATH", Operator::RequestOriginalPathOrPath, Category::Argument);
        trie
    };

    static ref ENVOY_RESP_ARGS: Trie<u8, (Operator, Category, usize, bool)> = {
        Trie::new()
    };

    static ref ENVOY_PATTERNS: Trie<u8, (Operator, Category, usize, bool)> = {
        let mut trie = Trie::new();
        // trie_mapstr!(trie, "REQUEST_DURATION", Token::RequestDuration);
        // trie_mapstr!(trie, "REQUEST_TX_DURATION", Token::RequestTxDuration);
        // trie_mapstr!(trie, "RESPONSE_DURATION", Token::ResponseDuration);
        // trie_mapstr!(trie, "RESPONSE_TX_DURATION", Token::ResponseTxDuration);
        // trie_mapstr!(trie, "DOWNSTREAM_HANDSHAKE_DURATION", Token::DownstreamHandshakeDuration);
        // trie_mapstr!(trie, "ROUNDTRIP_DURATION", Token::RoundtripDuration);
        trie_mapstr!(trie, "BYTES_RECEIVED", Operator::BytesReceived, Category::FinishContext);
        // trie_mapstr!(trie, "BYTES_RETRANSMITTED", Token::BytesRetransmitted);
        // trie_mapstr!(trie, "PACKETS_RETRANSMITTED", Token::PacketsRetransmitted);
        // trie_mapstr!(trie, "UPSTREAM_WIRE_BYTES_RECEIVED", Token::UpstreamWireBytesReceived);
        // trie_mapstr!(trie, "UPSTREAM_HEADER_BYTES_RECEIVED", Token::UpstreamHeaderBytesReceived);
        // trie_mapstr!(trie, "DOWNSTREAM_WIRE_BYTES_RECEIVED", Token::DownstreamWireBytesReceived);
        // trie_mapstr!(trie, "DOWNSTREAM_HEADER_BYTES_RECEIVED", Token::DownstreamHeaderBytesReceived);
        trie_mapstr!(trie, "PROTOCOL", Operator::Protocol, Category::DownstreamRequest);
        trie_mapstr!(trie, "UPSTREAM_PROTOCOL", Operator::UpstreamProtocol, Category::UpstreamRequest);
        trie_mapstr!(trie, "RESPONSE_CODE", Operator::ResponseCode, Category::DownstreamResponse);
        // trie_mapstr!(trie, "RESPONSE_CODE_DETAILS", Token::ResponseCodeDetails);
        // trie_mapstr!(trie, "CONNECTION_TERMINATION_DETAILS", Token::ConnectionTerminationDetails);
        trie_mapstr!(trie, "BYTES_SENT", Operator::BytesSent, Category::FinishContext);
        // trie_mapstr!(trie, "UPSTREAM_WIRE_BYTES_SENT", Token::UpstreamWireBytesSent);
        // trie_mapstr!(trie, "UPSTREAM_HEADER_BYTES_SENT", Token::UpstreamHeaderBytesSent);
        // trie_mapstr!(trie, "DOWNSTREAM_WIRE_BYTES_SENT", Token::DownstreamWireBytesSent);
        // trie_mapstr!(trie, "DOWNSTREAM_HEADER_BYTES_SENT", Token::DownstreamHeaderBytesSent);
        trie_mapstr!(trie, "DURATION", Operator::Duration, Category::FinishContext);
        // trie_mapstr!(trie, "COMMON_DURATION", Token::CommonDuration);
        // trie_mapstr!(trie, "CUSTOM_FLAGS", Token::CustomFlags);
        trie_mapstr!(trie, "RESPONSE_FLAGS", Operator::ResponseFlags, Category::FinishContext);
        // trie_mapstr!(trie, "RESPONSE_FLAGS_LONG", Token::ResponseFlagsLong);
        // trie_mapstr!(trie, "UPSTREAM_HOST_NAME", Token::UpstreamHostName);
        // trie_mapstr!(trie, "UPSTREAM_HOST_NAME_WITHOUT_PORT", Token::UpstreamHostNameWithoutPort);
        trie_mapstr!(trie, "UPSTREAM_HOST", Operator::UpstreamHost, Category::UpstreamContext);
        // trie_mapstr!(trie, "UPSTREAM_CONNECTION_ID", Token::UpstreamConnectionId);
        // trie_mapstr!(trie, "UPSTREAM_CLUSTER", Token::UpstreamCluster);
        // trie_mapstr!(trie, "UPSTREAM_CLUSTER_RAW", Token::UpstreamClusterRaw);
        // trie_mapstr!(trie, "UPSTREAM_LOCAL_ADDRESS", Token::UpstreamLocalAddress);
        // trie_mapstr!(trie, "UPSTREAM_LOCAL_ADDRESS_WITHOUT_PORT", Token::UpstreamLocalAddressWithoutPort);
        // trie_mapstr!(trie, "UPSTREAM_LOCAL_PORT", Token::UpstreamLocalPort);
        // trie_mapstr!(trie, "UPSTREAM_REMOTE_ADDRESS", Token::UpstreamRemoteAddress);
        // trie_mapstr!(trie, "UPSTREAM_REMOTE_ADDRESS_WITHOUT_PORT", Token::UpstreamRemoteAddressWithoutPort);
        // trie_mapstr!(trie, "UPSTREAM_REMOTE_PORT", Token::UpstreamRemotePort);
        // trie_mapstr!(trie, "UPSTREAM_REQUEST_ATTEMPT_COUNT", Token::UpstreamRequestAttemptCount);
        // trie_mapstr!(trie, "UPSTREAM_TLS_CIPHER", Token::UpstreamTlsCipher);
        // trie_mapstr!(trie, "UPSTREAM_TLS_VERSION", Token::UpstreamTlsVersion);
        // trie_mapstr!(trie, "UPSTREAM_TLS_SESSION_ID", Token::UpstreamTlsSessionId);
        // trie_mapstr!(trie, "UPSTREAM_PEER_ISSUER", Token::UpstreamPeerIssuer);
        // trie_mapstr!(trie, "UPSTREAM_PEER_CERT", Token::UpstreamPeerCert);
        // trie_mapstr!(trie, "UPSTREAM_PEER_SUBJECT", Token::UpstreamPeerSubject);
        // trie_mapstr!(trie, "DOWNSTREAM_LOCAL_ADDRESS", Token::DownstreamLocalAddress);
        // trie_mapstr!(trie, "DOWNSTREAM_DIRECT_LOCAL_ADDRESS", Token::DownstreamDirectLocalAddress);
        // trie_mapstr!(trie, "DOWNSTREAM_LOCAL_ADDRESS_WITHOUT_PORT", Token::DownstreamLocalAddressWithoutPort);
        // trie_mapstr!(trie, "DOWNSTREAM_DIRECT_LOCAL_ADDRESS_WITHOUT_PORT", Token::DownstreamDirectLocalAddressWithoutPort);
        // trie_mapstr!(trie, "DOWNSTREAM_LOCAL_PORT", Token::DownstreamLocalPort);
        // trie_mapstr!(trie, "DOWNSTREAM_DIRECT_LOCAL_PORT", Token::DownstreamDirectLocalPort);
        // trie_mapstr!(trie, "DOWNSTREAM_REMOTE_ADDRESS", Token::DownstreamRemoteAddress);
        // trie_mapstr!(trie, "DOWNSTREAM_REMOTE_ADDRESS_WITHOUT_PORT", Token::DownstreamRemoteAddressWithoutPort);
        // trie_mapstr!(trie, "DOWNSTREAM_REMOTE_PORT", Token::DownstreamRemotePort);
        // trie_mapstr!(trie, "DOWNSTREAM_DIRECT_REMOTE_ADDRESS", Token::DownstreamDirectRemoteAddress);
        // trie_mapstr!(trie, "DOWNSTREAM_DIRECT_REMOTE_ADDRESS_WITHOUT_PORT", Token::DownstreamDirectRemoteAddressWithoutPort);
        // trie_mapstr!(trie, "DOWNSTREAM_DIRECT_REMOTE_PORT", Token::DownstreamDirectRemotePort);
        // trie_mapstr!(trie, "CONNECTION_ID", Token::ConnectionId);
        // trie_mapstr!(trie, "REQUESTED_SERVER_NAME", Token::RequestedServerName);
        // trie_mapstr!(trie, "ROUTE_NAME", Token::RouteName);
        // trie_mapstr!(trie, "UPSTREAM_PEER_URI_SAN", Token::UpstreamPeerUriSan);
        // trie_mapstr!(trie, "UPSTREAM_PEER_DNS_SAN", Token::UpstreamPeerDnsSan);
        // trie_mapstr!(trie, "UPSTREAM_PEER_IP_SAN", Token::UpstreamPeerIpSan);
        // trie_mapstr!(trie, "UPSTREAM_LOCAL_URI_SAN", Token::UpstreamLocalUriSan);
        // trie_mapstr!(trie, "UPSTREAM_LOCAL_DNS_SAN", Token::UpstreamLocalDnsSan);
        // trie_mapstr!(trie, "UPSTREAM_LOCAL_IP_SAN", Token::UpstreamLocalIpSan);
        // trie_mapstr!(trie, "DOWNSTREAM_PEER_URI_SAN", Token::DownstreamPeerUriSan);
        // trie_mapstr!(trie, "DOWNSTREAM_PEER_DNS_SAN", Token::DownstreamPeerDnsSan);
        // trie_mapstr!(trie, "DOWNSTREAM_PEER_IP_SAN", Token::DownstreamPeerIpSan);
        // trie_mapstr!(trie, "DOWNSTREAM_PEER_EMAIL_SAN", Token::DownstreamPeerEmailSan);
        // trie_mapstr!(trie, "DOWNSTREAM_PEER_OTHERNAME_SAN", Token::DownstreamPeerOthernameSan);
        // trie_mapstr!(trie, "DOWNSTREAM_LOCAL_URI_SAN", Token::DownstreamLocalUriSan);
        // trie_mapstr!(trie, "DOWNSTREAM_LOCAL_DNS_SAN", Token::DownstreamLocalDnsSan);
        // trie_mapstr!(trie, "DOWNSTREAM_LOCAL_IP_SAN", Token::DownstreamLocalIpSan);
        // trie_mapstr!(trie, "DOWNSTREAM_LOCAL_EMAIL_SAN", Token::DownstreamLocalEmailSan);
        // trie_mapstr!(trie, "DOWNSTREAM_LOCAL_OTHERNAME_SAN", Token::DownstreamLocalOthernameSan);
        // trie_mapstr!(trie, "DOWNSTREAM_PEER_SUBJECT", Token::DownstreamPeerSubject);
        // trie_mapstr!(trie, "DOWNSTREAM_LOCAL_SUBJECT", Token::DownstreamLocalSubject);
        // trie_mapstr!(trie, "DOWNSTREAM_TLS_SESSION_ID", Token::DownstreamTlsSessionId);
        // trie_mapstr!(trie, "DOWNSTREAM_TLS_CIPHER", Token::DownstreamTlsCipher);
        // trie_mapstr!(trie, "DOWNSTREAM_TLS_VERSION", Token::DownstreamTlsVersion);
        // trie_mapstr!(trie, "DOWNSTREAM_PEER_FINGERPRINT_256", Token::DownstreamPeerFingerprint256);
        // trie_mapstr!(trie, "DOWNSTREAM_PEER_FINGERPRINT_1", Token::DownstreamPeerFingerprint1);
        // trie_mapstr!(trie, "DOWNSTREAM_PEER_SERIAL", Token::DownstreamPeerSerial);
        // trie_mapstr!(trie, "DOWNSTREAM_PEER_CHAIN_FINGERPRINTS_256", Token::DownstreamPeerChainFingerprints256);
        // trie_mapstr!(trie, "DOWNSTREAM_PEER_CHAIN_FINGERPRINTS_1", Token::DownstreamPeerChainFingerprints1);
        // trie_mapstr!(trie, "DOWNSTREAM_PEER_CHAIN_SERIALS", Token::DownstreamPeerChainSerials);
        // trie_mapstr!(trie, "DOWNSTREAM_PEER_ISSUER", Token::DownstreamPeerIssuer);
        // trie_mapstr!(trie, "DOWNSTREAM_PEER_CERT", Token::DownstreamPeerCert);
        // trie_mapstr!(trie, "DOWNSTREAM_TRANSPORT_FAILURE_REASON", Token::DownstreamTransportFailureReason);
        // trie_mapstr!(trie, "UPSTREAM_TRANSPORT_FAILURE_REASON", Token::UpstreamTransportFailureReason);
        // trie_mapstr!(trie, "HOSTNAME", Token::Hostname);
        // trie_mapstr!(trie, "FILTER_CHAIN_NAME", Token::FilterChainName);
        // trie_mapstr!(trie, "VIRTUAL_CLUSTER_NAME", Token::VirtualClusterName);
        // trie_mapstr!(trie, "TLS_JA3_FINGERPRINT", Token::TlsJa3Fingerprint);
        // trie_mapstr!(trie, "UNIQUE_ID", Token::UniqueId);
        // trie_mapstr!(trie, "STREAM_ID", Token::StreamId);
        trie_mapstr!(trie, "START_TIME", Operator::StartTime, Category::InitContext);
        // trie_mapstr!(trie, "START_TIME_LOCAL", Token::StartTimeLocal);
        // trie_mapstr!(trie, "EMIT_TIME", Token::EmitTime);
        // trie_mapstr!(trie, "EMIT_TIME_LOCAL", Token::EmitTimeLocal);
        // trie_mapstr!(trie, "DYNAMIC_METADATA", Token::DynamicMetadata);
        // trie_mapstr!(trie, "CLUSTER_METADATA", Token::ClusterMetadata);
        // trie_mapstr!(trie, "UPSTREAM_METADATA", Token::UpstreamMetadata);
        // trie_mapstr!(trie, "FILTER_STATE", Token::FilterState);
        // trie_mapstr!(trie, "UPSTREAM_FILTER_STATE", Token::UpstreamFilterState);
        // trie_mapstr!(trie, "DOWNSTREAM_PEER_CERT_V_START", Token::DownstreamPeerCertVStart);
        // trie_mapstr!(trie, "DOWNSTREAM_PEER_CERT_V_END", Token::DownstreamPeerCertVEnd);
        // trie_mapstr!(trie, "UPSTREAM_PEER_CERT_V_START", Token::UpstreamPeerCertVStart);
        // trie_mapstr!(trie, "UPSTREAM_PEER_CERT_V_END", Token::UpstreamPeerCertVEnd);
        // trie_mapstr!(trie, "ENVIRONMENT", Token::Environment);
        // trie_mapstr!(trie, "UPSTREAM_CONNECTION_POOL_READY_DURATION", Token::UpstreamConnectionPoolReadyDuration);
        trie_mapstr!(trie, "REQ", Operator::Request, Category::DownstreamRequest, true); // %REQ(USER-AGENT)%
        trie_mapstr!(trie, "RESP", Operator::Response, Category::DownstreamResponse, true); // %RESP(X-ENVOY-UPSTREAM-SERVICE-TIME)%
        trie
    };
}

pub struct EnvoyGrammar;

impl EnvoyGrammar {
    fn parse_request(arg: &str) -> Result<Operator, FormatError> {
        if let Some((t, _, _, _)) = ENVOY_REQ_ARGS.find_longest_prefix(arg.bytes()) {
            Ok(t.clone())
        } else {
            let name =
                HeaderName::from_bytes(arg.as_bytes()).map_err(|_| FormatError::InvalidRequestArg(arg.into()))?;
            Ok(Operator::RequestStandard(name))
        }
    }

    fn parse_response(arg: &str) -> Result<Operator, FormatError> {
        if let Some((t, _, _, _)) = ENVOY_RESP_ARGS.find_longest_prefix(arg.bytes()) {
            Ok(t.clone())
        } else {
            let name =
                HeaderName::from_bytes(arg.as_bytes()).map_err(|_| FormatError::InvalidResponseArg(arg.into()))?;
            Ok(Operator::ResponseStandard(name))
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
                    let after_placeholder = &remainder[*placeholder_len..];
                    // placeholder found
                    if *has_arg {
                        let (arg_value, arg_len) = Self::extract_operator_arg(after_placeholder)?;

                        if longest_placeholder.as_ref().is_none_or(|(_, _, len)| *placeholder_len > *len) {
                            match placeholder {
                                Operator::Request => {
                                    let token = Self::parse_request(arg_value)?;
                                    longest_placeholder = Some((token, *category, *placeholder_len));
                                },
                                Operator::Response => {
                                    let token = Self::parse_response(arg_value)?;
                                    longest_placeholder = Some((token, *category, *placeholder_len));
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
    use crate::DEFAULT_ENVOY_FORMAT;

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
            Template::Placeholder(Operator::StartTime, Category::InitContext),
            Template::Placeholder(Operator::Protocol, Category::DownstreamRequest),
        ];
        let actual = EnvoyGrammar::parse(input).unwrap();
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_parse_mixed_literal_and_placeholders() {
        let input = "Start %REQ(:METHOD)% middle %PROTOCOL% end.";
        let expected = vec![
            Template::Literal("Start ".into()),
            Template::Placeholder(Operator::RequestMethod, Category::DownstreamRequest),
            Template::Literal(" middle ".into()),
            Template::Placeholder(Operator::Protocol, Category::DownstreamRequest),
            Template::Literal(" end.".into()),
        ];
        let actual = EnvoyGrammar::parse(input).unwrap();
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_parse_starts_with_placeholder() {
        let input = "%START_TIME% literal after.";
        let expected = vec![
            Template::Placeholder(Operator::StartTime, Category::InitContext),
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
            Template::Placeholder(Operator::Protocol, Category::DownstreamRequest),
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
            Template::Placeholder(Operator::StartTime, Category::InitContext),
            Template::Literal("] \"".into()),
            Template::Placeholder(Operator::RequestMethod, Category::DownstreamRequest),
            Template::Char(' '),
            Template::Placeholder(Operator::RequestPath, Category::DownstreamRequest),
            Template::Char(' '),
            Template::Placeholder(Operator::Protocol, Category::DownstreamRequest),
            Template::Char('"'),
        ];
        let actual = EnvoyGrammar::parse(input).unwrap();
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_default_fmt() {
        _ = EnvoyGrammar::parse(DEFAULT_ENVOY_FORMAT).unwrap();
    }

    // bad patters..

    #[test]
    fn test_parse_unsupported_operator() {
        let input = "%UNSUPPORTED%";
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
