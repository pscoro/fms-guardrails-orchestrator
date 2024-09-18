/*
 Copyright FMS Guardrails Orchestrator Authors

 Licensed under the Apache License, Version 2.0 (the "License");
 you may not use this file except in compliance with the License.
 You may obtain a copy of the License at

     http://www.apache.org/licenses/LICENSE-2.0

 Unless required by applicable law or agreed to in writing, software
 distributed under the License is distributed on an "AS IS" BASIS,
 WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 See the License for the specific language governing permissions and
 limitations under the License.

*/

use axum::{
    response::{sse::Event, Sse},
    Json,
};
use futures::stream::BoxStream;
use hyper::body::Incoming;
use opentelemetry::trace::{SpanId, TraceFlags};
use opentelemetry::{
    global,
    metrics::{Counter, Histogram, Meter},
    trace::{SpanContext, TraceContextExt, TraceId, TracerProvider},
    KeyValue,
};
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::{propagation::TraceContextPropagator, trace, trace::Sampler, Resource};
use serde::ser::SerializeStruct;
use serde::{Deserialize, Serialize};
use std::time::SystemTime;
use std::{
    convert::Infallible,
    fmt::{Display, Formatter},
    sync::Arc,
};
use tokio::time::Instant;
use tonic::metadata::AsciiMetadataKey;
use tracing::{error, Span};
use tracing_opentelemetry::OpenTelemetrySpanExt;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Layer};
use uuid::Uuid;

use crate::clients::{ClientKind, GenerationClientKind};
use crate::models::OrchestratorRequest;
use crate::{clients, orchestrator, server};

pub const ALLOWED_HEADERS: [&str; 1] = ["traceparent"];

pub fn init_tracer(service_name: String, json_output: bool, otlp_endpoint: Option<String>) {
    let mut layers = Vec::new();

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or(EnvFilter::new("INFO"))
        .add_directive("ginepro=info".parse().unwrap());

    let fmt_layer = match json_output {
        true => tracing_subscriber::fmt::layer()
            .json()
            .flatten_event(true)
            .boxed(),
        false => tracing_subscriber::fmt::layer().boxed(),
    };
    layers.push(fmt_layer);

    if let Some(tracing_otlp_endpoint) = otlp_endpoint {
        global::set_text_map_propagator(TraceContextPropagator::new());
        let provider = opentelemetry_otlp::new_pipeline()
            .tracing()
            .with_exporter(
                opentelemetry_otlp::new_exporter()
                    .tonic()
                    .with_endpoint(tracing_otlp_endpoint),
            )
            .with_trace_config(
                trace::Config::default()
                    .with_resource(Resource::new(vec![KeyValue::new(
                        "service.name",
                        service_name.clone(),
                    )]))
                    .with_sampler(Sampler::AlwaysOn),
            )
            .install_batch(opentelemetry_sdk::runtime::Tokio);

        if let Ok(provider) = provider {
            layers.push(
                tracing_opentelemetry::layer()
                    .with_tracer(provider.tracer(service_name))
                    .boxed(),
            );
        };
    }

    tracing_subscriber::registry()
        .with(filter)
        .with(layers)
        .init();
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RequestInfo {
    pub trace: RequestTrace,
    pub metadata: RequestMetadata,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extra_metadata: Option<ExtraRequestMetadata>,
    pub child_request_metadata: Vec<RequestMetadata>,
}

#[cfg(test)]
impl Default for RequestInfo {
    fn default() -> Self {
        Self {
            trace: RequestTrace::default(),
            metadata: RequestMetadata::default(),
            extra_metadata: None,
            child_request_metadata: Vec::new(),
        }
    }
}

impl RequestInfo {
    pub fn incoming(trace: RequestTrace, metadata: RequestMetadata) -> Self {
        Self {
            trace,
            metadata,
            extra_metadata: None,
            child_request_metadata: Vec::new(),
        }
        .with_current_span_context()
    }

    pub fn unary(&self, request: &impl OrchestratorRequest) -> Self {
        let info = self.clone().with_current_span_context();
        let extra_metadata = ExtraRequestMetadata {
            is_streaming: false,
            generation_model_id: request.generation_model_id(),
            with_detection: request.with_detection(),
        };
        RequestInfo {
            extra_metadata: Some(extra_metadata),
            ..info
        }
    }

    pub fn streaming(&self, request: &impl OrchestratorRequest) -> Self {
        let info = self.clone().with_current_span_context();
        let extra_metadata = ExtraRequestMetadata {
            is_streaming: true,
            generation_model_id: request.generation_model_id(),
            with_detection: request.with_detection(),
        };
        RequestInfo {
            extra_metadata: Some(extra_metadata),
            ..info
        }
    }

    pub fn tgis_client<T>(
        self,
        request: T,
        id: String,
        path: impl Into<String>,
    ) -> (RequestInfo, InfoIntoRequest<T>) {
        let mut info = self.with_current_span_context().clone();
        let path = path.into();
        let mut headers = info.metadata.kind.headers().clone();
        headers.insert(
            "traceparent",
            TraceParent::from(info.trace.clone())
                .to_string()
                .parse()
                .unwrap(),
        );
        let kind = RequestMetadataKind::grpc(
            tonic::metadata::MetadataMap::from_headers(headers),
            path.clone(),
        );
        let child_metadata = info.metadata.clone().into_tgis_client(id, kind);
        info.add_child_request_metadata(child_metadata.clone());
        let client_request_into = InfoIntoRequest::from((child_metadata, request));
        (info, client_request_into)
    }

    pub fn add_child_request_metadata(&mut self, metadata: RequestMetadata) {
        self.child_request_metadata.push(metadata);
    }

    pub fn with_current_span_context(mut self) -> Self {
        let context = Span::current().context().span().span_context().clone();
        self.trace = self.trace.update_context(context);
        self
    }

    pub fn created_at(&self) -> SystemTime {
        self.metadata.created_at()
    }

    fn measure_duration(&self) -> f64 {
        self.created_at()
            .elapsed()
            .unwrap_or_else(|e| {
                error!(
                    "Failed to calculate duration for outgoing stream request: {}",
                    e
                );
                std::time::Duration::from_secs(0)
            })
            .as_secs_f64()
    }

    pub fn to_labels(&self) -> Vec<KeyValue> {
        let mut labels = vec![
            KeyValue::new("request_id", self.trace.request_id.to_string()),
            KeyValue::new("trace_id", self.trace.trace_id.to_string()),
            KeyValue::new("span_id", self.trace.context.span_id().to_string()),
            KeyValue::new("created_at", format!("{:?}", self.created_at())),
            KeyValue::new("service", self.metadata.service.to_string()),
            KeyValue::new("service_kind", self.metadata.kind.to_string()),
            KeyValue::new("service_path", self.metadata.kind.to_string()),
        ];

        match self.clone().metadata.kind {
            RequestMetadataKind::Http {
                headers,
                method,
                path,
            } => {
                let headers = headers.clone();
                labels.push(KeyValue::new("method", method.to_string()));
                labels.push(KeyValue::new("path", path.to_string()));
                for (key, value) in headers.iter() {
                    let (key, value) = (key.clone(), value.clone());
                    if ALLOWED_HEADERS.contains(&key.as_str()) {
                        labels.push(KeyValue::new(
                            format!("header.{}", key.clone()),
                            value.to_str().unwrap().to_owned(),
                        ));
                    }
                }
            }
            RequestMetadataKind::Grpc { metadata, path } => {
                let metadata = metadata.clone();
                labels.push(KeyValue::new("path", path.to_string()));
                for (key, value) in metadata.into_headers().iter() {
                    let (key, value) = (key.clone(), value.clone());
                    if ALLOWED_HEADERS.contains(&key.as_str()) {
                        labels.push(KeyValue::new(
                            format!("header.{}", key),
                            value.to_str().unwrap().to_owned(),
                        ));
                    }
                }
            }
        }

        if let Some(extra_metadata) = &self.extra_metadata {
            labels.push(KeyValue::new(
                "streaming",
                extra_metadata.is_streaming.to_string(),
            ));
            labels.push(KeyValue::new(
                "with_generation",
                extra_metadata.generation_model_id.is_some().to_string(),
            ));
            labels.push(KeyValue::new(
                "with_detection",
                extra_metadata.with_detection.to_string(),
            ));

            if let Some(model_id) = extra_metadata.generation_model_id.as_ref() {
                labels.push(KeyValue::new("model_id", model_id.to_string()));
            }
        }

        labels
    }
}

fn serialize_uuid_str<S>(uuid: &Uuid, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_str(&uuid.to_string())
}

fn serialize_trace_id_str<S>(trace_id: &TraceId, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_str(&trace_id.to_string())
}

fn serialize_span_context_as_span_id_str<S>(
    context: &SpanContext,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_str(&context.span_id().to_string())
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RequestTrace {
    #[serde(serialize_with = "serialize_uuid_str")]
    pub request_id: Uuid,
    // Remove after verifying its preserved across span contexts
    #[serde(serialize_with = "serialize_trace_id_str")]
    pub trace_id: TraceId,
    #[serde(
        rename = "span_id",
        serialize_with = "serialize_span_context_as_span_id_str"
    )]
    pub context: SpanContext,
}

impl Default for RequestTrace {
    fn default() -> Self {
        Self {
            request_id: Uuid::new_v4(),
            trace_id: TraceId::INVALID,
            context: SpanContext::empty_context(),
        }
    }
}

impl RequestTrace {
    pub fn new(request_id: Uuid, context: SpanContext) -> Self {
        Self {
            request_id,
            trace_id: context.trace_id(),
            context,
        }
    }

    pub fn update_context(mut self, context: SpanContext) -> Self {
        if context.trace_id() != self.trace_id {
            panic!("Trace ID mismatch");
        }
        self.context = context;
        self
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RequestMetadata {
    pub service: RequestService,
    pub kind: RequestMetadataKind,
    pub created_at: Instant,
}

#[cfg(test)]
impl Default for RequestMetadata {
    fn default() -> Self {
        Self {
            service: RequestService::default(),
            kind: RequestMetadataKind::default(),
            created_at: Instant::now(),
        }
    }
}

impl Serialize for RequestMetadata {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut state = serializer.serialize_struct("RequestMetadata", 3)?;
        state.serialize_field("service", &self.service)?;
        state.serialize_field("kind", &self.kind)?;
        state.serialize_field("created_at", &self.created_at())?;
        state.end()
    }
}

impl RequestMetadata {
    pub fn orchestrator(request: &axum::extract::Request<Incoming>) -> Self {
        let created_at = Instant::now();
        Self {
            service: RequestService::Orchestrator,
            kind: request.into(),
            created_at,
        }
    }

    pub fn into_chunker_client(self, id: String, kind: impl Into<RequestMetadataKind>) -> Self {
        let created_at = Instant::now();
        Self {
            service: RequestService::Client {
                id,
                kind: ClientKind::Chunker,
            },
            kind: kind.into(),
            created_at,
        }
    }

    pub fn into_detector_client(self, id: String, kind: impl Into<RequestMetadataKind>) -> Self {
        let created_at = Instant::now();
        Self {
            service: RequestService::Client {
                id,
                kind: ClientKind::Detector,
            },
            kind: kind.into(),
            created_at,
        }
    }

    pub fn into_nlp_client(self, id: String, kind: impl Into<RequestMetadataKind>) -> Self {
        let created_at = Instant::now();
        Self {
            service: RequestService::Client {
                id,
                kind: ClientKind::Generation(GenerationClientKind::Nlp),
            },
            kind: kind.into(),
            created_at,
        }
    }

    pub fn into_tgis_client(self, id: String, kind: impl Into<RequestMetadataKind>) -> Self {
        let created_at = Instant::now();
        Self {
            service: RequestService::Client {
                id,
                kind: ClientKind::Generation(GenerationClientKind::Tgis),
            },
            kind: kind.into(),
            created_at,
        }
    }

    pub fn created_at(&self) -> SystemTime {
        let time_elapsed = self.created_at.elapsed();
        let now = SystemTime::now();
        now.checked_sub(time_elapsed).unwrap()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RequestService {
    Orchestrator,
    Client { id: String, kind: ClientKind },
}

#[cfg(test)]
impl Default for RequestService {
    fn default() -> Self {
        Self::Orchestrator
    }
}

impl Display for RequestService {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Orchestrator => write!(f, "orchestrator"),
            Self::Client { id, kind } => write!(f, "{} client: {}", kind, id),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RequestMetadataKind {
    Http {
        #[serde(skip)]
        headers: reqwest::header::HeaderMap,
        method: String,
        path: String,
    },
    Grpc {
        #[serde(skip)]
        metadata: tonic::metadata::MetadataMap,
        path: String,
    },
}

impl PartialEq for RequestMetadataKind {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::Http {
                    headers: headers1,
                    method: method1,
                    path: path1,
                },
                Self::Http {
                    headers: headers2,
                    method: method2,
                    path: path2,
                },
            ) => headers1 == headers2 && method1 == method2 && path1 == path2,
            (
                Self::Grpc {
                    metadata: metadata1,
                    path: path1,
                },
                Self::Grpc {
                    metadata: metadata2,
                    path: path2,
                },
            ) => {
                metadata1.clone().into_headers() == metadata2.clone().into_headers()
                    && path1 == path2
            }
            _ => false,
        }
    }
}

impl Default for RequestMetadataKind {
    fn default() -> Self {
        Self::Http {
            headers: reqwest::header::HeaderMap::new(),
            method: "".to_string(),
            path: "".to_string(),
        }
    }
}

impl Display for RequestMetadataKind {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Http { .. } => write!(f, "http"),
            Self::Grpc { .. } => write!(f, "grpc"),
        }
    }
}

impl From<&axum::extract::Request<Incoming>> for RequestMetadataKind {
    fn from(request: &axum::extract::Request<Incoming>) -> Self {
        Self::Http {
            headers: request.headers().clone(),
            method: request.method().to_string(),
            path: request.uri().path().to_string(),
        }
    }
}

impl RequestMetadataKind {
    pub fn http(headers: reqwest::header::HeaderMap, method: String, path: String) -> Self {
        Self::Http {
            headers,
            method,
            path,
        }
    }

    pub fn grpc(metadata: tonic::metadata::MetadataMap, path: String) -> Self {
        Self::Grpc { metadata, path }
    }

    pub fn headers(&self) -> reqwest::header::HeaderMap {
        match self {
            Self::Http { headers, .. } => headers.clone(),
            Self::Grpc { metadata, .. } => metadata.clone().into_headers(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExtraRequestMetadata {
    pub is_streaming: bool,
    pub generation_model_id: Option<String>,
    pub with_detection: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestStats {
    pub duration: f64,
    pub success: bool,
}

pub struct Metrics {
    /// Total count of orchestrator service requests received/handled
    pub service_request_count: Counter<u64>,

    /// Histogram of non-streaming orchestrator service request durations
    pub service_request_duration: Histogram<f64>,
    /// Total count of non-streaming orchestrator service requests that were not successful
    pub service_request_error_count: Counter<u64>,

    /// Total count of orchestrator service requests received/handled that are streaming responses
    pub service_stream_request_count: Counter<u64>,
    /// Histogram of orchestrator service streaming request durations
    pub service_stream_request_duration: Histogram<f64>,
    /// Total count of events in orchestrator service streaming requests
    pub service_stream_request_event_count: Counter<u64>,
    /// Total count of events that are errors in orchestrator service streaming requests
    pub service_stream_request_error_event_count: Counter<u64>,

    /// Total count of outgoing client requests made by the orchestrator
    pub client_request_count: Counter<u64>,

    /// Histogram of non-streaming outgoing client request durations
    pub client_request_duration: Histogram<f64>,
    /// Total count of non-streaming outgoing client requests that were not successful
    pub client_request_error_count: Counter<u64>,

    /// Total count of outgoing client requests made that are streaming responses
    pub client_stream_request_count: Counter<u64>,
    /// Histogram of outgoing client streaming request durations
    pub client_stream_request_duration: Histogram<f64>,
    /// Total count of events in outgoing client streaming requests
    pub client_stream_request_event_count: Counter<u64>,
    /// Total count of events that are errors in outgoing client streaming requests
    pub client_stream_request_error_event_count: Counter<u64>,
}

impl Metrics {
    pub fn new(meter: &Meter) -> Arc<Self> {
        Arc::new(Self {
            service_request_count: meter.u64_counter("service_request_count").init(),

            service_request_duration: meter.f64_histogram("service_request_duration").init(),
            service_request_error_count: meter.u64_counter("service_request_error_count").init(),

            service_stream_request_count: meter.u64_counter("service_stream_request_count").init(),
            service_stream_request_duration: meter
                .f64_histogram("service_stream_request_duration")
                .init(),
            service_stream_request_event_count: meter
                .u64_counter("service_stream_request_event_count")
                .init(),
            service_stream_request_error_event_count: meter
                .u64_counter("service_stream_request_error_event_count")
                .init(),

            client_request_count: meter.u64_counter("client_request_count").init(),

            client_request_duration: meter.f64_histogram("client_request_duration").init(),
            client_request_error_count: meter.u64_counter("client_request_error_count").init(),

            client_stream_request_count: meter.u64_counter("client_stream_request_count").init(),
            client_stream_request_duration: meter
                .f64_histogram("client_stream_request_duration")
                .init(),
            client_stream_request_event_count: meter
                .u64_counter("client_stream_request_event_count")
                .init(),
            client_stream_request_error_event_count: meter
                .u64_counter("client_stream_request_error_event_count")
                .init(),
        })
    }

    pub fn record_service_request(&self, request_info: &RequestInfo, stats: RequestStats) {
        let labels = request_info.to_labels();

        self.service_request_count.add(1, &labels);
        self.service_request_duration
            .record(stats.duration, &labels);
        if !stats.success {
            self.service_request_error_count.add(1, &labels);
        }
        if let Some(extra_metadata) = &request_info.extra_metadata {
            if extra_metadata.is_streaming {
                self.service_stream_request_count.add(1, &labels);
            }
        }
    }

    pub fn record_outgoing_request(&self, request_info: &RequestInfo, stats: RequestStats) {
        let labels = request_info.to_labels();

        self.client_request_count.add(1, &labels);
        self.client_request_duration.record(stats.duration, &labels);
        if !stats.success {
            self.client_request_error_count.add(1, &labels);
        }
        if let Some(extra_metadata) = &request_info.extra_metadata {
            if extra_metadata.is_streaming {
                self.client_stream_request_count.add(1, &labels);
            }
        }
    }

    pub fn record_stream_event(&self, request_info: &RequestInfo, event: &str) {
        let mut labels = request_info.to_labels();
        labels.insert(0, KeyValue::new("event", event.to_string()));

        self.service_stream_request_event_count.add(1, &labels);
    }

    pub fn record_stream_error(&self, message: &str, request_info: &RequestInfo, error: &str) {
        let mut labels = request_info.to_labels();
        labels.insert(0, KeyValue::new("error", error.to_string()));
        labels.insert(1, KeyValue::new("message", message.to_string()));

        self.service_stream_request_error_event_count
            .add(1, &labels);
    }

    pub fn record_stream_duration(&self, request_info: &RequestInfo, duration: f64) {
        let labels = request_info.to_labels();

        self.service_stream_request_duration
            .record(duration, &labels);
    }
}

pub async fn trace_incoming_request_metrics<F, Fut, R>(
    request_info: &RequestInfo,
    metrics: Option<Arc<Metrics>>,
    handler: F,
) -> Result<Json<R>, server::Error>
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Result<Json<R>, server::Error>>,
    R: Serialize,
{
    let result = handler().await;
    let duration = request_info.measure_duration();
    let success = result.is_ok();

    if let Some(metrics) = metrics {
        metrics.record_service_request(request_info, RequestStats { duration, success });
    }
    result
}

pub async fn trace_incoming_stream_request_metrics<'a, F, Fut>(
    request_info: RequestInfo,
    metrics: Option<Arc<Metrics>>,
    handler: F,
) -> Sse<BoxStream<'a, Result<Event, Infallible>>>
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Sse<BoxStream<'a, Result<Event, Infallible>>>>,
{
    let result = handler().await;
    let duration = request_info.measure_duration();
    let success = true; // TODO: probably just remove for stream

    if let Some(metrics) = metrics {
        metrics.record_service_request(&request_info, RequestStats { duration, success });
    }
    result
}

pub async fn trace_outgoing_request_metrics<T, F, Fut>(
    request_info: RequestInfo,
    metrics: Option<Arc<Metrics>>,
    handler: F,
) -> Result<T, clients::Error>
where
    T: Send + 'static,
    F: FnOnce() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Result<T, clients::Error>>,
{
    let result = handler().await;
    let duration = request_info.measure_duration();
    let success = result.is_ok();

    if let Some(metrics) = metrics {
        metrics.record_outgoing_request(&request_info, RequestStats { duration, success });
    }
    result
}

pub async fn trace_outgoing_stream_request_metrics<'a, T, F, Fut>(
    request_info: RequestInfo,
    metrics: Option<Arc<Metrics>>,
    handler: F,
) -> Result<BoxStream<'a, Result<T, orchestrator::Error>>, orchestrator::Error>
where
    T: Serialize,
    F: FnOnce() -> Fut + Send + 'static,
    Fut: std::future::Future<
        Output = Result<BoxStream<'a, Result<T, orchestrator::Error>>, orchestrator::Error>,
    >,
{
    let result = handler().await?;
    let duration = request_info.measure_duration();
    let success = true; // TODO: probably just remove for stream

    if let Some(metrics) = metrics {
        metrics.record_outgoing_request(&request_info, RequestStats { duration, success });
    }
    Ok(result)
}

pub fn stream_close_callback(request_info: RequestInfo, metrics: Option<Arc<Metrics>>) {
    let duration = request_info.measure_duration();
    if let Some(metrics) = metrics {
        metrics.record_stream_duration(&request_info, duration);
    }
}

pub struct InfoIntoRequest<T> {
    client_request_metadata: RequestMetadata,
    inner: T,
}

impl<T> From<(RequestMetadata, T)> for InfoIntoRequest<T> {
    fn from((client_request_metadata, inner): (RequestMetadata, T)) -> Self {
        Self {
            client_request_metadata,
            inner,
        }
    }
}

impl<T> tonic::IntoRequest<T> for InfoIntoRequest<T> {
    fn into_request(self) -> tonic::Request<T> {
        let mut request = tonic::Request::new(self.inner);
        match self.client_request_metadata.clone().kind {
            RequestMetadataKind::Http { .. } => {
                panic!("Cannot convert HTTP metadata to tonic request")
            }
            RequestMetadataKind::Grpc { metadata, .. } => {
                let metadata = metadata.clone();
                for (key, value) in metadata.into_headers() {
                    if let (Some(key), value) = (key.clone(), value.clone()) {
                        if ALLOWED_HEADERS.contains(&key.as_str()) {
                            if let Ok(value) = value.to_str() {
                                if let (Ok(key), Ok(value)) =
                                    (key.to_string().parse::<AsciiMetadataKey>(), value.parse())
                                {
                                    request.metadata_mut().insert(key, value);
                                } else {
                                    error!("Failed to parse a propagated metadata header key to string for tracing");
                                }
                            } else {
                                error!("Failed to read a propagated metadata header value as string for tracing");
                            }
                        }
                    }
                }
            }
        }
        request
    }
}

pub struct TraceParent {
    pub version: u8,
    pub trace_id: TraceId,
    pub parent_id: SpanId,
    pub flags: TraceFlags,
}

impl From<RequestTrace> for TraceParent {
    fn from(trace: RequestTrace) -> Self {
        Self {
            version: 0x00,
            trace_id: trace.trace_id,
            parent_id: trace.context.span_id(),
            flags: TraceFlags::new(0x01),
        }
    }
}

impl Display for TraceParent {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{:02x}-{}-{}-{:02x}",
            self.version,
            self.trace_id,
            self.parent_id,
            self.flags.to_u8()
        )
    }
}
