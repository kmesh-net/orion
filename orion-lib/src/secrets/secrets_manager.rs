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

use crate::Result;
use compact_str::{CompactString, ToCompactString};
use orion_configuration::{
    config::secret::{Secret, TlsCertificate, Type, ValidationContext},
    VerifySingleIter,
};
use rustc_hash::FxHashMap as HashMap;
use rustls::{
    pki_types::{CertificateDer, PrivateKeyDer},
    RootCertStore,
};
use rustls_pemfile::{certs, pkcs8_private_keys};
use std::sync::Arc;
use tracing::{debug, warn};
use webpki::types::ServerName;
use x509_parser::{self, extensions::GeneralName};

#[derive(Clone, Debug)]
pub struct CertStore {
    pub store: Arc<RootCertStore>,
    pub config: ValidationContext,
}

impl From<CertStore> for Arc<RootCertStore> {
    fn from(value: CertStore) -> Self {
        value.store
    }
}

impl TryFrom<&ValidationContext> for CertStore {
    type Error = crate::Error;

    fn try_from(validation_context: &ValidationContext) -> Result<Self> {
        let mut ca_reader = validation_context.trusted_ca().into_buf_read()?;
        let mut root_store = rustls::RootCertStore::empty();
        let ca_certs = certs(&mut ca_reader)
            .map(|f| f.map_err(|e| format!("Can't parse certificate {e:?}").into()))
            .collect::<Result<Vec<_>>>()?;

        if ca_certs.is_empty() {
            return Err("No certificates have been configured".into());
        }

        let (good, bad) = root_store.add_parsable_certificates(ca_certs);
        debug!("Added certs {good} rejected certs {bad}");
        if bad > 0 {
            Err("Some certs in the trust store were invalid".into())
        } else {
            Ok(CertStore { store: Arc::new(root_store), config: validation_context.clone() })
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct SecretManager {
    certificate_secrets: HashMap<String, Arc<CertificateSecret>>,
    validation_contexts: HashMap<String, Arc<CertStore>>,
}

#[derive(Debug, Clone)]
pub struct CertificateSecret {
    pub name: Option<CompactString>,
    pub key: Arc<PrivateKeyDer<'static>>,
    pub certs: Arc<Vec<CertificateDer<'static>>>,
    pub config: TlsCertificate,
}

#[derive(Debug, Clone)]
pub enum TransportSecret {
    Certificate(Arc<CertificateSecret>),
    ValidationContext(Arc<CertStore>),
}

impl TryFrom<&TlsCertificate> for CertificateSecret {
    type Error = crate::Error;

    fn try_from(certificate: &TlsCertificate) -> Result<Self> {
        let mut cert_reader = certificate.certificate_chain().into_buf_read()?;
        let mut key_reader = certificate.private_key().into_buf_read()?;
        let key = pkcs8_private_keys(&mut key_reader)
            .map(|f| f.map_err(|e| format!("Can't parse private key: {e}")))
            .verify_single()??;

        let certificates = certs(&mut cert_reader)
            .map(|f| f.map_err(|e| format!("Can't parse certificate {e:?}").into()))
            .collect::<Result<Vec<_>>>()?;

        let Some(cert) = certificates.first() else {
            return Err("No certificates have been configured".into());
        };

        let mut server_name = None;
        let cloned_cert = cert.clone();
        let (_, x509_cert) = x509_parser::parse_x509_certificate(&cloned_cert)?;
        let subject = x509_cert.subject();
        if let Ok(Some(san)) = x509_cert.subject_alternative_name() {
            for san_name in &san.value.general_names {
                let name = match *san_name {
                    GeneralName::DNSName(name) => name.to_owned(),
                    _ => continue,
                };

                let is_server_name = ServerName::try_from(name.clone()).is_ok();
                debug!("Certificate SAN name {san_name} {name } is server name {is_server_name}");
                if is_server_name {
                    server_name = Some(name.to_compact_string());
                }
            }
        }
        let common_name = subject.iter_common_name().next().and_then(|cn| cn.as_str().ok());

        debug!("Certificate Subject's common name {common_name:?}");

        let key = Arc::new(PrivateKeyDer::Pkcs8(key));
        Ok(CertificateSecret { name: server_name, key, certs: Arc::new(certificates), config: certificate.clone() })
    }
}

impl SecretManager {
    pub fn new() -> Self {
        Self { certificate_secrets: HashMap::default(), validation_contexts: HashMap::default() }
    }

    pub fn add(&mut self, secret: &Secret) -> Result<TransportSecret> {
        let secret_id = secret.name();
        let secret = match secret.kind() {
            Type::TlsCertificate(certificate) => {
                let secret = Arc::new(CertificateSecret::try_from(certificate)?);
                let _old_value = self.certificate_secrets.insert(secret_id.to_owned(), Arc::clone(&secret));
                TransportSecret::Certificate(secret)
            },
            Type::ValidationContext(validation_context) => {
                let store = Arc::new(CertStore::try_from(validation_context)?);
                let _old_value = self.validation_contexts.insert(secret_id.to_owned(), Arc::clone(&store));
                TransportSecret::ValidationContext(store)
            },
        };
        Ok(secret)
    }
    pub fn remove(&mut self, secret_id: &str, secret_type: &Type) -> Result<()> {
        match secret_type {
            Type::TlsCertificate(_) => {
                let _old_value = self.certificate_secrets.remove(secret_id);
            },
            Type::ValidationContext(_) => {
                let _old_value = self.validation_contexts.remove(secret_id);
            },
        }
        Ok(())
    }

    pub fn get_certificate(&self, secret_id: &str) -> Result<Option<TransportSecret>> {
        let value = self.certificate_secrets.get(secret_id);
        if value.is_none() {
            warn!("SDS secret '{secret_id}' is missing");
        }
        Ok(value.map(|s| TransportSecret::Certificate(Arc::clone(s))))
    }
    pub fn get_validation_context(&self, secret_id: &str) -> Result<Option<TransportSecret>> {
        let value = self.validation_contexts.get(secret_id);
        Ok(value.map(|s| TransportSecret::ValidationContext(Arc::clone(s))))
    }
    pub fn get_all_secrets(&self) -> Vec<Secret> {
        let mut secrets = Vec::new();
        secrets.extend(
            self.certificate_secrets
                .iter()
                .map(|(name, secret)| Secret { name: name.into(), kind: Type::TlsCertificate(secret.config.clone()) }),
        );
        secrets.extend(
            self.validation_contexts.iter().map(|(name, secret)| Secret {
                name: name.into(),
                kind: Type::ValidationContext(secret.config.clone()),
            }),
        );
        secrets
    }
}
