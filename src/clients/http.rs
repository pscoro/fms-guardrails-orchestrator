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

use hyper::{HeaderMap, http, StatusCode};
use reqwest::Body;
use serde::de::DeserializeOwned;
use serde::Serialize;
use tower::util::BoxCloneService;
use tower::Service;
use tower_http::classify::{NeverClassifyEos, ServerErrorsFailureClass};
use tower_http::trace::{DefaultOnBodyChunk, ResponseBody};
use tower_http_client::ResponseExt;
use tracing::error;
use url::Url;

use crate::clients::Error;
use crate::health::{HealthCheckResult, HealthStatus, OptionalHealthCheckResponseBody};
use crate::tracing_utils::{trace_context_from_http_response, trace_layer, with_traceparent_header};

pub type ServiceRequest = http::Request<Body>;
pub type ServiceResponse = http::Response<ResponseBody<Body, NeverClassifyEos<ServerErrorsFailureClass>, DefaultOnBodyChunk, trace_layer::client::OnEos, trace_layer::client::OnFailure>>;
pub type HttpClientService = BoxCloneService<ServiceRequest, ServiceResponse, Error>;

#[derive(Clone)]
pub struct HttpClient {
    base_url: Url,
    health_url: Url,
    client: HttpClientService,
}

impl HttpClient {
    pub fn new(base_url: Url, client: HttpClientService) -> Self {
        let health_url = base_url.join("health").unwrap();
        Self {
            base_url,
            health_url,
            client,
        }
    }

    pub fn base_url(&self) -> &Url {
        &self.base_url
    }

    async fn call(&self, request: http::Request<Body>) -> Result<ServiceResponse, Error> {
        let mut client = self.client.clone();
        client.call(request).await
    }

    fn add_headers(request: http::request::Builder, headers: HeaderMap) -> http::request::Builder {
        headers.iter().map(|k, v| request.header(k, v)).collect()
    }

    async fn from_response<U: DeserializeOwned + Send>(response: ServiceResponse) -> Result<U, Error> {
        match response.status().as_u16() {
            200 => Ok(response.body_reader().json().await?),
            code => Err(Error::Http {
                code: StatusCode::from_u16(code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
                message: format!("client response {:?} - {:?}", response.status(), response.body_reader().utf8().await?),
            })
        }
    }

    pub async fn get<T: Serialize + Send, U: DeserializeOwned + Send>(&self, url: Url, headers: HeaderMap, body: T) -> Result<U, Error> {
        let headers = with_traceparent_header(headers);
        let response = self.call(
            Self::add_headers(hyper::Request::get(url.as_str()), headers)
                .body(serde_json::to_string(&body)?.into())?
        ).await?;
        trace_context_from_http_response(&response);
        Self::from_response(response).await
    }

    pub async fn post<T: Serialize + Send, U: DeserializeOwned + Send>(&self, url: Url, headers: HeaderMap, body: T) -> Result<U, Error> {
        let headers = with_traceparent_header(headers);
        let response = self.call(
            Self::add_headers(hyper::Request::post(url.as_str()), headers)
                .body(serde_json::to_string(&body)?.into())?
        ).await?;
        trace_context_from_http_response(&response);
        Self::from_response(response).await
    }

    /// This is sectioned off to allow for testing.
    pub(super) async fn http_response_to_health_check_result(
        res: Result<ServiceResponse, Error>,
    ) -> HealthCheckResult {
        match res {
            Ok(response) => {
                if response.status() == StatusCode::OK {
                    if let Ok(body) = response.body_reader().json::<OptionalHealthCheckResponseBody>().await {
                        // If the service provided a body, we only anticipate a minimal health status and optional reason.
                        HealthCheckResult {
                            status: body.status.clone(),
                            code: StatusCode::OK,
                            reason: match body.status {
                                HealthStatus::Healthy => None,
                                _ => body.reason,
                            },
                        }
                    } else {
                        // If the service did not provide a body, we assume it is healthy.
                        HealthCheckResult {
                            status: HealthStatus::Healthy,
                            code: StatusCode::OK,
                            reason: None,
                        }
                    }
                } else {
                    HealthCheckResult {
                        // The most we can presume is that 5xx errors are likely indicating service issues, implying the service is unhealthy.
                        // and that 4xx errors are more likely indicating health check failures, i.e. due to configuration/implementation issues.
                        // Regardless we can't be certain, so the reason is also provided.
                        // TODO: We will likely circle back to re-evaluate this logic in the future
                        // when we know more about how the client health results will be used.
                        status: if response.status().as_u16() >= 500
                            && response.status().as_u16() < 600
                        {
                            HealthStatus::Unhealthy
                        } else if response.status().as_u16() >= 400
                            && response.status().as_u16() < 500
                        {
                            HealthStatus::Unknown
                        } else {
                            error!(
                                "unexpected http health check status code: {}",
                                response.status()
                            );
                            HealthStatus::Unknown
                        },
                        code: response.status(),
                        reason: response.status().canonical_reason().map(|v| v.to_string()),
                    }
                }
            }
            Err(e) => {
                error!("error checking health: {}", e);
                HealthCheckResult {
                    status: HealthStatus::Unknown,
                    code: e.status_code(),
                    reason: Some(e.to_string()),
                }
            }
        }
    }

    pub async fn health(&self) -> HealthCheckResult {
        let res = self.call(
            hyper::Request::get(self.health_url.as_str()).body(Body::default()).unwrap()
        ).await;
        Self::http_response_to_health_check_result(res).await
    }
}

/// Extracts a base url from a url including path segments.
pub fn extract_base_url(url: &Url) -> Option<Url> {
    let mut url = url.clone();
    match url.path_segments_mut() {
        Ok(mut path) => {
            path.clear();
        }
        Err(_) => {
            return None;
        }
    }
    Some(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_base_url() {
        let url =
            Url::parse("https://example-detector.route.example.com/api/v1/text/contents").unwrap();
        let base_url = extract_base_url(&url);
        assert_eq!(
            Some(Url::parse("https://example-detector.route.example.com/").unwrap()),
            base_url
        );
        let health_url = base_url.map(|v| v.join("/health").unwrap());
        assert_eq!(
            Some(Url::parse("https://example-detector.route.example.com/health").unwrap()),
            health_url
        );
    }
}
