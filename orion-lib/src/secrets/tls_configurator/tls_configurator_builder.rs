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

use std::sync::Arc;

use compact_str::CompactString;
use rustls::{
    client::WebPkiServerVerifier, server::WebPkiClientVerifier, sign::CertifiedKey, ClientConfig, RootCertStore,
    ServerConfig, SupportedProtocolVersion,
};
use rustls_platform_verifier::Verifier;
use tracing::{debug, warn};

use super::configurator::{get_crypto_key_provider, ClientCert, RelaxedResolvesServerCertUsingSni, ServerCert};

#[derive(Debug, Clone)]
pub struct WantsCertStore {
    pub supported_versions: Vec<&'static SupportedProtocolVersion>,
}

#[derive(Debug, Clone)]
pub struct WantsServerCert {
    supported_versions: Vec<&'static SupportedProtocolVersion>,
    validation_context_secret_id: Option<String>,
    certificate_store: Option<Arc<RootCertStore>>,
}

#[derive(Debug, Clone)]
pub struct WantsClientCert {
    supported_versions: Vec<&'static SupportedProtocolVersion>,
    validation_context_secret_id: Option<String>,
    certificate_store: Arc<RootCertStore>,
}

#[derive(Debug, Clone)]
pub struct SecretHolder {
    pub name: CompactString,
    pub server_cert: ServerCert,
}

impl PartialEq for SecretHolder {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}

impl Eq for SecretHolder {}

impl PartialOrd for SecretHolder {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for SecretHolder {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.name.cmp(&other.name)
    }
}
impl SecretHolder {
    pub fn new(name: CompactString, server_cert: ServerCert) -> Self {
        Self { name, server_cert }
    }
}

#[derive(Debug, Clone)]
pub struct WantsToBuildServer {
    pub supported_versions: Vec<&'static SupportedProtocolVersion>,
    pub validation_context_secret_id: Option<String>,
    pub certificate_store: Option<Arc<RootCertStore>>,
    pub server_ids_and_certificates: Vec<SecretHolder>,
    pub require_client_cert: bool,
}

#[derive(Debug, Clone)]
pub struct WantsToVerifyClientCert {
    supported_versions: Vec<&'static SupportedProtocolVersion>,
    validation_context_secret_id: Option<String>,
    certificate_store: Option<Arc<RootCertStore>>,
    server_ids_and_certificates: Vec<SecretHolder>,
}

#[derive(Debug, Clone)]
pub struct WantsSni {
    supported_versions: Vec<&'static SupportedProtocolVersion>,
    validation_context_secret_id: Option<String>,
    certificate_store: Arc<RootCertStore>,
    certificate_secret_id: Option<String>,
    client_certificate: Option<Arc<ClientCert>>,
}

#[derive(Debug, Clone)]
pub struct WantsToBuildClient {
    pub supported_versions: Vec<&'static SupportedProtocolVersion>,
    pub validation_context_secret_id: Option<String>,
    pub certificate_store: Arc<RootCertStore>,
    pub certificate_secret_id: Option<String>,
    pub client_certificate: Option<Arc<ClientCert>>,
    pub sni: String,
}

#[derive(Debug, Clone)]
pub struct TlsContextBuilder<S> {
    pub state: S,
}

use crate::{secrets::tls_configurator::configurator::IgnoreCertVerifier, Result};

impl TlsContextBuilder<()> {
    pub fn with_supported_versions(
        supported_versions: Vec<&'static SupportedProtocolVersion>,
    ) -> TlsContextBuilder<WantsCertStore> {
        TlsContextBuilder { state: WantsCertStore { supported_versions } }
    }
}

impl TlsContextBuilder<WantsCertStore> {
    pub fn with_server_certificate_store(
        self,
        secret_id: Option<String>,
        certificate_store: Arc<RootCertStore>,
    ) -> TlsContextBuilder<WantsServerCert> {
        let state = WantsServerCert {
            supported_versions: self.state.supported_versions,
            validation_context_secret_id: secret_id,
            certificate_store: Some(certificate_store),
        };
        TlsContextBuilder { state }
    }

    pub fn with_no_client_auth(self) -> TlsContextBuilder<WantsServerCert> {
        let state = WantsServerCert {
            supported_versions: self.state.supported_versions,
            validation_context_secret_id: None,
            certificate_store: None,
        };
        TlsContextBuilder { state }
    }

    pub fn with_client_certificate_store(
        self,
        secret_id: Option<String>,
        certificate_store: Arc<RootCertStore>,
    ) -> TlsContextBuilder<WantsClientCert> {
        let state = WantsClientCert {
            supported_versions: self.state.supported_versions,
            validation_context_secret_id: secret_id,
            certificate_store,
        };
        TlsContextBuilder { state }
    }
}

impl TlsContextBuilder<WantsServerCert> {
    pub fn with_certificates(
        self,
        server_ids_and_certificates: Vec<SecretHolder>,
    ) -> TlsContextBuilder<WantsToVerifyClientCert> {
        TlsContextBuilder {
            state: WantsToVerifyClientCert {
                supported_versions: self.state.supported_versions,
                validation_context_secret_id: self.state.validation_context_secret_id,
                certificate_store: self.state.certificate_store,
                server_ids_and_certificates,
            },
        }
    }
}

impl TlsContextBuilder<WantsToVerifyClientCert> {
    pub fn with_client_authentication(self, require_client_cert: bool) -> TlsContextBuilder<WantsToBuildServer> {
        TlsContextBuilder {
            state: WantsToBuildServer {
                supported_versions: self.state.supported_versions,
                validation_context_secret_id: self.state.validation_context_secret_id,
                certificate_store: self.state.certificate_store,
                server_ids_and_certificates: self.state.server_ids_and_certificates,
                require_client_cert,
            },
        }
    }
}

impl TlsContextBuilder<WantsClientCert> {
    pub fn with_client_certificate(
        self,
        secret_id: Option<String>,
        client_certificate: Arc<ClientCert>,
    ) -> TlsContextBuilder<WantsSni> {
        TlsContextBuilder {
            state: WantsSni {
                supported_versions: self.state.supported_versions,
                certificate_store: self.state.certificate_store,
                validation_context_secret_id: self.state.validation_context_secret_id,
                client_certificate: Some(client_certificate),
                certificate_secret_id: secret_id,
            },
        }
    }
    pub fn with_no_client_auth(self) -> TlsContextBuilder<WantsSni> {
        TlsContextBuilder {
            state: WantsSni {
                supported_versions: self.state.supported_versions,
                certificate_store: self.state.certificate_store,
                validation_context_secret_id: self.state.validation_context_secret_id,
                client_certificate: None,
                certificate_secret_id: None,
            },
        }
    }
}

impl TlsContextBuilder<WantsSni> {
    pub fn with_sni(self, sni: String) -> TlsContextBuilder<WantsToBuildClient> {
        TlsContextBuilder {
            state: WantsToBuildClient {
                supported_versions: self.state.supported_versions,
                certificate_store: self.state.certificate_store,
                validation_context_secret_id: self.state.validation_context_secret_id,
                client_certificate: self.state.client_certificate,
                certificate_secret_id: self.state.certificate_secret_id,
                sni,
            },
        }
    }
}

impl TlsContextBuilder<WantsToBuildServer> {
    pub fn build(&self) -> Result<ServerConfig> {
        let builder = ServerConfig::builder_with_protocol_versions(&self.state.supported_versions.clone());
        warn!("TLS Server Context Builder {} {:?}",self.state.require_client_cert, self.state.certificate_store.as_ref().map(|s| s.len()));
        let verifier = match (self.state.require_client_cert, &self.state.certificate_store) {
            (true, None) => {
                return Err("requireClientCertificate is true but no validation_context is configured".into());
            },
            (true, Some(certificate_store)) => {
                warn!("Certificate store is empty and require_client_cert is true");
                if certificate_store.is_empty(){
                    None                    
                }else{
                    Some(WebPkiClientVerifier::builder(Arc::clone(certificate_store)).build()?)
                }
            },
            (false, Some(certificate_store)) => {
                warn!("Certificate store is empty and require_client_cert is false");
                if certificate_store.is_empty(){
                    None
                }else{
                    Some(WebPkiClientVerifier::builder(Arc::clone(certificate_store)).allow_unauthenticated().build()?)
                }
            },
            (false, None) => None,
        };

        let builder = if let Some(verifier) = verifier {
            builder.with_client_cert_verifier(verifier)
        } else {
            builder.with_no_client_auth()
        };
        let provider = get_crypto_key_provider()?;

        if let [SecretHolder { name: _, server_cert: ServerCert { certs, key, name: _ } }] =
            self.state.server_ids_and_certificates.as_slice()
        {
            // If only a single certificate exists, do not install SNI resolver, just accept all
            // connections using the provided certificate
            return Ok(builder.with_single_cert(certs.to_vec(), key.clone_key())?);
        }

        let mut resolver = RelaxedResolvesServerCertUsingSni::new();
        let errors = self
            .state
            .server_ids_and_certificates
            .iter()
            .map(|SecretHolder { name: secret_name, server_cert: ServerCert { certs, key, name } }| {
                provider
                    .load_private_key(key.clone_key())
                    .map(|private_key| {
                        let certs = (**certs).clone();
                        (secret_name, name, CertifiedKey::new(certs, private_key))
                    })
                    .map_err(|e| format!("UpstreamContext: Can't load private key {secret_name} {name} - {e}").into())
                    .inspect_err(|e| warn!("{e}"))
            })
            .filter_map(Result::ok)
            .filter_map(|(secret_name, name, ck)| {
                resolver
                    .add(name, ck)
                    .inspect_err(|e| {
                        warn!("UpstreamContext: Can't add certificate for secret '{secret_name}' {name} - {e}");
                    })
                    .err()
            })
            .count();
        if errors > 0 {
            Err(format!("Found {errors} errors in Tls context").into())
        } else {
            Ok(builder.with_cert_resolver(Arc::new(resolver)))
        }
    }
}

impl TlsContextBuilder<WantsToBuildClient> {
    pub fn build(&self) -> Result<ClientConfig> {
        let builder = ClientConfig::builder_with_protocol_versions(&self.state.supported_versions.clone());
        warn!("TLS Client Context Builder {}",self.state.certificate_store.len());
        let builder =  if self.state.certificate_store.is_empty(){
            let verifier = WebPkiServerVerifier::builder(Arc::clone(&self.state.certificate_store)).build()?;
            builder.with_webpki_verifier(verifier)
        }else{            
            warn!("TLS Client Context Builder using dangerous configuration to ignore server certificates");
            builder.dangerous().with_custom_certificate_verifier(Arc::new(IgnoreCertVerifier(Verifier::new())))
        };
        

        if let Some(ClientCert { key, certs: auth_certs }) = self.state.client_certificate.as_deref() {
            debug!("UpstreamContext :  Selected Client Cert");
            let certs: Vec<webpki::types::CertificateDer<'_>> = auth_certs.as_ref().clone();
            Ok(builder.with_client_auth_cert(certs, key.clone_key())?)
        } else {
            Ok(builder.with_no_client_auth())
        }
    }
}
