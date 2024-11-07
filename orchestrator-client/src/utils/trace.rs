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
use std::fmt::Display;
use std::time::Duration;
use hyper::http;
use hyper_util::rt::TokioExecutor;
use opentelemetry::{
    global,
    metrics::MetricsError,
    trace::{TraceError, TracerProvider},
    KeyValue,
};
use opentelemetry::trace::{TraceContextExt, TraceId};
use opentelemetry_http::HeaderInjector;
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::{
    metrics::SdkMeterProvider,
    propagation::TraceContextPropagator,
    runtime,
    trace::{Config, Sampler},
    Resource,
};
use serde::Serialize;
use tracing::{error, info, Span, warn};
use tracing_opentelemetry::{MetricsLayer, OpenTelemetrySpanExt};
use tracing_subscriber::{layer::SubscriberExt, EnvFilter, Layer};

mod defaults {
    pub const DEFAULT_HTTP_OTLP_ENDPOINT: &str = "http://localhost:4318";
    pub const DEFAULT_GRPC_OTLP_ENDPOINT: &str = "http://localhost:4317";
    pub const DEFAULT_TRACE_TIMEOUT_SEC: u64 = 10;
    pub const DEFAULT_METRIC_TIMEOUT_SEC: u64 = 3;
    pub const DEFAULT_METRIC_PERIOD_SEC: u64 = 3;
}
use defaults::*;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OtlpExport {
    Traces,
    Metrics,
}

impl Display for OtlpExport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OtlpExport::Traces => write!(f, "traces"),
            OtlpExport::Metrics => write!(f, "metrics"),
        }
    }
}

impl From<String> for OtlpExport {
    fn from(s: String) -> Self {
        match s.to_lowercase().as_str() {
            "traces" => OtlpExport::Traces,
            "metrics" => OtlpExport::Metrics,
            _ => panic!(
                "Invalid OTLP export type {}, orchestrator only supports exporting traces and metrics via OTLP",
                s
            ),
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub enum OtlpProtocol {
    #[default]
    Grpc,
    Http,
}

impl Display for OtlpProtocol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OtlpProtocol::Grpc => write!(f, "grpc"),
            OtlpProtocol::Http => write!(f, "http"),
        }
    }
}

impl From<String> for OtlpProtocol {
    fn from(s: String) -> Self {
        match s.to_lowercase().as_str() {
            "grpc" => OtlpProtocol::Grpc,
            "http" => OtlpProtocol::Http,
            _ => {
                error!(
                    "Invalid OTLP protocol {}, defaulting to {}",
                    s,
                    OtlpProtocol::default()
                );
                OtlpProtocol::default()
            }
        }
    }
}

impl OtlpProtocol {
    pub fn default_endpoint(&self) -> &str {
        match self {
            OtlpProtocol::Http => DEFAULT_HTTP_OTLP_ENDPOINT,
            OtlpProtocol::Grpc => DEFAULT_GRPC_OTLP_ENDPOINT,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LogFormat {
    #[default]
    Full,
    Compact,
    Pretty,
    JSON,
}

impl Display for LogFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", serde_json::to_string(&self)?)
    }
}

impl From<String> for LogFormat {
    fn from(s: String) -> Self {
        match s.to_lowercase().as_str() {
            "full" => LogFormat::Full,
            "compact" => LogFormat::Compact,
            "pretty" => LogFormat::Pretty,
            "json" => LogFormat::JSON,
            _ => {
                warn!(
                    "Invalid trace format {}, defaulting to {}",
                    s,
                    LogFormat::default()
                );
                LogFormat::default()
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct TracingConfig {
    pub service_name: String,
    pub traces: Option<(OtlpProtocol, String)>,
    pub metrics: Option<(OtlpProtocol, String)>,
    pub log_format: LogFormat,
    pub quiet: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum TracingError {
    #[error("Error from tracing provider: {0}")]
    TraceError(#[from] TraceError),
    #[error("Error from metrics provider: {0}")]
    MetricsError(#[from] MetricsError),
}

fn service_config(tracing_config: TracingConfig) -> Config {
    Config::default()
        .with_resource(Resource::new(vec![KeyValue::new(
            "service.name",
            tracing_config.service_name,
        )]))
        .with_sampler(Sampler::AlwaysOn)
}

/// Initializes an OpenTelemetry tracer provider with an OTLP export pipeline based on the
/// provided config.
fn init_tracer_provider(
    tracing_config: TracingConfig,
) -> Result<Option<opentelemetry_sdk::trace::TracerProvider>, TracingError> {
    if let Some((protocol, endpoint)) = tracing_config.clone().traces {
        Ok(Some(
            match protocol {
                OtlpProtocol::Grpc => opentelemetry_otlp::new_pipeline().tracing().with_exporter(
                    opentelemetry_otlp::new_exporter()
                        .tonic()
                        .with_endpoint(endpoint)
                        .with_timeout(Duration::from_secs(3)),
                ),
                OtlpProtocol::Http => opentelemetry_otlp::new_pipeline().tracing().with_exporter(
                    opentelemetry_otlp::new_exporter()
                        .http()
                        .with_http_client(opentelemetry_http::hyper::HyperClient::new_with_timeout(
                            hyper_util::client::legacy::Client::builder(TokioExecutor::new()).build(
                                hyper_util::client::legacy::connect::HttpConnector::new()
                            ),
                            Duration::from_secs(DEFAULT_TRACE_TIMEOUT_SEC),
                        ))
                        .with_endpoint(endpoint)
                        .with_timeout(Duration::from_secs(DEFAULT_TRACE_TIMEOUT_SEC)),
                ),
            }
            .with_trace_config(service_config(tracing_config))
            .install_batch(runtime::Tokio)?,
        ))
    } else if !tracing_config.quiet {
        // We still need a tracing provider as long as we are logging in order to enable any
        // trace-sensitive logs, such as any mentions of a request's trace_id.
        Ok(Some(
            opentelemetry_sdk::trace::TracerProvider::builder()
                .with_config(service_config(tracing_config))
                .build(),
        ))
    } else {
        Ok(None)
    }
}

/// Initializes an OpenTelemetry meter provider with an OTLP export pipeline based on the
/// provided config.
fn init_meter_provider(
    tracing_config: TracingConfig,
) -> Result<Option<SdkMeterProvider>, TracingError> {
    if let Some((protocol, endpoint)) = tracing_config.metrics {
        Ok(Some(
            match protocol {
                OtlpProtocol::Grpc => opentelemetry_otlp::new_pipeline()
                    .metrics(runtime::Tokio)
                    .with_exporter(
                        opentelemetry_otlp::new_exporter()
                            .tonic()
                            .with_endpoint(endpoint),
                    ),
                OtlpProtocol::Http => opentelemetry_otlp::new_pipeline()
                    .metrics(runtime::Tokio)
                    .with_exporter(
                        opentelemetry_otlp::new_exporter()
                            .http()
                            .with_http_client(opentelemetry_http::hyper::HyperClient::new_with_timeout(
                                hyper_util::client::legacy::Client::builder(TokioExecutor::new()).build(
                                    hyper_util::client::legacy::connect::HttpConnector::new()
                                ),
                                Duration::from_secs(DEFAULT_METRIC_TIMEOUT_SEC),
                            ))
                            .with_endpoint(endpoint)
                            .with_timeout(Duration::from_secs(DEFAULT_METRIC_TIMEOUT_SEC))
                    ),
            }
            .with_resource(Resource::new(vec![KeyValue::new(
                "service.name",
                tracing_config.service_name,
            )]))
            .with_timeout(Duration::from_secs(DEFAULT_METRIC_TIMEOUT_SEC))
            .with_period(Duration::from_secs(DEFAULT_METRIC_PERIOD_SEC))
            .build()?,
        ))
    } else {
        Ok(None)
    }
}

/// Initializes tracing for the orchestrator using the OpenTelemetry API/SDK and the `tracing`
/// crate. What telemetry is exported and to where is determined based on the provided config
pub fn init_tracing(
    tracing_config: TracingConfig,
) -> Result<impl FnOnce() -> Result<(), TracingError>, TracingError> {
    let mut layers = Vec::new();
    global::set_text_map_propagator(TraceContextPropagator::new());

    // TODO: Find a better way to only propagate errors from other crates
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or(EnvFilter::new("INFO"))
        .add_directive("ginepro=info".parse().unwrap())
        .add_directive("hyper=error".parse().unwrap())
        .add_directive("h2=error".parse().unwrap())
        .add_directive("trust_dns_resolver=error".parse().unwrap())
        .add_directive("trust_dns_proto=error".parse().unwrap())
        .add_directive("tower=error".parse().unwrap())
        .add_directive("tonic=error".parse().unwrap())
        .add_directive("reqwest=error".parse().unwrap());

    // Set up tracing layer with OTLP exporter
    let trace_provider = init_tracer_provider(tracing_config.clone())?;
    if let Some(tracer_provider) = trace_provider.clone() {
        global::set_tracer_provider(tracer_provider.clone());
        layers.push(
            tracing_opentelemetry::layer()
                .with_tracer(tracer_provider.tracer(tracing_config.service_name.clone()))
                .boxed(),
        );
    }

    // Set up metrics layer with OTLP exporter
    let meter_provider = init_meter_provider(tracing_config.clone())?;
    if let Some(meter_provider) = meter_provider.clone() {
        global::set_meter_provider(meter_provider.clone());
        layers.push(MetricsLayer::new(meter_provider).boxed());
    }

    // Set up formatted layer for logging to stdout
    // Because we use the `tracing` crate for logging, all logs are traces and will be exported
    // to OTLP if `--otlp-export=traces` is set.
    if !tracing_config.quiet {
        match tracing_config.log_format {
            LogFormat::Full => layers.push(tracing_subscriber::fmt::layer().boxed()),
            LogFormat::Compact => layers.push(tracing_subscriber::fmt::layer().compact().boxed()),
            LogFormat::Pretty => layers.push(tracing_subscriber::fmt::layer().pretty().boxed()),
            LogFormat::JSON => layers.push(
                tracing_subscriber::fmt::layer()
                    .json()
                    .flatten_event(true)
                    .boxed(),
            ),
        }
    }

    let subscriber = tracing_subscriber::registry().with(filter).with(layers);
    tracing::subscriber::set_global_default(subscriber).unwrap();

    if let Some(traces) = tracing_config.traces {
        info!(
            "OTLP tracing enabled: Exporting {} to {}",
            traces.0, traces.1
        );
    } else {
        info!("OTLP traces export disabled")
    }

    if let Some(metrics) = tracing_config.metrics {
        info!(
            "OTLP metrics enabled: Exporting {} to {}",
            metrics.0, metrics.1
        );
    } else {
        info!("OTLP metrics export disabled")
    }

    if !tracing_config.quiet {
        info!(
            "Stdout logging enabled with format {}",
            tracing_config.log_format
        );
    } else {
        info!("Stdout logging disabled"); // This will only be visible in traces
    }

    Ok(move || {
        global::shutdown_tracer_provider();
        if let Some(meter_provider) = meter_provider {
            meter_provider
                .shutdown()
                .map_err(TracingError::MetricsError)?;
        }
        Ok(())
    })
}

/// Injects the `traceparent` header into the header map from the current tracing span context.
/// Also injects empty `tracestate` header by default. This can be used to propagate
/// vendor-specific trace context.
/// Used by both gRPC and HTTP requests since `tonic::Metadata` uses `http::HeaderMap`.
/// See https://www.w3.org/TR/trace-context/#trace-context-http-headers-format.
pub fn with_traceparent_header(headers: http::HeaderMap) -> http::HeaderMap {
    let mut headers = headers.clone();
    let ctx = Span::current().context();
    global::get_text_map_propagator(|propagator| {
        // Injects current `traceparent` (and by default empty `tracestate`)
        propagator.inject_context(&ctx, &mut HeaderInjector(&mut headers))
    });
    headers
}

pub fn current_trace_id() -> TraceId {
    Span::current().context().span().span_context().trace_id()
}

pub fn set_span_context(ctx: opentelemetry::Context) {
    Span::current().set_parent(ctx);
}