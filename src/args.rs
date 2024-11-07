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

use std::{fmt::Display, path::PathBuf};

use clap::Parser;
use tracing::{error, warn};
use orchestr8_client::utils::trace::{OtlpExport, TracingConfig};

#[derive(Parser, Debug, Clone)]
#[clap(author, version, about, long_about = None)]
pub struct Args {
    #[clap(default_value = "8033", long, env)]
    pub http_port: u16,
    #[clap(default_value = "8034", long, env)]
    pub health_http_port: u16,
    #[clap(
        default_value = "config/config.yaml",
        long,
        env = "ORCHESTRATOR_CONFIG"
    )]
    pub config_path: PathBuf,
    #[clap(long, env)]
    pub tls_cert_path: Option<PathBuf>,
    #[clap(long, env)]
    pub tls_key_path: Option<PathBuf>,
    #[clap(long, env)]
    pub tls_client_ca_cert_path: Option<PathBuf>,
    #[clap(default_value = "false", long, env)]
    pub start_up_health_check: bool,
    #[clap(long, env, value_delimiter = ',')]
    pub otlp_export: Vec<OtlpExport>,
    #[clap(default_value_t = LogFormat::default(), long, env)]
    pub log_format: LogFormat,
    #[clap(default_value_t = false, long, short, env)]
    pub quiet: bool,
    #[clap(default_value = "fms_guardrails_orchestr8", long, env)]
    pub otlp_service_name: String,
    #[clap(long, env = "OTEL_EXPORTER_OTLP_ENDPOINT")]
    pub otlp_endpoint: Option<String>,
    #[clap(long, env = "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT")]
    pub otlp_traces_endpoint: Option<String>,
    #[clap(long, env = "OTEL_EXPORTER_OTLP_METRICS_ENDPOINT")]
    pub otlp_metrics_endpoint: Option<String>,
    #[clap(
        default_value_t = OtlpProtocol::Grpc,
        long,
        env = "OTEL_EXPORTER_OTLP_PROTOCOL"
    )]
    pub otlp_protocol: OtlpProtocol,
    #[clap(long, env = "OTEL_EXPORTER_OTLP_TRACES_PROTOCOL")]
    pub otlp_traces_protocol: Option<OtlpProtocol>,
    #[clap(long, env = "OTEL_EXPORTER_OTLP_METRICS_PROTOCOL")]
    pub otlp_metrics_protocol: Option<OtlpProtocol>,
    // TODO: Add timeout and header OTLP variables
}

impl From<Args> for TracingConfig {
    fn from(args: Args) -> Self {
        let otlp_protocol = args.otlp_protocol;
        let otlp_endpoint = args
            .otlp_endpoint
            .unwrap_or(otlp_protocol.default_endpoint().to_string());
        let otlp_traces_endpoint = args.otlp_traces_endpoint.unwrap_or(otlp_endpoint.clone());
        let otlp_metrics_endpoint = args.otlp_metrics_endpoint.unwrap_or(otlp_endpoint.clone());
        let otlp_traces_protocol = args.otlp_traces_protocol.unwrap_or(otlp_protocol);
        let otlp_metrics_protocol = args.otlp_metrics_protocol.unwrap_or(otlp_protocol);

        TracingConfig {
            service_name: args.otlp_service_name,
            traces: match args.otlp_export.contains(&OtlpExport::Traces) {
                true => Some((otlp_traces_protocol, otlp_traces_endpoint)),
                false => None,
            },
            metrics: match args.otlp_export.contains(&OtlpExport::Metrics) {
                true => Some((otlp_metrics_protocol, otlp_metrics_endpoint)),
                false => None,
            },
            log_format: args.log_format,
            quiet: args.quiet,
        }
    }
}

