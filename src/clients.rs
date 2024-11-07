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

#![allow(dead_code)]
use std::{
    any::TypeId,
    collections::{hash_map, HashMap},
    pin::Pin,
    time::Duration,
};

use async_trait::async_trait;
use axum::http::{Extensions, HeaderMap};
use futures::Stream;
use ginepro::LoadBalancedChannel;
use hyper_util::rt::TokioIo;
use rustls::RootCertStore;
use tokio::{fs::File, io::AsyncReadExt};
use tokio::net::TcpStream;
use tonic::{metadata::MetadataMap, Request};
use tracing::{debug, instrument};
use url::Url;

use crate::{
    config::{ServiceConfig, Tls},
    health::HealthCheckResult,
    tracing_utils::with_traceparent_header,
};

pub mod errors;
pub use errors::Error;

pub mod http;
pub use http::HttpClient;

pub mod chunker;

pub mod detector;
pub use detector::TextContentsDetectorClient;

pub mod tgis;
pub use tgis::TgisClient;

pub mod nlp;
pub use nlp::NlpClient;

pub mod generation;
pub use generation::GenerationClient;

pub mod openai;

const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(60);
const DEFAULT_REQUEST_TIMEOUT_SEC: u64 = 600;

fn foo() {
}

#[cfg(test)]
mod tests {
    use errors::grpc_to_http_code;
    use hyper::{http, StatusCode};
    use reqwest::Response;

    use super::*;
    use crate::{
        health::{HealthCheckResult, HealthStatus},
        pb::grpc::health::v1::{health_check_response::ServingStatus, HealthCheckResponse},
    };

    async fn mock_http_response(
        status: StatusCode,
        body: &str,
    ) -> Result<Response, reqwest::Error> {
        Ok(reqwest::Response::from(
            http::Response::builder()
                .status(status)
                .body(body.to_string())
                .unwrap(),
        ))
    }

    async fn mock_grpc_response(
        health_status: Option<i32>,
        tonic_status: Option<tonic::Status>,
    ) -> Result<tonic::Response<HealthCheckResponse>, tonic::Status> {
        match health_status {
            Some(health_status) => Ok(tonic::Response::new(HealthCheckResponse {
                status: health_status,
            })),
            None => Err(tonic_status
                .expect("tonic_status must be provided for test if health_status is None")),
        }
    }

    #[tokio::test]
    async fn test_http_health_check_responses() {
        // READY responses from HTTP 200 OK with or without reason
        let response = [
            (StatusCode::OK, r#"{}"#),
            (StatusCode::OK, r#"{ "status": "HEALTHY" }"#),
            (StatusCode::OK, r#"{ "status": "meaningless status" }"#),
            (
                StatusCode::OK,
                r#"{ "status": "HEALTHY", "reason": "needless reason" }"#,
            ),
        ];
        for (status, body) in response.iter() {
            let response = mock_http_response(*status, body).await;
            let result = HttpClient::http_response_to_health_check_result(response).await;
            assert_eq!(result.status, HealthStatus::Healthy);
            assert_eq!(result.code, StatusCode::OK);
            assert_eq!(result.reason, None);
            let serialized = serde_json::to_string(&result).unwrap();
            assert_eq!(serialized, r#"{"status":"HEALTHY"}"#);
        }

        // NOT_READY response from HTTP 200 OK without reason
        let response = mock_http_response(StatusCode::OK, r#"{ "status": "UNHEALTHY" }"#).await;
        let result = HttpClient::http_response_to_health_check_result(response).await;
        assert_eq!(result.status, HealthStatus::Unhealthy);
        assert_eq!(result.code, StatusCode::OK);
        assert_eq!(result.reason, None);
        let serialized = serde_json::to_string(&result).unwrap();
        assert_eq!(serialized, r#"{"status":"UNHEALTHY"}"#);

        // UNKNOWN response from HTTP 200 OK without reason
        let response = mock_http_response(StatusCode::OK, r#"{ "status": "UNKNOWN" }"#).await;
        let result = HttpClient::http_response_to_health_check_result(response).await;
        assert_eq!(result.status, HealthStatus::Unknown);
        assert_eq!(result.code, StatusCode::OK);
        assert_eq!(result.reason, None);
        let serialized = serde_json::to_string(&result).unwrap();
        assert_eq!(serialized, r#"{"status":"UNKNOWN"}"#);

        // NOT_READY response from HTTP 200 OK with reason
        let response = mock_http_response(
            StatusCode::OK,
            r#"{"status": "UNHEALTHY", "reason": "some reason" }"#,
        )
        .await;
        let result = HttpClient::http_response_to_health_check_result(response).await;
        assert_eq!(result.status, HealthStatus::Unhealthy);
        assert_eq!(result.code, StatusCode::OK);
        assert_eq!(result.reason, Some("some reason".to_string()));
        let serialized = serde_json::to_string(&result).unwrap();
        assert_eq!(
            serialized,
            r#"{"status":"UNHEALTHY","reason":"some reason"}"#
        );

        // UNKNOWN response from HTTP 200 OK with reason
        let response = mock_http_response(
            StatusCode::OK,
            r#"{ "status": "UNKNOWN", "reason": "some reason" }"#,
        )
        .await;
        let result = HttpClient::http_response_to_health_check_result(response).await;
        assert_eq!(result.status, HealthStatus::Unknown);
        assert_eq!(result.code, StatusCode::OK);
        assert_eq!(result.reason, Some("some reason".to_string()));
        let serialized = serde_json::to_string(&result).unwrap();
        assert_eq!(serialized, r#"{"status":"UNKNOWN","reason":"some reason"}"#);

        // NOT_READY response from HTTP 503 SERVICE UNAVAILABLE with reason
        let response = mock_http_response(
            StatusCode::SERVICE_UNAVAILABLE,
            r#"{ "message": "some error message" }"#,
        )
        .await;
        let result = HttpClient::http_response_to_health_check_result(response).await;
        assert_eq!(result.status, HealthStatus::Unhealthy);
        assert_eq!(result.code, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(result.reason, Some("Service Unavailable".to_string()));
        let serialized = serde_json::to_string(&result).unwrap();
        assert_eq!(
            serialized,
            r#"{"status":"UNHEALTHY","code":503,"reason":"Service Unavailable"}"#
        );

        // UNKNOWN response from HTTP 404 NOT FOUND with reason
        let response = mock_http_response(
            StatusCode::NOT_FOUND,
            r#"{ "message": "service not found" }"#,
        )
        .await;
        let result = HttpClient::http_response_to_health_check_result(response).await;
        assert_eq!(result.status, HealthStatus::Unknown);
        assert_eq!(result.code, StatusCode::NOT_FOUND);
        assert_eq!(result.reason, Some("Not Found".to_string()));
        let serialized = serde_json::to_string(&result).unwrap();
        assert_eq!(
            serialized,
            r#"{"status":"UNKNOWN","code":404,"reason":"Not Found"}"#
        );

        // NOT_READY response from HTTP 500 INTERNAL SERVER ERROR without reason
        let response = mock_http_response(StatusCode::INTERNAL_SERVER_ERROR, r#""#).await;
        let result = HttpClient::http_response_to_health_check_result(response).await;
        assert_eq!(result.status, HealthStatus::Unhealthy);
        assert_eq!(result.code, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(result.reason, Some("Internal Server Error".to_string()));
        let serialized = serde_json::to_string(&result).unwrap();
        assert_eq!(
            serialized,
            r#"{"status":"UNHEALTHY","code":500,"reason":"Internal Server Error"}"#
        );

        // UNKNOWN response from HTTP 400 BAD REQUEST without reason
        let response = mock_http_response(StatusCode::BAD_REQUEST, r#""#).await;
        let result = HttpClient::http_response_to_health_check_result(response).await;
        assert_eq!(result.status, HealthStatus::Unknown);
        assert_eq!(result.code, StatusCode::BAD_REQUEST);
        assert_eq!(result.reason, Some("Bad Request".to_string()));
        let serialized = serde_json::to_string(&result).unwrap();
        assert_eq!(
            serialized,
            r#"{"status":"UNKNOWN","code":400,"reason":"Bad Request"}"#
        );
    }

    #[tokio::test]
    async fn test_grpc_health_check_responses() {
        // READY responses from gRPC 0 OK from serving status 1 SERVING
        let response = mock_grpc_response(Some(ServingStatus::Serving as i32), None).await;
        let result = HealthCheckResult::from(response);
        assert_eq!(result.status, HealthStatus::Healthy);
        assert_eq!(result.code, grpc_to_http_code(tonic::Code::Ok));
        assert_eq!(result.reason, None);
        let serialized = serde_json::to_string(&result).unwrap();
        assert_eq!(serialized, r#"{"status":"HEALTHY"}"#);

        // NOT_READY response from gRPC 0 OK form serving status 2 NOT_SERVING
        let response = mock_grpc_response(Some(ServingStatus::NotServing as i32), None).await;
        let result = HealthCheckResult::from(response);
        assert_eq!(result.status, HealthStatus::Unhealthy);
        assert_eq!(result.code, grpc_to_http_code(tonic::Code::Ok));
        assert_eq!(result.reason, Some("NOT_SERVING".to_string()));
        let serialized = serde_json::to_string(&result).unwrap();
        assert_eq!(
            serialized,
            r#"{"status":"UNHEALTHY","reason":"NOT_SERVING"}"#
        );

        // UNKNOWN response from gRPC 0 OK from serving status 0 UNKNOWN
        let response = mock_grpc_response(Some(ServingStatus::Unknown as i32), None).await;
        let result = HealthCheckResult::from(response);
        assert_eq!(result.status, HealthStatus::Unknown);
        assert_eq!(result.code, grpc_to_http_code(tonic::Code::Ok));
        assert_eq!(result.reason, Some("UNKNOWN".to_string()));
        let serialized = serde_json::to_string(&result).unwrap();
        assert_eq!(serialized, r#"{"status":"UNKNOWN","reason":"UNKNOWN"}"#);

        // UNKNOWN response from gRPC 0 OK from serving status 3 SERVICE_UNKNOWN
        let response = mock_grpc_response(Some(ServingStatus::ServiceUnknown as i32), None).await;
        let result = HealthCheckResult::from(response);
        assert_eq!(result.status, HealthStatus::Unknown);
        assert_eq!(result.code, grpc_to_http_code(tonic::Code::Ok));
        assert_eq!(result.reason, Some("SERVICE_UNKNOWN".to_string()));
        let serialized = serde_json::to_string(&result).unwrap();
        assert_eq!(
            serialized,
            r#"{"status":"UNKNOWN","reason":"SERVICE_UNKNOWN"}"#
        );

        // UNKNOWN response from other gRPC error codes (covering main ones)
        let codes = [
            tonic::Code::InvalidArgument,
            tonic::Code::Internal,
            tonic::Code::NotFound,
            tonic::Code::Unimplemented,
            tonic::Code::Unauthenticated,
            tonic::Code::PermissionDenied,
            tonic::Code::Unavailable,
        ];
        for code in codes.into_iter() {
            let status = tonic::Status::new(code, "some error message");
            let code = grpc_to_http_code(code);
            let response = mock_grpc_response(None, Some(status.clone())).await;
            let result = HealthCheckResult::from(response);
            assert_eq!(result.status, HealthStatus::Unknown);
            assert_eq!(result.code, code);
            assert_eq!(result.reason, Some("some error message".to_string()));
            let serialized = serde_json::to_string(&result).unwrap();
            assert_eq!(
                serialized,
                format!(
                    r#"{{"status":"UNKNOWN","code":{},"reason":"some error message"}}"#,
                    code.as_u16()
                ),
            );
        }
    }

    #[test]
    fn test_is_valid_hostname() {
        let valid_hostnames = ["localhost", "example.route.cloud.com", "127.0.0.1"];
        for hostname in valid_hostnames {
            assert!(is_valid_hostname(hostname));
        }
        let invalid_hostnames = [
            "-LoCaLhOST_",
            ".invalid",
            "invalid.ending-.char",
            "@asdf",
            "too-long-of-a-hostnameeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
        ];
        for hostname in invalid_hostnames {
            assert!(!is_valid_hostname(hostname));
        }
    }
}
