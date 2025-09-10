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

use crate::config::core::DataSource;
use compact_str::CompactString;
use serde::{Deserialize, Serialize};
use std::fmt::Debug;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Secret {
    pub name: CompactString,
    #[serde(flatten)]
    pub kind: Type,
}

impl Secret {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn kind(&self) -> &Type {
        &self.kind
    }

    pub fn kind_mut(&mut self) -> &mut Type {
        &mut self.kind
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Type {
    TlsCertificate(TlsCertificate),
    ValidationContext(ValidationContext),
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub enum ValidationContext {
    TrustedCA(DataSource),
    None
}



#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct TlsCertificate {
    certificate_chain: DataSource,
    private_key: DataSource,
}

impl Debug for TlsCertificate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TlsCertificate")
            .field("certificate_chain", &self.certificate_chain)
            .field(
                "private_key",
                match self.private_key() {
                    DataSource::Path(p) => p,
                    DataSource::InlineBytes(_) | DataSource::InlineString(_) => &"[censored]",
                    DataSource::EnvironmentVariable(env) => env,
                },
            )
            .finish()
    }
}

impl TlsCertificate {
    pub fn certificate_chain(&self) -> &DataSource {
        &self.certificate_chain
    }

    pub fn private_key(&self) -> &DataSource {
        &self.private_key
    }

    pub fn private_key_mut(&mut self) -> &mut DataSource {
        &mut self.private_key
    }
}

#[cfg(feature = "envoy-conversions")]
mod envoy_conversions {
    #![allow(deprecated)]
    use super::{Secret, TlsCertificate, Type, ValidationContext};
    use crate::config::common::*;
    use compact_str::CompactString;
    use orion_data_plane_api::envoy_data_plane_api::envoy::extensions::transport_sockets::tls::v3::{
        secret::Type as EnvoyType, CertificateValidationContext as EnvoyCertificateValidationContext,
        Secret as EnvoySecret, TlsCertificate as EnvoyTlsCertificate,
    };

    impl TryFrom<EnvoySecret> for Secret {
        type Error = GenericError;
        fn try_from(envoy: EnvoySecret) -> Result<Self, Self::Error> {
            let EnvoySecret { name, r#type } = envoy;
            let name: CompactString = required!(name)?.into();
            let kind = convert_opt!(r#type, "type").with_name(name.clone())?;
            Ok(Self { name, kind })
        }
    }
    impl TryFrom<EnvoyType> for Type {
        type Error = GenericError;
        fn try_from(value: EnvoyType) -> Result<Self, Self::Error> {
            match value {
                EnvoyType::TlsCertificate(envoy) => Ok(Self::TlsCertificate(envoy.try_into()?)),
                EnvoyType::ValidationContext(envoy) => Ok(Self::ValidationContext(envoy.try_into()?)),
                EnvoyType::GenericSecret(_) => Err(GenericError::unsupported_variant("GenericSecret")),
                EnvoyType::SessionTicketKeys(_) => Err(GenericError::unsupported_variant("SessionTicketKeys")),
            }
        }
    }
    impl TryFrom<EnvoyTlsCertificate> for TlsCertificate {
        type Error = GenericError;
        fn try_from(envoy: EnvoyTlsCertificate) -> Result<Self, Self::Error> {
            let EnvoyTlsCertificate {
                certificate_chain,
                private_key,
                pkcs12,
                watched_directory,
                private_key_provider,
                password,
                ocsp_staple,
                signed_certificate_timestamp,
            } = envoy;
            unsupported_field!(
                // certificate_chain,
                // private_key,
                pkcs12,
                watched_directory,
                private_key_provider,
                password,
                ocsp_staple,
                signed_certificate_timestamp
            )?;
            let certificate_chain = convert_opt!(certificate_chain)?;
            let private_key = convert_opt!(private_key)?;
            Ok(Self { certificate_chain, private_key })
        }
    }

    impl TryFrom<EnvoyCertificateValidationContext> for ValidationContext {
        type Error = GenericError;
        fn try_from(envoy: EnvoyCertificateValidationContext) -> Result<Self, Self::Error> {
            let EnvoyCertificateValidationContext {
                trusted_ca,
                ca_certificate_provider_instance,
                watched_directory,
                verify_certificate_spki,
                verify_certificate_hash,
                match_typed_subject_alt_names,
                match_subject_alt_names:_,
                require_signed_certificate_timestamp,
                crl,
                allow_expired_certificate,
                trust_chain_verification,
                custom_validator_config,
                only_verify_leaf_cert_crl,
                max_verify_depth,
                system_root_certs,
            } = envoy;
            unsupported_field!(
                // trusted_ca,
                ca_certificate_provider_instance,
                watched_directory,
                verify_certificate_spki,
                verify_certificate_hash,
                match_typed_subject_alt_names,
                //match_subject_alt_names,
                require_signed_certificate_timestamp,
                crl,
                allow_expired_certificate,
                trust_chain_verification,
                custom_validator_config,
                only_verify_leaf_cert_crl,
                max_verify_depth,
                system_root_certs
            )?;
            //let trusted_ca = convert_opt!(trusted_ca)?;
            if let Some(trusted_ca) = trusted_ca{
                Ok(Self::TrustedCA(trusted_ca.try_into()?))
            }else{
                Ok(Self::None)
            }
             
        }
    }
}
