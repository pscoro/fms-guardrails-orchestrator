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

use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::PathBuf,
};

use clap::Parser;
use fms_guardrails_orchestr8::tracing_utils::Metrics;
use fms_guardrails_orchestr8::{
    config::OrchestratorConfig, orchestrator::Orchestrator, server, tracing_utils::init_tracer,
};
use opentelemetry::global;

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    #[clap(default_value = "8033", long, env)]
    http_port: u16,
    #[clap(default_value = "8034", long, env)]
    health_http_port: u16,
    #[clap(long, env)]
    json_output: bool,
    #[clap(default_value = "config/config.yaml", long, env)]
    config_path: PathBuf,
    #[clap(long, env)]
    tls_cert_path: Option<PathBuf>,
    #[clap(long, env)]
    tls_key_path: Option<PathBuf>,
    #[clap(long, env)]
    tls_client_ca_cert_path: Option<PathBuf>,
    #[clap(default_value = "true", long, env)] // Do we want this to be true by default?
    start_up_health_check: bool,
    #[clap(long, env = "OTEL_EXPORTER_OTLP_ENDPOINT")]
    otlp_endpoint: Option<String>,
    #[clap(long, env = "OTEL_SERVICE_NAME", default_value = "orchestrator")]
    otlp_service_name: String,
}

fn main() -> Result<(), anyhow::Error> {
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .expect("Failed to install rustls crypto provider");

    let args = Args::parse();
    if args.tls_key_path.is_some() != args.tls_cert_path.is_some() {
        panic!("tls: must provide both cert and key")
    }
    if args.tls_client_ca_cert_path.is_some() && args.tls_cert_path.is_none() {
        panic!("tls: cannot provide client ca cert without keypair")
    }

    let http_addr: SocketAddr =
        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), args.http_port);
    let health_http_addr: SocketAddr =
        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), args.health_http_port);

    let meter = global::meter("guardrails-orchestrator");

    // Launch Tokio runtime
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            init_tracer(args.otlp_service_name, args.json_output, args.otlp_endpoint);
            let metrics = Metrics::new(&meter);
            let config = OrchestratorConfig::load(args.config_path).await?;
            let orchestrator = Orchestrator::new(config, metrics.clone(), args.start_up_health_check).await?;

            server::run(
                http_addr,
                health_http_addr,
                args.tls_cert_path,
                args.tls_key_path,
                args.tls_client_ca_cert_path,
                orchestrator,
                Some(metrics),
            )
            .await?;
            Ok(())
        })
}
