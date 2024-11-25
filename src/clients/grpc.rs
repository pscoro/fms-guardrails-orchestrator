use axum::http::{Extensions, HeaderMap};
use ginepro::LoadBalancedChannel;
use std::fmt::Debug;
use std::ops::{Deref, DerefMut};
use tonic::{metadata::MetadataMap, Request};
use tower::ServiceBuilder;
use tracing::{debug, instrument, Span};
use tracing_opentelemetry::OpenTelemetrySpanExt;

use crate::{
    clients::Error,
    config::{ServiceConfig, ServiceDefaults},
    utils::trace::with_traceparent_header,
};

#[derive(Debug, Clone)]
pub struct GrpcClient<C: Debug + Clone>(C);

impl<'a, C: Debug + Clone> GrpcClient<C> {
    pub fn from_config(service_config: &'a ServiceConfig) -> GrpcClientBuilder<'a, C> {
        GrpcClientBuilder::from_config(service_config)
    }
}

impl<C: Debug + Clone> Deref for GrpcClient<C> {
    type Target = C;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<C: Debug + Clone> DerefMut for GrpcClient<C> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

#[derive(Debug, Clone)]
pub struct GrpcClientBuilder<'a, C: Debug + Clone> {
    service_config: Option<&'a ServiceConfig>,
    /// Every client implementation needs to specify its own default port
    /// There is no default across all clients so this is not part of the service defaults
    default_port: Option<u16>,
    new_fn: Option<fn(LoadBalancedChannel) -> C>,
    service_defaults: ServiceDefaults,
}

impl<'a, C: Debug + Clone> Default for GrpcClientBuilder<'a, C> {
    fn default() -> Self {
        Self {
            service_config: None,
            default_port: None,
            new_fn: None,
            service_defaults: ServiceDefaults::default(),
        }
    }
}

impl<'a, C: Debug + Clone> GrpcClientBuilder<'a, C> {
    pub fn from_config(service_config: &'a ServiceConfig) -> Self {
        Self::default().with_config(service_config)
    }

    fn with_config(mut self, service_config: &'a ServiceConfig) -> Self {
        self.service_config = Some(service_config);
        self
    }

    pub fn with_default_port(mut self, default_port: u16) -> Self {
        self.default_port = Some(default_port);
        self
    }

    pub fn with_new_fn(mut self, new_fn: fn(LoadBalancedChannel) -> C) -> Self {
        self.new_fn = Some(new_fn);
        self
    }

    pub fn with_service_defaults(mut self, service_defaults: ServiceDefaults) -> Self {
        self.service_defaults = service_defaults;
        self
    }

    #[instrument(skip_all)]
    pub async fn build(self) -> Result<C, Error> {
        let service_config = self
            .service_config
            .ok_or(Error::BuildFailed("no service config provided".to_string()))?;
        let default_port = self.default_port.ok_or(Error::BuildFailed(format!(
            "no default port provided for client: {}",
            service_config.hostname
        )))?;
        let new_fn = self.new_fn.ok_or(Error::BuildFailed(format!(
            "no gRPC new service function provided: {}",
            service_config.hostname
        )))?;
        let base_url = service_config.base_url_from_port_or(default_port);
        let ServiceDefaults {
            health_endpoint: _,
            request_timeout_sec,
            connection_timeout_sec,
        } = self.service_defaults;
        debug!(%base_url, "creating gRPC client");

        let channel = LoadBalancedChannel::builder((
            service_config.hostname.clone(),
            service_config.port_or(default_port),
        ))
        .connect_timeout(service_config.connection_timeout_or(connection_timeout_sec))
        .timeout(service_config.request_timeout_or(request_timeout_sec))
        .with_tls(
            service_config
                .grpc_tls_config()
                .await
                .map_err(|e| Error::BuildFailed(e.to_string()))?,
        )
        .channel()
        .await
        .unwrap_or_else(|error| panic!("error creating grpc client: {error}"));

        let channel = ServiceBuilder::new()
            // .layer(grpc::TraceLayer::new())
            .service(channel);
        Ok(new_fn(channel))
    }
}

/// Turns a gRPC client request body of type `T` and header map into a `tonic::Request<T>`.
/// Will also inject the current `traceparent` header into the request based on the current span.
pub fn grpc_request_with_headers<T>(request: T, headers: HeaderMap) -> Request<T> {
    let ctx = Span::current().context();
    let headers = with_traceparent_header(&ctx, headers);
    let metadata = MetadataMap::from_headers(headers);
    Request::from_parts(metadata, Extensions::new(), request)
}
