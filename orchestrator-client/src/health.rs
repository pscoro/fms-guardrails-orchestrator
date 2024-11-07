use std::{
    collections::HashMap, fmt::{Debug, Display}
};

use hyper::http::StatusCode;
use serde::{Deserialize, Serialize};

use crate::{
    pb::grpc::health::v1::{health_check_response, HealthCheckResponse},
    utils::grpc_to_http_code,
};
use crate::pb::grpc::health::v1::health_check_response::ServingStatus;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(rename_all = "UPPERCASE")]
    pub enum HealthStatus {
        /// The service status is healthy.
        Healthy,
        /// The service status is unhealthy.
        Unhealthy,
        /// The service status is unknown.
        Unknown,
    }

// mod r#impl {
//     use serde::{Deserialize, Serialize};
//
//     use super::*;
//
//     /// Health status determined for or returned by a client service.
//     #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
//     #[serde(rename_all = "UPPERCASE")]
//     pub enum DefaultHealthStatus {
//         /// The service status is healthy.
//         Healthy,
//         /// The service status is unhealthy.
//         Unhealthy,
//         /// The service status is unknown.
//         Unknown,
//     }
//
//     impl HealthStatus for DefaultHealthStatus {}
//
//
// }
// pub use r#impl::*;
//
// mod ext {
//     use super::*;
//
//     pub trait HealthStatusDisplayExt {
//         fn display(&self) -> HealthStatusDisplay<Self>;
//     }
//
//     impl<T: HealthStatus> HealthStatusDisplayExt for T {
//         fn display(&self) -> HealthStatusDisplay<Self> {
//             HealthStatusDisplay(self)
//         }
//     }
//
//     pub struct HealthStatusDisplay<'a, T: HealthStatus>(&'a T);
//
//     impl<T: HealthStatus> Display for HealthStatusDisplay<'_, T> {
//         fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
//             write!(f, "{}", serde_json::to_string(self.0)?)
//         }
//     }
// }
// pub use ext::*;
//
// pub trait HealthStatus: Debug + Clone + PartialEq + Serialize + DeserializeOwned + Send + Sync + Sized {}
// impl<H: HealthStatus, U> FromGroupExt for H {
//     type From<T> = U where U: FromGroup<T>;
// }
//
// pub trait FromGroup<T> {}
//
// pub trait FromGroupExt {
//     type From<T>: FromGroup<T>;
// }
// impl<T, U: Grpc> FromGroup<T> for U {}
//
// pub enum Protocol {
//     Http(dyn Http<Health=()>),
//     Grpc(dyn Grpc<Health=()>),
// }
//
// pub trait ProtocolLike {
//     type Health;
// }
// pub trait HttpHealth {
//     // type ServingStatus;
// }
// // impl dyn HttpHealth<ServingStatus = ServingStatus> {}
// pub trait ServingStatus<T> {
//     fn serving() -> T;
//     fn not_serving() -> T;
//     fn unknown() -> T;
//     fn service_unknown() -> T;
// }
//
// pub trait GrpcHealth {
//     type ServingStatus<T>: ServingStatus<T>;
// }
// impl dyn GrpcHealth<ServingStatus = health_check_response::ServingStatus> {}
//
// pub trait Http {
//     type Health: HttpHealth;
// }
//
// impl<H> ProtocolLike for dyn Http<Health = H> { type Health = H; }
//
// pub trait Grpc {
//     type Health: GrpcHealth;
// }
//
// impl<H> ProtocolLike for dyn Grpc<Health = H> { type Health = H; }
//
// #[cfg(feature = "grpc")]
// impl<Status, Health> From<HealthCheckResponse> for Status
// where
//     Status: HealthStatus + FromGroupExt<From = Health>,
//     Health: GrpcHealth<ServingStatus: ServingStatus<Status>>,
// {
//     fn from(value: HealthCheckResponse) -> Box<Self> {
//         match value.status() {
//             health_check_response::ServingStatus::Serving => Health::ServingStatus::serving(),
//             health_check_response::ServingStatus::NotServing => Health::ServingStatus::not_serving(), // UNHEALTHY
//             health_check_response::ServingStatus::Unknown => Health::ServingStatus::unknown(), // UNKNOWN
//             health_check_response::ServingStatus::ServiceUnknown => Health::ServingStatus::service_unknown(), // UNKNOWN
//         }
//     }
// }

impl From<HealthCheckResponse> for HealthStatus
{
    fn from(value: HealthCheckResponse) -> Self {
        match value.status() {
            health_check_response::ServingStatus::Serving => HealthStatus::Healthy,
            health_check_response::ServingStatus::NotServing => HealthStatus::Unhealthy,
            health_check_response::ServingStatus::Unknown => HealthStatus::Unknown,
            health_check_response::ServingStatus::ServiceUnknown => HealthStatus::Unknown,
        }
    }
}

impl From<StatusCode> for HealthStatus {
    fn from(code: StatusCode) -> Self {
        match code.as_u16() {
            200..=299 => Self::Healthy,
            500..=599 => Self::Unhealthy,
            _ => Self::Unknown,
        }
    }
}

/// A cache to hold the latest health check results for each client service.
/// Orchestrator has a reference-counted mutex-protected instance of this cache.
#[derive(Debug, Clone, Default, Serialize)]
pub struct HealthCheckCache(HashMap<String, HealthCheckResult>);

impl HealthCheckCache {
    pub fn new() -> Self {
        Self(HashMap::new())
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self(HashMap::with_capacity(capacity))
    }

    /// Returns `true` if all services are healthy or unknown.
    pub fn healthy(&self) -> bool {
        !self
            .0
            .iter()
            .any(|(_, value)| matches!(value.status, HealthStatus::Unhealthy))
    }
}

impl std::ops::Deref for HealthCheckCache {
    type Target = HashMap<String, HealthCheckResult>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for HealthCheckCache {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Display for HealthCheckCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", serde_json::to_string_pretty(self).unwrap())
    }
}

impl HealthCheckResponse {
    pub fn reason(&self) -> Option<String> {
        let status = self.status();
        match status {
            ServingStatus::Serving => None,
            _ => Some(status.as_str_name().to_string()),
        }
    }
}

/// Result of a health check request.
#[derive(Debug, Clone, Serialize)]
pub struct HealthCheckResult {
    /// Overall health status of client service.
    pub status: HealthStatus,
    /// Response code of the latest health check request.
    #[serde(
        with = "http_serde::status_code",
        skip_serializing_if = "StatusCode::is_success"
    )]
    pub code: StatusCode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl Display for HealthCheckResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.reason {
            Some(reason) => write!(f, "{} ({})\n\t\t\t{}", self.status, self.code, reason),
            None => write!(f, "{} ({})", self.status, self.code),
        }
    }
}

impl From<Result<tonic::Response<HealthCheckResponse>, tonic::Status>> for HealthCheckResult {
    fn from(result: Result<tonic::Response<HealthCheckResponse>, tonic::Status>) -> Self {
        match result {
            Ok(response) => {
                let response = response.into_inner();
                Self {
                    status: response.into(),
                    code: StatusCode::OK,
                    reason: response.reason(),
                }
            }
            Err(status) => Self {
                status: HealthStatus::on_grpc_error(), // UNKNOWN
                code: grpc_to_http_code(status.code()),
                reason: Some(status.message().to_string()),
            },
        }
    }
}

/// An optional response body that can be interpreted from an HTTP health check response.
/// This is a minimal contract that allows HTTP health requests to opt in to more detailed health check responses than just the status code.
/// If the body omitted, the health check response is considered successful if the status code is `HTTP 200 OK`.
#[derive(Deserialize)]
pub struct OptionalHealthCheckResponseBody {
    /// `HEALTHY`, `UNHEALTHY`, or `UNKNOWN`. Although `HEALTHY` is already implied without a body.
    pub status: HealthStatus,
    /// Optional reason for the health check result status being `UNHEALTHY` or `UNKNOWN`.
    /// May be omitted overall if the health check was successful.
    #[serde(default)]
    pub reason: Option<String>,
}
