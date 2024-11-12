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

use std::fmt::Debug;

use axum::http::HeaderMap;
use hyper::StatusCode;
use serde::Deserialize;
use url::Url;

use super::{
    http::{HttpClientExt, RequestLike, ResponseLike},
    Error,
};

pub mod text_contents;
pub use text_contents::*;
pub mod text_chat;
pub use text_chat::*;
pub mod text_context_doc;
pub use text_context_doc::*;
pub mod text_generation;
pub use text_generation::*;

const DEFAULT_PORT: u16 = 8080;
const DETECTOR_ID_HEADER_NAME: &str = "detector-id";

#[derive(Debug, Clone, Deserialize)]
pub struct DetectorError {
    pub code: u16,
    pub message: String,
}

impl From<DetectorError> for Error {
    fn from(error: DetectorError) -> Self {
        Error::Http {
            code: StatusCode::from_u16(error.code).unwrap(),
            message: format!("error response from detector: {}", error.message),
        }
    }
}

pub trait DetectorClient {}

pub trait DetectorClientExt: HttpClientExt {
    async fn post_to_detector<U: ResponseLike>(
        &self,
        model_id: &str,
        url: Url,
        headers: HeaderMap,
        request: impl RequestLike,
    ) -> Result<U, Error>;

    fn endpoint(&self, path: impl Into<&'static str>) -> Url;
}

impl<C: DetectorClient + HttpClientExt> DetectorClientExt for C {
    /// Make a POST request for an HTTP detector client and return the response.
    /// Also injects the `traceparent` header from the current span and traces the response.
    async fn post_to_detector<U: ResponseLike>(
        &self,
        model_id: &str,
        url: Url,
        headers: HeaderMap,
        request: impl RequestLike,
    ) -> Result<U, Error> {
        let mut headers = headers;
        headers.append(DETECTOR_ID_HEADER_NAME, model_id.parse().unwrap());
        let response = self.inner().clone().post(url, headers, request).await?;

        let status = response.status();
        match status {
            StatusCode::OK => Ok(response.json().await?),
            _ => Err(response
                .json::<DetectorError>()
                .await
                .unwrap_or(DetectorError {
                    code: status.as_u16(),
                    message: "".into(),
                })
                .into()),
        }
    }

    fn endpoint(&self, path: impl Into<&'static str>) -> Url {
        self.inner().endpoint(path)
    }
}
