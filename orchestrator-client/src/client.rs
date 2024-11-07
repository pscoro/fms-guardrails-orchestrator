use std::{
    any::{Any, TypeId},
    marker::PhantomData,
    pin::Pin,
    time::Duration,
};

use async_trait::async_trait;
use futures::Stream;
use ginepro::LoadBalancedChannel;
use hyper_rustls::{ConfigBuilderExt, HttpsConnector};
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use tracing::{debug, instrument};

use crate::{config::ServiceConfig, health::HealthCheckResult, utils};

mod error;
pub use error::Error;

pub mod http;
pub use http::HttpClientService;

pub mod grpc;
pub use grpc::GrpcClientService;

pub type BoxStream<T> = Pin<Box<dyn Stream<Item = T> + Send>>;
pub type BoxClient = Box<dyn Client>;

mod private {
    pub struct Seal;
}

#[derive(Debug, Clone, Copy)]
pub enum ClientKind {
    Http,
    Grpc,
}

#[async_trait]
pub trait Client: Send + Sync + 'static {
    /// Returns the name of the client type.
    fn name(&self) -> &str;

    /// Returns the `TypeId` of the client type. Sealed to prevent overrides.
    fn type_id(&self, _: private::Seal) -> TypeId {
        TypeId::of::<Self>()
    }

    /// Performs a client health check.
    async fn health(&self) -> HealthCheckResult;
}

impl dyn Client {
    pub fn is<T: 'static>(&self) -> bool {
        TypeId::of::<T>() == self.type_id(private::Seal)
    }

    pub fn downcast<T: 'static>(self: Box<Self>) -> Result<Box<T>, Box<Self>> {
        if (*self).is::<T>() {
            let ptr = Box::into_raw(self) as *mut T;
            // SAFETY: guaranteed by `is`
            unsafe { Ok(Box::from_raw(ptr)) }
        } else {
            Err(self)
        }
    }

    pub fn downcast_ref<T: 'static>(&self) -> Option<&T> {
        if (*self).is::<T>() {
            let ptr = self as *const dyn Client as *const T;
            // SAFETY: guaranteed by `is`
            unsafe { Some(&*ptr) }
        } else {
            None
        }
    }

    pub fn downcast_mut<T: 'static>(&mut self) -> Option<&mut T> {
        if (*self).is::<T>() {
            let ptr = self as *mut dyn Client as *mut T;
            // SAFETY: guaranteed by `is`
            unsafe { Some(&mut *ptr) }
        } else {
            None
        }
    }
}

#[derive(Debug)]
struct WithHttpService;

#[derive(Debug)]
struct WithGrpcService<G: tower::Service<grpc::Request>> {
    _phantom: PhantomData<G>,
}

#[derive(Debug)]
struct WithHealthService<K: Kind> {
    _phantom: PhantomData<K>,
}

#[derive(Debug)]
struct NoHealthService;

#[derive(Debug)]
struct MissingHttpService;

#[derive(Debug)]
struct MissingGrpcService;

#[derive(Debug)]
struct MissingHealthService;

#[derive(Debug, Default)]
pub struct ClientBuilder<R, T: tower::Service<R> + Clone + Send, U: tower::Service<R> + Clone + Send, S, H> {
    _request_type: PhantomData<R>,
    _service_state: PhantomData<S>,
    _health_service_state: PhantomData<H>,
    service: Option<T>,
    health_service: Option<U>,
}

pub trait Kind {
    fn type_id(_: private::Seal) -> TypeId {
        TypeId::of::<Self>()
    }
}

pub struct Http;
impl Kind for Http {}

pub struct Grpc;
impl Kind for Grpc {}

impl<'a, T: tower::Service<http::Request> + Clone + Send, U: tower::Service<http::Request> + Clone + Send, S, H> ClientBuilder<http::Request, T, U, S, H> {
    pub fn http() -> ClientBuilder<http::Request, T, U, MissingHttpService, MissingHealthService> {
        Self::default()
    }
}

impl<'a, U: tower::Service<http::Request> + Clone + Send, H> ClientBuilder<http::Request, hyper_util::client::legacy::Client::<HttpsConnector::<HttpConnector>, http::Body>, U, MissingHttpService, H> {
    #[instrument(skip_all, fields(hostname = service_config.hostname))]
    pub async fn service(self, service_config: &ServiceConfig<'_>) -> ClientBuilder<http::Request, hyper_util::client::legacy::Client::<HttpsConnector::<HttpConnector>, http::Body>, U, WithHttpService, H>
    {
        let base_url = service_config.base_url();
        debug!(%base_url, "creating HTTP client");
        let request_timeout = Duration::from_secs(service_config.request_timeout);
        let connection_timeout = Duration::from_secs(service_config.connection_timeout);

        let client = hyper_util::client::legacy::Client::builder(TokioExecutor::new())
            .http2_keep_alive_timeout(connection_timeout)
            .pool_idle_timeout(request_timeout)
            .to_owned();

        let https_conn_builder = match &service_config.tls {
            Some(tls) => hyper_rustls::HttpsConnectorBuilder::new().with_tls_config(utils::tls::build_client_config(&tls)),
            None => hyper_rustls::HttpsConnectorBuilder::new().with_tls_config(utils::tls::build_insecure_client_config())
        };

        let https_conn = https_conn_builder
            .https_or_http()
            .enable_http1()
            .enable_http2()
            .build();

        let service = client.build(https_conn);

        ClientBuilder::<http::Request, hyper_util::client::legacy::Client<_, _>, U, WithHttpService, H> {
            _request_type: PhantomData::default(),
            _service_state: PhantomData::default(),
            _health_service_state: PhantomData::default(),
            service: Some(service),
            health_service: Some(self.health_service.unwrap()),
        }
    }
}

impl<'a, T: tower::Service<http::Request> + Clone + Send, S> ClientBuilder<http::Request, T, hyper_util::client::legacy::Client::<HttpsConnector::<HttpConnector>, http::Body>, S, MissingHealthService> {
    #[instrument(skip_all, fields(hostname = service_config.hostname))]
    pub async fn health_service<K: Kind>(self, service_config: &ServiceConfig<'a>) -> ClientBuilder<http::Request, T, hyper_util::client::legacy::Client::<HttpsConnector::<HttpConnector>, http::Body>, S, WithHealthService<Http>> {
        if K::type_id(private::Seal) == TypeId::of::<Http>(){
            let base_url = service_config.base_url();
            debug!(%base_url, "creating HTTP health client");
            let request_timeout = Duration::from_secs(service_config.request_timeout);
            let connection_timeout = Duration::from_secs(service_config.connection_timeout);

            let client_builder = hyper_util::client::legacy::Client::builder(TokioExecutor::new())
                .http2_keep_alive_timeout(connection_timeout)
                .pool_idle_timeout(request_timeout);

            let https_conn_builder = match &service_config.tls {
                Some(tls) => hyper_rustls::HttpsConnectorBuilder::new().with_tls_config(utils::tls::build_client_config(&tls)),
                None => hyper_rustls::HttpsConnectorBuilder::new().with_tls_config(utils::tls::build_insecure_client_config())
            };

            let https_conn = https_conn_builder
                .https_or_http()
                .enable_http1()
                .enable_http2()
                .build();

            let service = client_builder.build(https_conn);
            ClientBuilder::<http::Request, T, hyper_util::client::legacy::Client<_, _>, S, WithHealthService<Http>> {
                _request_type: PhantomData::default(),
                _service_state: PhantomData::default(),
                _health_service_state: PhantomData::default(),
                service: self.service,
                health_service: Some(service),
            }
        } else {
            panic!("unexpected client type {:?}, expected HTTP", K::type_id(private::Seal));
        }
    }
}

impl<'a, T: tower::Service<grpc::Request>, U: tower::Service<grpc::Request>, S, H> ClientBuilder<grpc::Request, T, U, S, H> {
    pub fn grpc() -> ClientBuilder<grpc::Request, T, U, MissingGrpcService, MissingHealthService> {
        Self::default()
    }
}

impl<'a, T: tower::Service<grpc::Request>, U: tower::Service<grpc::Request>, H> ClientBuilder<grpc::Request, T, U, MissingGrpcService, H> {
    #[instrument(skip_all, fields(hostname = service_config.hostname))]
    pub async fn service<G: tower::Service<grpc::Request>>(self, service_config: &ServiceConfig<'_>, new: fn(LoadBalancedChannel) -> G) -> ClientBuilder<grpc::Request, T, U, WithGrpcService<G>, H> {
        let base_url = service_config.base_url();
        debug!(%base_url, "creating gRPC client");
        let request_timeout = Duration::from_secs(service_config.request_timeout);
        let connection_timeout = Duration::from_secs(service_config.connection_timeout);

        let mut builder = LoadBalancedChannel::builder((service_config.hostname.clone(), service_config.port))
            .connect_timeout(connection_timeout)
            .timeout(request_timeout);

        match &service_config.tls {
            Some(tls) => {
                let mut client_tls_config = tonic::transport::ClientTlsConfig::new();
                match &tls.insecure {
                    true => {},
                    false => match &tls.key {
                        Some(key) => {
                            let identity = tonic::transport::Identity::from_pem(&tls.cert, key);
                            let client_tls_config = client_tls_config.clone().identity(identity)
                                .with_native_roots()
                                .with_webpki_roots();
                            builder = builder.with_tls(client_tls_config.clone());
                        }
                        None => {}
                    }
                }
                client_tls_config = client_tls_config
                    .ca_certificate(tonic::transport::Certificate::from_pem(&tls.ca_cert));
            },
            None => {}
        }

        let channel = builder
            .channel()
            .await
            .unwrap_or_else(|error| panic!("error creating grpc client: {error}"));

        let service = new(channel);

        ClientBuilder::<grpc::Request, T, U, WithGrpcService<G>, H> {
            _request_type: PhantomData::default(),
            _service_state: PhantomData::default(),
            _health_service_state: PhantomData::default(),
            service: Some(service),
            health_service: None,
        }
    }
}

impl<'a, T: tower::Service<grpc::Request>, U: tower::Service<grpc::Request>, S, H> ClientBuilder<grpc::Request, T, U, S, MissingHealthService> {
    #[instrument(skip_all, fields(hostname = service_config.hostname))]
    pub async fn health_service<K: Kind>(self, service_config: &ServiceConfig<'a>) -> ClientBuilder<grpc::Request, T, U, S, WithHealthService<Grpc>> {
        if K::type_id(private::Seal) == TypeId::of::<Grpc>(){
            let base_url = service_config.base_url();
            debug!(%base_url, "creating gRPC health client");
            let request_timeout = Duration::from_secs(service_config.request_timeout);
            let connection_timeout = Duration::from_secs(service_config.connection_timeout);

            let client_builder = hyper_util::client::legacy::Client::builder(TokioExecutor::new())
                .http2_keep_alive_timeout(connection_timeout)
                .pool_idle_timeout(request_timeout);

            let https_conn_builder = match &service_config.tls {
                Some(tls) => hyper_rustls::HttpsConnectorBuilder::new().with_tls_config(utils::tls::build_client_config(&tls)),
                None => hyper_rustls::HttpsConnectorBuilder::new().with_tls_config(utils::tls::build_insecure_client_config())
            };

            let https_conn = https_conn_builder
                .https_or_http()
                .enable_http1()
                .enable_http2()
                .build();

            let service = client_builder.build(https_conn);

            ClientBuilder::<grpc::Request, T, U, S, WithHealthService<Grpc>> {
                _request_type: PhantomData::default(),
                _service_state: PhantomData::default(),
                _health_service_state: PhantomData::default(),
                service: Some(service),
                health_service: self.health_service,
                _response_type: Default::default(),
            }
        } else {
            panic!("unexpected client type {:?}, expected HTTP", K::type_id(private::Seal));
        }
    }
}

/// Returns `true` if hostname is valid according to [IETF RFC 1123](https://tools.ietf.org/html/rfc1123).
///
/// Conditions:
/// - It does not start or end with `-` or `.`.
/// - It does not contain any characters outside of the alphanumeric range, except for `-` and `.`.
/// - It is not empty.
/// - It is 253 or fewer characters.
/// - Its labels (characters separated by `.`) are not empty.
/// - Its labels are 63 or fewer characters.
/// - Its labels do not start or end with '-' or '.'.
pub fn is_valid_hostname(hostname: &str) -> bool {
    fn is_valid_char(byte: u8) -> bool {
        byte.is_ascii_lowercase()
            || byte.is_ascii_uppercase()
            || byte.is_ascii_digit()
            || byte == b'-'
            || byte == b'.'
    }
    !(hostname.bytes().any(|byte| !is_valid_char(byte))
        || hostname.split('.').any(|label| {
        label.is_empty() || label.len() > 63 || label.starts_with('-') || label.ends_with('-')
    })
        || hostname.is_empty()
        || hostname.len() > 253)
}


