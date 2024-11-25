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

use async_trait::async_trait;
use axum::http::HeaderMap;
use futures::{StreamExt, TryStreamExt};
use ginepro::LoadBalancedChannel;
use tonic::{Code, Request};
use tracing::{info, instrument};

use super::{
    errors::grpc_to_http_code, grpc::GrpcClientBuilder, grpc_request_with_headers, BoxStream,
    Client, ClientBuilderExt, Error,
};
use crate::{
    config::ServiceConfig,
    health::{HealthCheckResult, HealthStatus},
    pb::{
        caikit::runtime::nlp::{
            nlp_service_client::NlpServiceClient, ServerStreamingTextGenerationTaskRequest,
            TextGenerationTaskRequest, TokenClassificationTaskRequest, TokenizationTaskRequest,
        },
        caikit_data_model::nlp::{
            GeneratedTextResult, GeneratedTextStreamResult, TokenClassificationResults,
            TokenizationResults,
        },
        grpc::health::v1::{health_client::HealthClient, HealthCheckRequest},
    },
    utils::trace::trace_context_from_grpc_response,
};

const DEFAULT_PORT: u16 = 8085;
const MODEL_ID_HEADER_NAME: &str = "mm-model-id";

#[cfg_attr(test, faux::create)]
#[derive(Clone)]
pub struct NlpClient {
    client: NlpServiceClient<LoadBalancedChannel>,
    health_client: HealthClient<LoadBalancedChannel>,
}

#[cfg_attr(test, faux::methods)]
impl NlpClient {
    pub async fn new(config: &ServiceConfig) -> Result<Self, Error> {
        let client = GrpcClientBuilder::from_config(config)
            .with_default_port(DEFAULT_PORT)
            .with_new_fn(NlpServiceClient::new)
            .build()
            .await?;
        let health_client = GrpcClientBuilder::from_config(config)
            .with_default_port(DEFAULT_PORT)
            .with_new_fn(HealthClient::new)
            .build()
            .await?;

        Ok(Self {
            client,
            health_client,
        })
    }

    #[instrument(skip_all, fields(model_id))]
    pub async fn tokenization_task_predict(
        &self,
        model_id: &str,
        request: TokenizationTaskRequest,
        headers: HeaderMap,
    ) -> Result<TokenizationResults, Error> {
        let mut client = self.client.clone();
        let request = request_with_headers(request, model_id, headers);
        info!(?request, "sending request to NLP gRPC service");
        let response = client.tokenization_task_predict(request).await?;
        trace_context_from_grpc_response(&response);
        Ok(response.into_inner())
    }

    #[instrument(skip_all, fields(model_id))]
    pub async fn token_classification_task_predict(
        &self,
        model_id: &str,
        request: TokenClassificationTaskRequest,
        headers: HeaderMap,
    ) -> Result<TokenClassificationResults, Error> {
        let mut client = self.client.clone();
        let request = request_with_headers(request, model_id, headers);
        info!(?request, "sending request to NLP gRPC service");
        let response = client.token_classification_task_predict(request).await?;
        trace_context_from_grpc_response(&response);
        Ok(response.into_inner())
    }

    #[instrument(skip_all, fields(model_id))]
    pub async fn text_generation_task_predict(
        &self,
        model_id: &str,
        request: TextGenerationTaskRequest,
        headers: HeaderMap,
    ) -> Result<GeneratedTextResult, Error> {
        let mut client = self.client.clone();
        let request = request_with_headers(request, model_id, headers);
        info!(?request, "sending request to NLP gRPC service");
        let response = client.text_generation_task_predict(request).await?;
        trace_context_from_grpc_response(&response);
        Ok(response.into_inner())
    }

    #[instrument(skip_all, fields(model_id))]
    pub async fn server_streaming_text_generation_task_predict(
        &self,
        model_id: &str,
        request: ServerStreamingTextGenerationTaskRequest,
        headers: HeaderMap,
    ) -> Result<BoxStream<Result<GeneratedTextStreamResult, Error>>, Error> {
        let mut client = self.client.clone();
        let request = request_with_headers(request, model_id, headers);
        info!(?request, "sending stream request to NLP gRPC service");
        let response = client
            .server_streaming_text_generation_task_predict(request)
            .await?;
        trace_context_from_grpc_response(&response);
        let response_stream = response.into_inner().map_err(Into::into).boxed();
        Ok(response_stream)
    }
}

#[cfg_attr(test, faux::methods)]
#[async_trait]
impl ClientBuilderExt for NlpClient {
    async fn build(
        service_config: &ServiceConfig,
        _health_service_config: Option<&ServiceConfig>,
    ) -> Result<Self, Error> {
        Self::new(service_config).await
    }
}

#[cfg_attr(test, faux::methods)]
#[async_trait]
impl Client for NlpClient {
    fn name(&self) -> &str {
        "nlp"
    }

    async fn health(&self) -> HealthCheckResult {
        let mut client = self.health_client.clone();
        let response = client
            .check(HealthCheckRequest { service: "".into() })
            .await;
        let code = match response {
            Ok(_) => Code::Ok,
            Err(status) if matches!(status.code(), Code::InvalidArgument | Code::NotFound) => {
                Code::Ok
            }
            Err(status) => status.code(),
        };
        let status = if matches!(code, Code::Ok) {
            HealthStatus::Healthy
        } else {
            HealthStatus::Unhealthy
        };
        HealthCheckResult {
            status,
            code: grpc_to_http_code(code),
            reason: None,
        }
    }
}

/// Turns an NLP client gRPC request body of type `T` and headers into a `tonic::Request<T>`.
/// Also injects provided `model_id` and `traceparent` from current context into headers.
fn request_with_headers<T>(request: T, model_id: &str, headers: HeaderMap) -> Request<T> {
    let mut request = grpc_request_with_headers(request, headers);
    request
        .metadata_mut()
        .insert(MODEL_ID_HEADER_NAME, model_id.parse().unwrap());
    request
}
