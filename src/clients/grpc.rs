use std::{
    fmt::Debug,
    ops::{Deref, DerefMut},
    time::Duration,
};

use axum::http::{Extensions, HeaderMap};
use ginepro::LoadBalancedChannel;
use http_body_util::combinators::BoxBody;
use hyper::{
    body::{Bytes, Incoming},
    Response,
};
use tonic::{metadata::MetadataMap, Request};
use tower::ServiceBuilder;
use tower_http::{
    classify::{GrpcErrorsAsFailures, GrpcFailureClass, SharedClassifier},
    trace::{
        DefaultOnBodyChunk, GrpcMakeClassifier, MakeSpan, OnEos, OnFailure, OnRequest, OnResponse,
        Trace, TraceLayer,
    },
};
use tracing::{debug, error, info, info_span, instrument, Span};
use tracing_opentelemetry::OpenTelemetrySpanExt;

use super::errors::{grpc_to_http_code, Error};
use crate::{
    config::{ServiceConfig, ServiceDefaults},
    utils::trace::{current_trace_id, with_traceparent_header},
};

pub type GrpcServiceRequest = hyper::Request<tonic::body::BoxBody>;

#[derive(Debug, Clone)]
pub struct GrpcClient<C: Debug + Clone>(
    Trace<
        C,
        SharedClassifier<GrpcErrorsAsFailures>,
        ClientMakeSpan,
        ClientOnRequest,
        ClientOnResponse,
        DefaultOnBodyChunk,
        ClientOnEos,
        ClientOnFailure,
    >,
);

impl<'a, C: Debug + Clone> GrpcClient<C> {
    pub fn from_config(service_config: &'a ServiceConfig) -> GrpcClientBuilder<'a, C> {
        GrpcClientBuilder::from_config(service_config)
    }
}

impl<C: Debug + Clone> Deref for GrpcClient<C> {
    type Target = C;

    fn deref(&self) -> &Self::Target {
        self.0.get_ref()
    }
}

impl<C: Debug + Clone> DerefMut for GrpcClient<C> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.0.get_mut()
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
    pub async fn build(self) -> Result<GrpcClient<C>, Error> {
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

        let client = new_fn(channel);
        let service = ServiceBuilder::new()
            .layer(grpc_trace_layer())
            .service(client);
        Ok(GrpcClient(service))
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

pub type GrpcClientTraceLayer = TraceLayer<
    GrpcMakeClassifier,
    ClientMakeSpan,
    ClientOnRequest,
    ClientOnResponse,
    DefaultOnBodyChunk, // no metrics currently per body chunk
    ClientOnEos,
    ClientOnFailure,
>;

pub fn grpc_trace_layer() -> GrpcClientTraceLayer {
    TraceLayer::new_for_grpc()
        .make_span_with(ClientMakeSpan)
        .on_request(ClientOnRequest)
        .on_response(ClientOnResponse)
        .on_failure(ClientOnFailure)
        .on_eos(ClientOnEos)
}

#[derive(Debug, Clone)]
pub struct ClientMakeSpan;

impl MakeSpan<BoxBody<Bytes, hyper::Error>> for ClientMakeSpan {
    fn make_span(&mut self, request: &hyper::Request<BoxBody<Bytes, hyper::Error>>) -> Span {
        info_span!(
            "client gRPC request",
            request_method = request.method().to_string(),
            request_path = request.uri().path().to_string(),
        )
    }
}

#[derive(Debug, Clone)]
pub struct ClientOnRequest;

impl OnRequest<BoxBody<Bytes, hyper::Error>> for ClientOnRequest {
    fn on_request(&mut self, request: &hyper::Request<BoxBody<Bytes, hyper::Error>>, span: &Span) {
        let _guard = span.enter();
        info!(
            "outgoing gRPC client request to {} {} with trace {:?}",
            request.method(),
            request.uri().path(),
            current_trace_id(),
        );
        info!(
            monotonic_counter.incoming_request_count = 1,
            request_method = request.method().as_str(),
            request_path = request.uri().path()
        );
    }
}

#[derive(Debug, Clone)]
pub struct ClientOnResponse;

impl OnResponse<Incoming> for ClientOnResponse {
    fn on_response(self, response: &Response<Incoming>, latency: Duration, span: &Span) {
        let _guard = span.enter();
        info!(
            "gRPC client response {} for request with trace {:?} received in {} ms",
            &response.status(),
            current_trace_id(),
            latency.as_millis()
        );

        // On every response
        info!(
            monotonic_counter.client_response_count = 1,
            response_status = response.status().as_u16(),
            request_duration = latency.as_millis()
        );
        info!(
            histogram.client_request_duration = latency.as_millis(),
            response_status = response.status().as_u16()
        );

        if response.status().is_server_error() {
            // ServerOnFailure should catch 5xx as failures, TODO: check if this block is ever reached
            // On every server error (HTTP 5xx) response
            info!(
                monotonic_counter.client_5xx_response_count = 1,
                response_status = response.status().as_u16(),
                request_duration = latency.as_millis()
            );
        } else if response.status().is_client_error() {
            // On every client error (HTTP 4xx) response
            info!(
                monotonic_counter.client_4xx_response_count = 1,
                response_status = response.status().as_u16(),
                request_duration = latency.as_millis()
            );
        } else if response.status().is_success() {
            // On every successful (HTTP 2xx) response
            info!(
                monotonic_counter.client_success_response_count = 1,
                response_status = response.status().as_u16(),
                request_duration = latency.as_millis()
            );
        } else {
            error!(
                "unexpected gRPC client response status code: {}",
                response.status().as_u16()
            );
        }
    }
}

#[derive(Debug, Clone)]
pub struct ClientOnFailure;

impl OnFailure<GrpcFailureClass> for ClientOnFailure {
    fn on_failure(
        &mut self,
        failure_classification: GrpcFailureClass,
        latency: Duration,
        span: &Span,
    ) {
        let _guard = span.enter();
        let trace_id = current_trace_id();
        let latency_ms = latency.as_millis().to_string();

        let (status_code, error) = match failure_classification {
            GrpcFailureClass::Code(code) => {
                error!(
                    ?trace_id,
                    code, latency_ms, "gRPC client failed to handle request",
                );
                (Some(grpc_to_http_code(tonic::Code::from(code.get()))), None)
            }
            GrpcFailureClass::Error(error) => {
                error!(
                    ?trace_id,
                    latency_ms, "gRPC client failed to handle request: {}", error,
                );
                (None, Some(error))
            }
        };

        info!(
            monotonic_counter.client_request_failure_count = 1,
            latency_ms,
            ?status_code,
            ?error
        );
        info!(
            monotonic_counter.client_5xx_response_count = 1,
            latency_ms,
            ?status_code,
            ?error
        );
    }
}

#[derive(Debug, Clone)]
pub struct ClientOnEos;

impl OnEos for ClientOnEos {
    fn on_eos(self, trailers: Option<&HeaderMap>, stream_duration: Duration, span: &Span) {
        let _guard = span.enter();
        info!(
            "gRPC client stream response for request with trace {:?} closed after {} ms with trailers: {:?}",
            current_trace_id(),
            stream_duration.as_millis(),
            trailers
        );
        info!(
            monotonic_counter.client_stream_response_count = 1,
            stream_duration = stream_duration.as_millis()
        );
        info!(monotonic_histogram.client_stream_response_duration = stream_duration.as_millis());
    }
}
