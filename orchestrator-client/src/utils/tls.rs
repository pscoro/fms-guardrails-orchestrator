use std::sync::Arc;
use hyper_rustls::ConfigBuilderExt;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::{ClientConfig, DigitallySignedStruct, SignatureScheme};
use rustls_pki_types::{CertificateDer, ServerName, UnixTime};
use crate::config::TlsConfig;

#[derive(Debug)]
struct NoVerifier;

impl ServerCertVerifier for NoVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer,
        _intermediates: &[CertificateDer],
        _server_name: &ServerName,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![
            SignatureScheme::RSA_PKCS1_SHA1,
            SignatureScheme::ECDSA_SHA1_Legacy,
            SignatureScheme::RSA_PKCS1_SHA256,
            SignatureScheme::ECDSA_NISTP256_SHA256,
            SignatureScheme::RSA_PKCS1_SHA384,
            SignatureScheme::ECDSA_NISTP384_SHA384,
            SignatureScheme::RSA_PKCS1_SHA512,
            SignatureScheme::ECDSA_NISTP521_SHA512,
            SignatureScheme::RSA_PSS_SHA256,
            SignatureScheme::RSA_PSS_SHA384,
            SignatureScheme::RSA_PSS_SHA512,
            SignatureScheme::ED25519,
            SignatureScheme::ED448,
        ]
    }
}

pub fn build_insecure_client_config() -> rustls::ClientConfig {
    let mut config = ClientConfig::builder().with_native_roots()?.with_no_client_auth();
    config.dangerous().set_certificate_verifier(Arc::new(NoVerifier));
    config
}

pub fn build_client_config(tls_config: &TlsConfig) -> rustls::ClientConfig {
    let mut client_config_builder = rustls::ClientConfig::builder();

    let client_config_builder = match &tls_config.ca_cert {
        Some(ca_cert) if !ca_cert.is_empty() => {
            let mut root = rustls::RootCertStore::empty();
            ca_cert.clone().iter().for_each(
                |certs| { let (_, _) = root.add_parsable_certificates(certs); });
            client_config_builder.with_root_certificates(root)
        },
        _ => client_config_builder.with_native_roots()?,
    };
    let mut client_config = match &tls_config.key {
        Some(key) if !&tls_config.cert.is_empty() => client_config_builder.with_client_auth_cert(tls_config.cert.clone(), key.clone_key())?,
        _ => client_config_builder.with_no_client_auth(),
    };
    match &tls_config.insecure {
        true => client_config.dangerous().set_certificate_verifier(Arc::new(NoVerifier)),
        false => {},
    };
    client_config
}