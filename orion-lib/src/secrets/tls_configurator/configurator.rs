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
#![allow(clippy::into_iter_on_ref)]
use super::tls_configurator_builder::{WantsToBuildClient, WantsToBuildServer};
use crate::{
    secrets::{
        secrets_manager::CertStore,
        tls_configurator::tls_configurator_builder::{SecretHolder, TlsContextBuilder},
        CertificateSecret, TransportSecret,
    },
    Result, SecretManager,
};
use compact_str::CompactString;
use orion_configuration::config::{
    cluster::{TlsConfig as TlsClientConfig, TlsSecret},
    listener::TlsConfig as TlsServerConfig,
    secret::TlsCertificate as TlsCertificateConfig,
    transport::{CommonTlsValidationContext, Secrets, TlsVersion},
};
use rustls::{
    client::danger::ServerCertVerifier,
    crypto::KeyProvider,
    pki_types::{CertificateDer, PrivateKeyDer},
    version::{TLS12, TLS13},
    ClientConfig, RootCertStore, ServerConfig,
};
use rustls_platform_verifier::Verifier;
use std::{collections::HashMap, result::Result as StdResult, sync::Arc};
use tracing::{debug, warn};
use webpki::types::ServerName;

pub fn get_crypto_key_provider() -> Result<&'static dyn KeyProvider> {
    rustls::crypto::CryptoProvider::get_default()
        .map(|p| p.key_provider)
        .ok_or("Unable to get rustls crypto provider".into())
}

#[allow(dead_code)]
#[derive(Debug)]
struct IgnoreCertVerifier(Verifier);

impl ServerCertVerifier for IgnoreCertVerifier {
    fn verify_server_cert(
        &self,
        _: &rustls::pki_types::CertificateDer<'_>,
        _: &[rustls::pki_types::CertificateDer<'_>],
        _: &rustls::pki_types::ServerName<'_>,
        _: &[u8],
        _: rustls::pki_types::UnixTime,
    ) -> StdResult<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> StdResult<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        self.0.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> StdResult<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        self.0.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.supported_verify_schemes()
    }
}

#[derive(Debug, Clone)]
pub struct ClientCert {
    pub key: Arc<PrivateKeyDer<'static>>,
    pub certs: Arc<Vec<CertificateDer<'static>>>,
}

impl From<CertificateSecret> for ClientCert {
    fn from(secret: CertificateSecret) -> Self {
        let CertificateSecret { name: _, key, certs } = secret;
        ClientCert { key, certs }
    }
}

impl TryFrom<TransportSecret> for Arc<RootCertStore> {
    type Error = crate::Error;
    fn try_from(value: TransportSecret) -> Result<Self> {
        match value {
            TransportSecret::ValidationContext(context) => {
                let cert_store = context.as_ref().clone();
                Ok(cert_store.into())
            },
            TransportSecret::Certificate(_) => {
                Err("TransportSecret certificate is not supported for root cert store".into())
            },
        }
    }
}

impl TryFrom<TransportSecret> for ServerCert {
    type Error = crate::Error;
    fn try_from(value: TransportSecret) -> Result<Self> {
        match value {
            TransportSecret::Certificate(certificate) => {
                let certificate = certificate.as_ref().clone();
                ServerCert::try_from(certificate)
            },
            TransportSecret::ValidationContext(_) => {
                Err("TransportSecret ValidationContext is not supported for server certificate".into())
            },
        }
    }
}

impl TryFrom<TransportSecret> for ClientCert {
    type Error = crate::Error;
    fn try_from(value: TransportSecret) -> Result<Self> {
        match value {
            TransportSecret::Certificate(certificate) => {
                let certificate = certificate.as_ref().clone();
                Ok(ClientCert::from(certificate))
            },
            TransportSecret::ValidationContext(_) => {
                Err("TransportSecret ValidationContext is not supported for client certificate".into())
            },
        }
    }
}

impl TryFrom<CertificateSecret> for ServerCert {
    type Error = crate::Error;
    fn try_from(secret: CertificateSecret) -> Result<Self> {
        let CertificateSecret { name, key, certs } = secret;
        if let Some(name) = name {
            Ok(ServerCert { name, key: Arc::new(key.clone_key()), certs })
        } else {
            Err("secret doesn't contain server name".into())
        }
    }
}

impl TryFrom<Vec<TlsCertificateConfig>> for ClientCert {
    type Error = crate::Error;

    fn try_from(mut tls_certificates: Vec<TlsCertificateConfig>) -> Result<Self> {
        if tls_certificates.len() > 1 {
            return Err("Only one client certificate should be configured".into());
        }
        let certificate = tls_certificates.remove(0);
        ClientCert::try_from(certificate)
    }
}

impl TryFrom<TlsCertificateConfig> for ClientCert {
    type Error = crate::Error;

    fn try_from(certificate: TlsCertificateConfig) -> Result<Self> {
        let secret = CertificateSecret::try_from(&certificate)?;
        Ok(ClientCert::from(secret))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerCert {
    pub name: CompactString,
    pub key: Arc<PrivateKeyDer<'static>>,
    pub certs: Arc<Vec<CertificateDer<'static>>>,
}

#[derive(Debug, Clone)]
pub struct TlsConfigurator<S: Clone, CtxType> {
    context_builder: TlsContextBuilder<CtxType>,
    config: Arc<S>,
}

impl TlsConfigurator<ClientConfig, WantsToBuildClient> {
    pub fn update(self, secret_id: &str, secret: TransportSecret) -> Result<Self> {
        let TlsContextBuilder { state } = self.context_builder;
        let WantsToBuildClient {
            supported_versions,
            validation_context_secret_id,
            certificate_store,
            certificate_secret_id,
            client_certificate,
            sni,
        } = state;
        let new_builder = match secret {
            TransportSecret::Certificate(certificate) => {
                if Some(secret_id.to_owned()) == certificate_secret_id {
                    let client_cert: ClientCert = (*certificate).clone().into();
                    TlsContextBuilder::with_supported_versions(supported_versions)
                        .with_client_certificate_store(validation_context_secret_id, certificate_store)
                        .with_client_certificate(certificate_secret_id, Arc::new(client_cert))
                        .with_sni(sni)
                } else {
                    let msg = format!("Secret name doesn't match {secret_id}  {:?}", certificate_secret_id.as_deref());
                    warn!("{msg}");
                    return Err(msg.into());
                }
            },
            TransportSecret::ValidationContext(cert_store) => {
                if Some(secret_id.to_owned()) == validation_context_secret_id {
                    let cert_store = (*cert_store).clone().into();
                    let builder = TlsContextBuilder::with_supported_versions(supported_versions)
                        .with_client_certificate_store(validation_context_secret_id, cert_store);
                    if let Some(client_certificate) = client_certificate {
                        builder.with_client_certificate(certificate_secret_id, client_certificate)
                    } else {
                        builder.with_no_client_auth()
                    }
                    .with_sni(sni)
                } else {
                    let msg = format!("Secret name doesn't match {secret_id} {validation_context_secret_id:?}",);
                    warn!("{msg}");
                    return Err(msg.into());
                }
            },
        };

        let new_config = new_builder.build()?;
        Ok(TlsConfigurator { context_builder: new_builder, config: Arc::new(new_config) })
    }
}

impl TlsConfigurator<ServerConfig, WantsToBuildServer> {
    pub fn update(self, secret_id: &str, secret: TransportSecret) -> Result<Self> {
        let TlsContextBuilder { state } = self.context_builder;
        let WantsToBuildServer {
            supported_versions,
            validation_context_secret_id,
            certificate_store,
            mut server_ids_and_certificates,
            require_client_cert,
        } = state;
        let new_builder = match secret {
            TransportSecret::Certificate(certificate) => {
                if let Some(secret) = server_ids_and_certificates.iter_mut().find(|s| s.name == secret_id) {
                    let server_cert: ServerCert = (*certificate).clone().try_into()?;
                    secret.server_cert = server_cert;

                    let builder = TlsContextBuilder::with_supported_versions(supported_versions);

                    if let Some(certificate_store) = certificate_store {
                        builder.with_server_certificate_store(validation_context_secret_id, certificate_store)
                    } else {
                        builder.with_no_client_auth()
                    }
                    .with_certificates(server_ids_and_certificates)
                    .with_client_authentication(require_client_cert)
                } else {
                    let msg = format!("Can't find secret {secret_id}");
                    debug!("{msg}");
                    return Err(msg.into());
                }
            },
            TransportSecret::ValidationContext(cert_store) => {
                if Some(secret_id.to_owned()) == validation_context_secret_id {
                    let cert_store = (*cert_store).clone().into();
                    TlsContextBuilder::with_supported_versions(supported_versions)
                        .with_server_certificate_store(validation_context_secret_id, cert_store)
                        .with_certificates(server_ids_and_certificates)
                        .with_client_authentication(require_client_cert)
                } else {
                    let msg = format!("Can't find secret {secret_id} {validation_context_secret_id:?}",);
                    debug!("{msg}");
                    return Err(msg.into());
                }
            },
        };

        let new_config = new_builder.build()?;
        Ok(TlsConfigurator { context_builder: new_builder, config: Arc::new(new_config) })
    }
}

impl TryFrom<(TlsServerConfig, &SecretManager)> for TlsConfigurator<ServerConfig, WantsToBuildServer> {
    type Error = crate::Error;
    fn try_from((config, secret_manager): (TlsServerConfig, &SecretManager)) -> StdResult<Self, Self::Error> {
        let require_client_cert = config.require_client_certificate;
        let common_context = config.common_tls_context;
        #[allow(clippy::into_iter_on_ref)]
        let supported_versions = common_context
            .parameters
            .supported_version()
            .into_iter()
            .map(|version| match version {
                TlsVersion::TLSv1_2 => &TLS12,
                TlsVersion::TLSv1_3 => &TLS13,
            })
            .collect();
        debug!("DownstreamTlsContext : Selected TLS versions {supported_versions:?}");
        let (certificate_store_secret_id, certificate_store) =
            TlsConfigurator::create_certificate_store(secret_manager, common_context.validation_context)?;

        let certs_and_secret_ids = match common_context.secrets {
            Secrets::Certificates(certs) => {
                let mut certs_and_secret_ids = vec![];
                for certificate in certs {
                    certs_and_secret_ids.push(SecretHolder::new(
                        CompactString::default(),
                        ServerCert::try_from(CertificateSecret::try_from(&certificate)?)?,
                    ));
                }
                certs_and_secret_ids
            },
            Secrets::SdsConfig(sds) => {
                let mut certs_and_secret_ids = vec![];
                for sds_config_name in sds {
                    if let Some(certificate) = secret_manager.get_certificate(&sds_config_name)? {
                        let server_cert: ServerCert = certificate.try_into()?;
                        let secret = SecretHolder::new(sds_config_name.clone(), server_cert);
                        if certs_and_secret_ids.contains(&secret) {
                            let msg = format!("DownstreamTlsContext : Duplicate secret name {}", &sds_config_name);
                            warn!("{msg}");
                            return Err(msg.into());
                        }
                        certs_and_secret_ids.push(secret);
                    }
                }
                certs_and_secret_ids
            },
        };

        let ctx_builder = TlsContextBuilder::with_supported_versions(supported_versions);
        let ctx_builder = if let Some(certificate_store) = certificate_store {
            ctx_builder.with_server_certificate_store(
                certificate_store_secret_id.map(CompactString::into_string),
                certificate_store,
            )
        } else {
            ctx_builder.with_no_client_auth()
        }
        .with_certificates(certs_and_secret_ids)
        .with_client_authentication(require_client_cert);

        let config = ctx_builder.build()?;

        Ok(TlsConfigurator::<ServerConfig, WantsToBuildServer> {
            context_builder: ctx_builder,
            config: Arc::new(config),
        })
    }
}

impl TryFrom<(TlsClientConfig, &SecretManager)> for TlsConfigurator<ClientConfig, WantsToBuildClient> {
    type Error = crate::Error;

    fn try_from((context, secret_manager): (TlsClientConfig, &SecretManager)) -> StdResult<Self, Self::Error> {
        let sni = context.sni;
        let supported_versions = context
            .parameters
            .supported_version()
            .into_iter()
            .map(|version| match version {
                TlsVersion::TLSv1_2 => &TLS12,
                TlsVersion::TLSv1_3 => &TLS13,
            })
            .collect();
        debug!("DownstreamTlsContext : Selected TLS versions {supported_versions:?}");
        let (certificate_store_secret_id, certificate_store) =
            TlsConfigurator::create_certificate_store(secret_manager, context.validation_context)?;

        let (secret_id, client_certificate) = match context.secret {
            Some(TlsSecret::Certificate(cert)) => (None, Some(ClientCert::try_from(cert)?)),
            Some(TlsSecret::SdsConfig(sds_config_name)) => {
                let cert = secret_manager.get_certificate(&sds_config_name)?.map(ClientCert::try_from).transpose()?;
                (Some(sds_config_name), cert)
            },
            None => (None, None),
        };

        let Some(certificate_store) = certificate_store else {
            return Err("UpstreamContext : no TLS validation options found".into());
        };
        let ctx_builder = TlsContextBuilder::with_supported_versions(supported_versions).with_client_certificate_store(
            certificate_store_secret_id.map(CompactString::into_string),
            certificate_store,
        );

        let ctx_builder = if let Some(client_certificate) = client_certificate {
            ctx_builder.with_client_certificate(secret_id.map(CompactString::into_string), Arc::new(client_certificate))
        } else {
            ctx_builder.with_no_client_auth()
        }
        .with_sni(sni.into_string());

        let config = ctx_builder.build()?;

        Ok(TlsConfigurator::<ClientConfig, WantsToBuildClient> {
            context_builder: ctx_builder,
            config: Arc::new(config),
        })
    }
}

impl TlsConfigurator<(), ()> {
    /// Create a certificate store from the provided configuration options
    ///
    /// Returns an sds secret id and a TLS certificate store. If no validation context
    /// is configured returns Ok((None, None))
    fn create_certificate_store(
        secret_manager: &SecretManager,
        validation_options: Option<CommonTlsValidationContext>,
    ) -> Result<(Option<CompactString>, Option<Arc<RootCertStore>>)> {
        match validation_options {
            Some(CommonTlsValidationContext::ValidationContext(validation_context)) => {
                Ok((None, Some(CertStore::try_from(&validation_context)?.into())))
            },
            Some(CommonTlsValidationContext::SdsConfig(sds_config_name)) => {
                if let Some(cert_store) = secret_manager.get_validation_context(&sds_config_name)? {
                    Ok((Some(sds_config_name.clone()), Some(cert_store.try_into()?)))
                } else {
                    Ok((Some(sds_config_name.clone()), Some(Arc::new(RootCertStore::empty()))))
                }
            },
            None => Ok((None, None)),
        }
    }
}

impl TlsConfigurator<ServerConfig, WantsToBuildServer> {
    pub fn into_inner(self) -> ServerConfig {
        (*self.config).clone()
    }
}

impl TlsConfigurator<ClientConfig, WantsToBuildClient> {
    pub fn into_inner(self) -> ClientConfig {
        (*self.config).clone()
    }
    pub fn sni(&self) -> String {
        self.context_builder.state.sni.clone()
    }
}

/// More relaxed version of ResolvesServerCertUsingSni
/// Allowing ServerName instead of DNSNames

#[derive(Debug)]
pub struct RelaxedResolvesServerCertUsingSni {
    by_name: HashMap<String, Arc<rustls::sign::CertifiedKey>>,
}

impl RelaxedResolvesServerCertUsingSni {
    pub fn new() -> Self {
        Self { by_name: HashMap::new() }
    }

    pub fn add(&mut self, name: &str, ck: rustls::sign::CertifiedKey) -> StdResult<(), rustls::Error> {
        let name = name.to_ascii_lowercase();
        let server_name =
            { ServerName::try_from(name).map_err(|_| rustls::Error::General("Bad Server/DNS name".into()))? };

        ck.end_entity_cert()
            .and_then(rustls::server::ParsedCertificate::try_from)
            .and_then(|cert| rustls::client::verify_server_name(&cert, &server_name))?;

        if let ServerName::DnsName(name) = server_name {
            self.by_name.insert(name.as_ref().to_owned(), Arc::new(ck));
        } else {
            warn!("Server name is not valid DNS name");
        }
        Ok(())
    }
}

impl rustls::server::ResolvesServerCert for RelaxedResolvesServerCertUsingSni {
    fn resolve(&self, client_hello: rustls::server::ClientHello) -> Option<Arc<rustls::sign::CertifiedKey>> {
        if let Some(name) = client_hello.server_name() {
            self.by_name.get(name).cloned()
        } else {
            // This kind of resolver requires SNI
            None
        }
    }
}
