use std::io;
use std::path::PathBuf;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use serde::Deserialize;
use url::Url;

mod defaults {
    pub const DEFAULT_CONNECT_TIMEOUT_SEC: u64 = 60;
    pub const DEFAULT_REQUEST_TIMEOUT_SEC: u64 = 600;
}
use defaults::*;

/// Configuration for client service needed for
/// orchestrator to communicate with it
#[derive(Debug, Clone)]
pub struct ServiceConfig<'a> {
    /// Hostname for service
    pub hostname: String,
    /// Port for service
    pub port: u16,
    /// Timeout in seconds for request to be handled
    pub request_timeout: u64,
    ///
    pub connection_timeout: u64,
    /// TLS provider info
    pub tls: Option<TlsConfig<'a>>,
}

impl ServiceConfig<'_> {
    fn from_parts(
        hostname: String,
        port: u16,
        request_timeout: Option<u64>,
        connection_timeout: Option<u64>,
        tls: Option<TlsConfig>,
    ) -> Self {
        Self {
            hostname,
            port,
            request_timeout: request_timeout.unwrap_or(DEFAULT_REQUEST_TIMEOUT_SEC),
            connection_timeout: connection_timeout.unwrap_or(DEFAULT_CONNECT_TIMEOUT_SEC),
            tls,
        }
    }

    pub fn base_url(&self) -> Url {
        let protocol = match self.tls {
            Some(_) => "https",
            None => "http",
        };
        let mut base_url = Url::parse(&format!("{}://{}", protocol, &self.hostname))
            .unwrap_or_else(|e| panic!("error parsing base url: {}", e));
        base_url
            .set_port(Some(self.port))
            .unwrap_or_else(|_| panic!("error setting port: {}", self.port));
        base_url
    }
}

pub struct ServiceConfigBuilder<'a>(ServiceConfig<'a>);

impl ServiceConfigBuilder<'_> {
    pub fn new(hostname: String, port: u16) -> Self {
        Self(ServiceConfig::from_parts(hostname, port, None, None, None))
    }

    pub fn with_request_timeout(self, request_timeout: u64) -> Self {
        Self(ServiceConfig {
            hostname: self.0.hostname.clone(),
            port: self.0.port,
            request_timeout,
            connection_timeout: self.0.connection_timeout,
            tls: self.0.tls,
        })
    }

    pub fn with_connection_timeout(self, connection_timeout: u64) -> Self {
        Self(ServiceConfig {
            hostname: self.0.hostname.clone(),
            port: self.0.port,
            request_timeout: self.0.request_timeout,
            connection_timeout,
            tls: self.0.tls,
        })
    }

    pub fn with_tls(self, tls: TlsConfig) -> Self {
        Self(ServiceConfig {
            hostname: self.0.hostname.clone(),
            port: self.0.port,
            request_timeout: self.0.request_timeout,
            connection_timeout: self.0.connection_timeout,
            tls: Some(tls),
        })
    }
}

impl<'a> ServiceConfigBuilder<'a> {
        pub fn build(self) -> ServiceConfig<'a> {
        self.0
    }
}

/// Client TLS configuration
#[cfg_attr(test, derive(Default))]
#[derive(Clone, Debug, Deserialize)]
pub struct TlsConfigBuilder {
    pub cert_path: PathBuf,
    pub key_path: Option<PathBuf>,
    pub ca_cert_path: Option<PathBuf>,
    pub insecure: Option<bool>,
}

#[derive(Debug)]
pub struct TlsConfig<'a> {
    pub cert: Vec<CertificateDer<'a>>,
    pub key: Option<PrivateKeyDer<'a>>,
    pub ca_cert: Option<Vec<CertificateDer<'a>>>,
    pub insecure: bool,
}

impl Clone for TlsConfig<'_> {
    fn clone(&self) -> Self {
        let key = match &self.key {
            Some(key) => Some(key.clone_key()),
            None => None,
        };
        Self {
            cert: self.cert.clone(),
            key,
            ca_cert: self.ca_cert.clone(),
            insecure: self.insecure,
        }
    }
}

impl TlsConfigBuilder {
    pub fn from_parts(
        cert_path: PathBuf,
        key_path: Option<PathBuf>,
        ca_cert_path: Option<PathBuf>,
        insecure: Option<bool>,
    ) -> Self {
        Self {
            cert_path,
            key_path,
            ca_cert_path,
            insecure,
        }
    }

    pub async fn build<'a>(self) -> Result<TlsConfig<'a>, Error> {
        let buf = io::BufReader::new(self.cert_path);
        let cert = rustls_pemfile::certs(&buf).collect::<Result<Vec<_>, _>>()?;

        let key = self.key_path.map(|path| {
            let buf = io::BufReader::new(path);
            rustls_pemfile::private_key(&buf)??
        });

        let ca_cert = self.ca_cert_path.map(|path| {
            let buf = io::BufReader::new(path);
            rustls_pemfile::certs(&buf).collect::<Result<Vec<_>, _>>()?
        });

        Ok(TlsConfig { cert, key, ca_cert, insecure: self.insecure.unwrap_or(false) })
    }
}


pub enum Error {

}
