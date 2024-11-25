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
use hyper::HeaderMap;
use serde::Serialize;
use tracing::{info, instrument};

use super::{DetectorClient, DetectorClientExt, DEFAULT_PORT};
use crate::{
    clients::{http::HttpClientExt, openai::Message, Client, ClientBuilderExt, Error, HttpClient},
    config::ServiceConfig,
    health::HealthCheckResult,
    models::{DetectionResult, DetectorParams},
};

const CHAT_DETECTOR_ENDPOINT: &str = "/api/v1/text/chat";

#[cfg_attr(test, faux::create)]
#[derive(Clone)]
pub struct TextChatDetectorClient {
    client: HttpClient,
    health_client: HttpClient,
}

#[cfg_attr(test, faux::methods)]
impl TextChatDetectorClient {
    pub async fn new(
        config: &ServiceConfig,
        health_config: Option<&ServiceConfig>,
    ) -> Result<Self, Error> {
        let client = HttpClient::from_config(config)
            .with_default_port(DEFAULT_PORT)
            .build()
            .await?;
        let health_client = if let Some(health_config) = health_config {
            HttpClient::from_config(health_config)
                .with_default_port(DEFAULT_PORT)
                .build()
                .await?
        } else {
            client.clone()
        };
        Ok(Self {
            client,
            health_client,
        })
    }

    fn client(&self) -> &HttpClient {
        &self.client
    }

    #[instrument(skip_all, fields(model_id, ?headers))]
    pub async fn text_chat(
        &self,
        model_id: &str,
        request: ChatDetectionRequest,
        headers: HeaderMap,
    ) -> Result<Vec<DetectionResult>, Error> {
        let url = self.endpoint(CHAT_DETECTOR_ENDPOINT);
        info!("sending text chat detector request to {}", url);
        self.post_to_detector(model_id, url, headers, request).await
    }
}

#[cfg_attr(test, faux::methods)]
#[async_trait]
impl Client for TextChatDetectorClient {
    fn name(&self) -> &str {
        "text_chat_detector"
    }

    async fn health(&self) -> HealthCheckResult {
        self.health_client.health().await
    }
}

#[cfg_attr(test, faux::methods)]
#[async_trait]
impl ClientBuilderExt for TextChatDetectorClient {
    async fn build(
        service_config: &ServiceConfig,
        health_service_config: Option<&ServiceConfig>,
    ) -> Result<Self, Error> {
        Self::new(service_config, health_service_config).await
    }
}

#[cfg_attr(test, faux::methods)]
impl DetectorClient for TextChatDetectorClient {}

#[cfg_attr(test, faux::methods)]
impl HttpClientExt for TextChatDetectorClient {
    fn inner(&self) -> &HttpClient {
        self.client()
    }
}

/// A struct representing a request to a detector compatible with the
/// /api/v1/text/chat endpoint.
// #[cfg_attr(test, derive(PartialEq))]
#[derive(Debug, Clone, Serialize)]
pub struct ChatDetectionRequest {
    /// Chat messages to run detection on
    pub messages: Vec<Message>,

    /// Detector parameters (available parameters depend on the detector)
    pub detector_params: DetectorParams,
}

impl ChatDetectionRequest {
    pub fn new(messages: Vec<Message>, detector_params: DetectorParams) -> Self {
        Self {
            messages,
            detector_params,
        }
    }
}
