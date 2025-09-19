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

use crate::config::common::*;
use base64::engine::general_purpose::STANDARD;
use base64_serde::base64_serde_type;
use compact_str::CompactString;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::{
    fmt::Debug,
    hash::{Hash, Hasher},
    io::{BufRead, BufReader, Read},
};
base64_serde_type!(Base64Standard, STANDARD);

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DataSource {
    Path(CompactString),
    InlineBytes(#[serde(with = "Base64Standard")] Vec<u8>),
    InlineString(CompactString),
    EnvironmentVariable(CompactString),
}

#[derive(thiserror::Error, Debug)]
pub enum DataSourceReadError {
    #[error("failed to read file \"{0}\"")]
    IoError(CompactString, #[source] std::io::Error),
    #[error("failed to read environment variable \"{0}\"")]
    EnvError(CompactString, #[source] std::env::VarError),
}

impl DataSource {
    pub fn to_bytes_blocking(&self) -> Result<Vec<u8>, DataSourceReadError> {
        match self {
            Self::InlineString(b) => Ok(b.as_bytes().to_owned()),
            Self::InlineBytes(b) => Ok(b.clone()),
            Self::Path(path) => std::fs::read(path).map_err(|e| DataSourceReadError::IoError(path.clone(), e)),
            Self::EnvironmentVariable(key) => {
                std::env::var(key).map(String::into_bytes).map_err(|e| DataSourceReadError::EnvError(key.clone(), e))
            },
        }
    }

    pub fn into_buf_read(&self) -> Result<DataSourceReader<'_>, DataSourceReadError> {
        DataSourceReader::new(self)
    }
}

pub enum DataSourceReader<'a> {
    Path(BufReader<std::fs::File>),
    InlineBytes(&'a [u8]),
    OwnedBytes { bytes: Box<[u8]>, read: usize },
}

impl<'a> DataSourceReader<'a> {
    pub fn new(inner: &'a DataSource) -> Result<Self, DataSourceReadError> {
        Ok(match inner {
            DataSource::EnvironmentVariable(_) => {
                let bytes = inner.to_bytes_blocking()?.into_boxed_slice();
                Self::OwnedBytes { bytes, read: 0 }
            },
            DataSource::InlineString(s) => Self::InlineBytes(s.as_bytes()),
            DataSource::InlineBytes(b) => Self::InlineBytes(b.as_slice()),
            DataSource::Path(p) => {
                let reader =
                    BufReader::new(std::fs::File::open(p).map_err(|e| DataSourceReadError::IoError(p.clone(), e))?);
                Self::Path(reader)
            },
        })
    }
}

impl Read for DataSourceReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::OwnedBytes { bytes, read } => {
                let avail_source = bytes.len() - *read;
                let avail_target = buf.len();
                let copied = avail_source.min(avail_target);
                buf[..copied].copy_from_slice(&bytes[*read..(*read + copied)]);
                *read += copied;
                Ok(copied)
            },
            Self::InlineBytes(b) => b.read(buf),
            Self::Path(reader) => reader.read(buf),
        }
    }
}

impl BufRead for DataSourceReader<'_> {
    fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
        match self {
            Self::OwnedBytes { bytes, read } => Ok(&bytes[*read..]),
            Self::InlineBytes(b) => b.fill_buf(),
            Self::Path(reader) => reader.fill_buf(),
        }
    }

    fn consume(&mut self, amt: usize) {
        match self {
            Self::OwnedBytes { bytes: _, read } => *read += amt,
            Self::InlineBytes(b) => b.consume(amt),
            Self::Path(reader) => reader.consume(amt),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, Hash)]
pub struct StringMatcher {
    // does not apply to regex
    // https://www.envoyproxy.io/docs/envoy/latest/api-v3/type/matcher/v3/string.proto#type-matcher-v3-stringmatcher
    #[serde(skip_serializing_if = "std::ops::Not::not", default = "Default::default")]
    pub ignore_case: bool,
    #[serde(flatten)]
    pub pattern: StringMatcherPattern,
}

pub(crate) struct CaseSensitive<'a>(pub bool, pub &'a str);
impl CaseSensitive<'_> {
    #[inline]
    pub fn equals(&self, b: &str) -> bool {
        if self.0 {
            self.1 == b
        } else {
            self.1.eq_ignore_ascii_case(b)
        }
    }

    #[inline]
    pub fn starts_with(&self, prefix: &str) -> bool {
        if self.0 {
            self.1.starts_with(prefix)
        } else {
            prefix.len() <= self.1.len() && prefix.eq_ignore_ascii_case(&self.1[..prefix.len()])
        }
    }

    #[inline]
    pub fn ends_with(&self, suffix: &str) -> bool {
        if self.0 {
            self.1.ends_with(suffix)
        } else {
            let slen = suffix.len();
            slen <= self.1.len() && suffix.eq_ignore_ascii_case(&self.1[self.1.len() - slen..])
        }
    }

    #[inline]
    pub fn find(&self, needle: &str) -> Option<usize> {
        if self.0 {
            self.1.find(needle)
        } else {
            if needle.len() <= self.1.len() {
                for i in 0..=(self.1.len() - needle.len()) {
                    if self.1[i..i + needle.len()].eq_ignore_ascii_case(needle) {
                        return Some(i);
                    }
                }
            }
            None
        }
    }

    #[inline]
    pub fn contains(&self, needle: &str) -> bool {
        self.find(needle).is_some()
    }
}

impl StringMatcher {
    pub fn matches(&self, to_match: &str) -> bool {
        let casematcher = CaseSensitive(!self.ignore_case, to_match);
        match &self.pattern {
            StringMatcherPattern::Exact(s) => casematcher.equals(s),
            StringMatcherPattern::Prefix(prefix) => casematcher.starts_with(prefix),
            StringMatcherPattern::Suffix(suffix) => casematcher.ends_with(suffix),
            StringMatcherPattern::Contains(needle) => casematcher.contains(needle),
            StringMatcherPattern::Regex(r) => r.matches_full(to_match),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StringMatcherPattern {
    Exact(CompactString),
    Prefix(CompactString),
    Suffix(CompactString),
    Contains(CompactString),
    Regex(#[serde(with = "serde_regex")] Regex),
}

impl PartialEq for StringMatcherPattern {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Regex(r1), Self::Regex(r2)) => r1.as_str().eq(r2.as_str()),
            (Self::Exact(s1), Self::Exact(s2))
            | (Self::Prefix(s1), Self::Prefix(s2))
            | (Self::Suffix(s1), Self::Suffix(s2))
            | (Self::Contains(s1), Self::Contains(s2)) => s1.eq(s2),
            _ => false,
        }
    }
}

impl Eq for StringMatcherPattern {}

impl Hash for StringMatcherPattern {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self {
            Self::Regex(r) => r.as_str().hash(state),
            Self::Exact(s) | Self::Prefix(s) | Self::Suffix(s) | Self::Contains(s) => s.hash(state),
        }
    }
}

#[cfg(feature = "envoy-conversions")]
pub(crate) use envoy_conversions::*;

#[cfg(feature = "envoy-conversions")]
pub mod envoy_conversions {
    #![allow(deprecated)]
    use super::{DataSource, StringMatcher, StringMatcherPattern};
    use crate::config::common::*;
    use http::uri::Authority;
    use ipnet::IpNet;
    use orion_data_plane_api::envoy_data_plane_api::envoy::{
        config::core::v3::{
            address::Address as EnvoyAddress, data_source::Specifier as EnvoySpecifier, socket_address::PortSpecifier,
            Address as EnvoyOuterAddress, CidrRange as EnvoyCidrRange, DataSource as EnvoyDataSource,
            Pipe as EnvoyPipe, SocketAddress as EnvoySocketAddress,
        },
        r#type::matcher::v3::{
            string_matcher::MatchPattern as EnvoyStringMatcherPattern, RegexMatcher as EnvoyRegexMatcher,
            StringMatcher as EnvoyStringMatcher,
        },
    };
    use regex::{Regex, RegexBuilder};
    use serde::{Deserialize, Serialize};
    use std::net::SocketAddr;

    pub struct CidrRange(IpNet);

    impl CidrRange {
        pub fn into_ipnet(self) -> IpNet {
            self.0
        }
    }

    #[derive(Debug, Eq, PartialEq, Serialize, Deserialize, Clone, Hash)]
    pub enum Address {
        Socket(String, u16),
        Pipe(String, u32),
    }

    impl Address {
        pub fn into_addr(self) -> Result<SocketAddr, GenericError> {
            #[allow(clippy::match_wildcard_for_single_variants)]
            match self {
                Self::Socket(address, port) => format!("{address}:{port}")
                    .parse()
                    .map_err(|e| {
                        GenericError::from_msg_with_cause(format!("failed to parse \"{address}\" as an ip adress"), e)
                    })
                    .with_node(address),
                _ => Err(GenericError::from_msg("only socket addresses are supported at the moment")),
            }
        }
    }

    impl std::fmt::Display for Address {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                Self::Socket(address, port) => f.write_str(&format!("{address}:{port}")),
                Self::Pipe(path, _) => f.write_str(path),
            }
        }
    }

    impl TryFrom<EnvoyCidrRange> for CidrRange {
        type Error = GenericError;
        fn try_from(value: EnvoyCidrRange) -> Result<Self, Self::Error> {
            let EnvoyCidrRange { address_prefix, prefix_len } = value;
            let address_prefix = address_prefix.parse::<std::net::IpAddr>().map_err(|e| {
                GenericError::from_msg_with_cause("failed to parse \"{address_prefix}\" as an ip adress", e)
                    .with_node("address_prefix")
            })?;
            // defaults to 0 when unset
            // https://www.envoyproxy.io/docs/envoy/latest/api-v3/config/core/v3/address.proto#envoy-v3-api-msg-config-core-v3-cidrrange
            let prefix_len = prefix_len.map(|v| v.value).unwrap_or(0);
            let prefix_len = u8::try_from(prefix_len).map_err(|_| {
                GenericError::from_msg(format!("failed to convert {prefix_len} to a u8")).with_node("prefix_len")
            })?;
            let ip_net = IpNet::new(address_prefix, prefix_len).map_err(|e| {
                GenericError::from_msg_with_cause(
                    format!(
                        "failed to make a cidr range from address_prefix {address_prefix} and prefix_len {prefix_len}"
                    ),
                    e,
                )
            })?;
            Ok(Self(ip_net))
        }
    }

    impl TryFrom<EnvoyOuterAddress> for Address {
        type Error = GenericError;
        fn try_from(value: EnvoyOuterAddress) -> Result<Self, Self::Error> {
            let EnvoyOuterAddress { address } = value;
            required!(address)?.try_into()
        }
    }

    impl TryFrom<EnvoyAddress> for Address {
        type Error = GenericError;
        fn try_from(value: EnvoyAddress) -> Result<Self, Self::Error> {
            match value {
                EnvoyAddress::SocketAddress(sock) => sock.try_into(),
                EnvoyAddress::Pipe(pipe) => pipe.try_into(),
                EnvoyAddress::EnvoyInternalAddress(_) => Err(GenericError::unsupported_variant("EnvoyInternalAddress")),
            }
        }
    }

    impl TryFrom<EnvoyPipe> for Address {
        type Error = GenericError;
        fn try_from(value: EnvoyPipe) -> Result<Self, Self::Error> {
            let EnvoyPipe { path, mode } = value;
            Ok(Address::Pipe(path, mode))
        }
    }

    impl TryFrom<&Authority> for Address {
        type Error = GenericError;
        fn try_from(value: &Authority) -> Result<Self, Self::Error> {
            let port =
                value.port_u16().ok_or(GenericError::from_msg(format!("Authority doesn't have port {value}")))?;
            Ok(Address::Socket(value.host().to_owned(), port))
        }
    }

    impl TryFrom<EnvoySocketAddress> for Address {
        type Error = GenericError;
        fn try_from(value: EnvoySocketAddress) -> Result<Self, Self::Error> {
            let EnvoySocketAddress {
                protocol,
                address,
                resolver_name,
                ipv4_compat,
                port_specifier,
                network_namespace_filepath,
            } = value;
            unsupported_field!(protocol, resolver_name, ipv4_compat, network_namespace_filepath)?;
            let address = required!(address)?;
            let port_specifier = match required!(port_specifier)? {
                PortSpecifier::NamedPort(_) => Err(GenericError::unsupported_variant("NamedPort")),
                PortSpecifier::PortValue(port) => Ok(port),
            }?;
            let port = u16::try_from(port_specifier).map_err(|_| {
                GenericError::from_msg(format!("failed to convert {port_specifier} to a port number"))
                    .with_node("port_specifier")
            })?;

            Ok(Address::Socket(address, port))
        }
    }
    impl TryFrom<EnvoyDataSource> for DataSource {
        type Error = GenericError;
        fn try_from(envoy: EnvoyDataSource) -> Result<Self, Self::Error> {
            let EnvoyDataSource { specifier, watched_directory } = envoy;
            unsupported_field!(watched_directory)?;
            let specifier = required!(specifier)?;
            Ok(match specifier {
                EnvoySpecifier::InlineBytes(b) => Self::InlineBytes(b),
                EnvoySpecifier::InlineString(s) => Self::InlineString(s.into()),
                EnvoySpecifier::Filename(filename) => Self::Path(filename.into()),
                EnvoySpecifier::EnvironmentVariable(var) => Self::EnvironmentVariable(var.into()),
            })
        }
    }
    impl TryFrom<EnvoyStringMatcher> for StringMatcher {
        type Error = GenericError;
        fn try_from(value: EnvoyStringMatcher) -> Result<Self, Self::Error> {
            let EnvoyStringMatcher { ignore_case, match_pattern } = value;
            let pattern = convert_opt!(match_pattern)?;
            Ok(Self { ignore_case, pattern })
        }
    }

    impl TryFrom<EnvoyStringMatcherPattern> for StringMatcherPattern {
        type Error = GenericError;
        fn try_from(value: EnvoyStringMatcherPattern) -> Result<Self, Self::Error> {
            match value {
                EnvoyStringMatcherPattern::Exact(s) => Ok(Self::Exact(s.into())),
                EnvoyStringMatcherPattern::Contains(s) => Ok(Self::Contains(s.into())),
                EnvoyStringMatcherPattern::Prefix(s) => Ok(Self::Prefix(s.into())),
                EnvoyStringMatcherPattern::Suffix(s) => Ok(Self::Suffix(s.into())),
                EnvoyStringMatcherPattern::SafeRegex(r) => Ok(Self::Regex(regex_from_envoy(r)?)),
                EnvoyStringMatcherPattern::Custom(_) => Err(GenericError::from_msg("Custom is not supported")),
            }
        }
    }

    pub fn regex_from_envoy(envoy: EnvoyRegexMatcher) -> Result<Regex, GenericError> {
        let EnvoyRegexMatcher { regex, engine_type } = envoy;
        unsupported_field!(engine_type)?;
        RegexBuilder::new(&regex)
            .build()
            .map_err(|e| GenericError::from_msg_with_cause(format!("failed to convert \"{regex}\" into a regex"), e))
    }
}
