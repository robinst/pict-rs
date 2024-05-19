use std::path::PathBuf;

use rustls::{crypto::aws_lc_rs::sign::any_supported_type, sign::CertifiedKey, Error};

pub(super) struct Tls {
    certificate: PathBuf,
    private_key: PathBuf,
}

#[derive(Debug, thiserror::Error)]
enum TlsError {
    #[error("Failed to read file")]
    Io(#[from] std::io::Error),

    #[error("Failed to sign certificate")]
    Sign(#[from] Error),

    #[error("No certificates found in certificate file")]
    MissingCerts,

    #[error("No key found in private key file")]
    MissingKey,
}

impl Tls {
    pub(super) fn from_config(config: &crate::config::Configuration) -> Option<Self> {
        config
            .server
            .certificate
            .as_ref()
            .zip(config.server.private_key.as_ref())
            .map(|(cert, key)| Tls {
                certificate: cert.clone(),
                private_key: key.clone(),
            })
    }

    pub(super) async fn open_keys(&self) -> color_eyre::Result<CertifiedKey> {
        let cert_bytes = tokio::fs::read(&self.certificate)
            .await
            .map_err(TlsError::from)?;

        let certs = rustls_pemfile::certs(&mut cert_bytes.as_slice())
            .collect::<Result<Vec<_>, _>>()
            .map_err(TlsError::from)?;

        if certs.is_empty() {
            return Err(TlsError::MissingCerts.into());
        }

        let key_bytes = tokio::fs::read(&self.private_key)
            .await
            .map_err(TlsError::from)?;

        let private_key = rustls_pemfile::private_key(&mut key_bytes.as_slice())
            .map_err(TlsError::from)?
            .ok_or(TlsError::MissingKey)?;

        let private_key = any_supported_type(&private_key).map_err(TlsError::from)?;

        Ok(CertifiedKey::new(certs, private_key))
    }
}
