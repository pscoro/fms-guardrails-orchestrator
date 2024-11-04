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
use serde::{Deserialize, Serialize};
use serde::de::DeserializeOwned;
use url::Url;

use super::{Client, Error, HeaderMapExt, HttpClient};

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
            message: error.message,
        }
    }
}

pub trait DetectorClientExt: Client<ClientImpl = HttpClient> {
    /// All detector clients are HTTP (for now)
    async fn post<T: Debug + Serialize + Send, U: DeserializeOwned + Send>(
       &self,
       url: Url,
       request: T,
       headers: HeaderMap,
       model_id: &str,
    ) -> Result<U, Error> {
       let response = HttpClient::post(
           self.inner(),
           url,
           headers.with_header(DETECTOR_ID_HEADER_NAME, model_id),
           request
       ).await;
       response
   }
}

// Make a POST request for an HTTP detector client and return the response.
// Also injects the `traceparent` header from the current span and traces the response.
// pub async fn post_with_headers<T: Debug + Serialize + Send, U: DeserializeOwned + Send>(
//     client: HttpClient,
//     url: Url,
//     request: T,
//     headers: HeaderMap,
//     model_id: &str,
// ) -> Result<U, Error> {
//     info!(?url, ?headers, ?request, "sending client request");
//     let response = client.post(
//         url,
//         headers.with_header(DETECTOR_ID_HEADER_NAME, model_id),
//         request
//     ).await;
//     response
// }
