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
use hyper::{HeaderMap, StatusCode};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, instrument};

use super::{DetectorClient, DetectorClientExt, DetectorError, DEFAULT_PORT};
use crate::{
    clients::{create_http_client, http::HttpClientExt, Client, Error, HttpClient},
    config::ServiceConfig,
    health::HealthCheckResult,
    models::{DetectionResult, DetectorParams},
};

#[cfg_attr(test, faux::create)]
#[derive(Clone)]
pub struct TextContextDocDetectorClient {
    client: HttpClient,
    health_client: Option<HttpClient>,
}

#[cfg_attr(test, faux::methods)]
impl TextContextDocDetectorClient {
    pub async fn new(
        config: &ServiceConfig,
        health_config: Option<&ServiceConfig>,
    ) -> Result<Self, Error> {
        let client = create_http_client(DEFAULT_PORT, config).await?;
        let health_client = if let Some(health_config) = health_config {
            Some(create_http_client(DEFAULT_PORT, health_config).await?)
        } else {
            None
        };
        Ok(Self {
            client,
            health_client,
        })
    }

    fn client(&self) -> &HttpClient {
        &self.client
    }

    #[instrument(skip_all, fields(model_id))]
    pub async fn text_context_doc(
        &self,
        model_id: &str,
        request: ContextDocsDetectionRequest,
        headers: HeaderMap,
    ) -> Result<Vec<DetectionResult>, Error> {
        let url = self
            .client
            .base_url()
            .join("/api/v1/text/context/doc")
            .unwrap();
        info!(?url, "sending context doc detector request");
        debug!(?request, "text context doc detector request");
        let response = self
            .post_with_model_id(model_id, url, headers, request)
            .await?;
        if response.status() == StatusCode::OK {
            let response = response.json().await?;
            debug!(?response, "text context doc detector response");
            Ok(response)
        } else {
            let code = response.status().as_u16();
            let error = response
                .json::<DetectorError>()
                .await
                .unwrap_or(DetectorError {
                    code,
                    message: "".into(),
                });
            Err(error.into())
        }
    }
}

#[cfg_attr(test, faux::methods)]
#[async_trait]
impl Client for TextContextDocDetectorClient {
    fn name(&self) -> &str {
        "text_context_doc_detector"
    }

    async fn health(&self) -> HealthCheckResult {
        if let Some(health_client) = &self.health_client {
            health_client.health().await
        } else {
            self.client.health().await
        }
    }
}

impl DetectorClient for TextContextDocDetectorClient {}

impl HttpClientExt for TextContextDocDetectorClient {
    fn inner(&self) -> &HttpClient {
        self.client()
    }
}

/// A struct representing a request to a detector compatible with the
/// /api/v1/text/context/doc endpoint.
#[cfg_attr(test, derive(PartialEq))]
#[derive(Debug, Clone, Serialize)]
pub struct ContextDocsDetectionRequest {
    /// Content to run detection on
    pub content: String,

    /// Type of context being sent
    pub context_type: ContextType,

    /// Context to run detection on
    pub context: Vec<String>,

    /// Detector parameters (available parameters depend on the detector)
    pub detector_params: DetectorParams,
}

/// Enum representing the context type of a detection
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ContextType {
    #[serde(rename = "docs")]
    Document,
    #[serde(rename = "url")]
    Url,
}

impl ContextDocsDetectionRequest {
    pub fn new(
        content: String,
        context_type: ContextType,
        context: Vec<String>,
        detector_params: DetectorParams,
    ) -> Self {
        Self {
            content,
            context_type,
            context,
            detector_params,
        }
    }
}
