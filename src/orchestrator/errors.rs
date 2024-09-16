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
use crate::clients;
use uuid::Uuid;

/// Orchestrator errors.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Client(#[from] clients::Error),
    #[error("detector `{0}` not found")]
    DetectorNotFound(String),
    #[error("detector request {request_id} failed for detector `{detector_id}`: {error}")]
    DetectorRequestFailed {
        request_id: Uuid,
        detector_id: String,
        error: clients::Error,
    },
    #[error("chunker request {request_id} failed for chunker `{chunker_id}`: {error}")]
    ChunkerRequestFailed {
        request_id: Uuid,
        chunker_id: String,
        error: clients::Error,
    },
    #[error("generate request {request_id} failed for generation model `{generation_model_id}`: {error}")]
    GenerateRequestFailed {
        request_id: Uuid,
        generation_model_id: String,
        error: clients::Error,
    },
    #[error("tokenize request {request_id} failed for generation model `{generation_model_id}`: {error}")]
    TokenizeRequestFailed {
        request_id: Uuid,
        generation_model_id: String,
        error: clients::Error,
    },
    #[error("orchestrator or configured clients are unhealthy: {message}")]
    Unhealthy { message: String },
    #[error("{0}")]
    Other(String),
    #[error("cancelled")]
    Cancelled,
}

impl From<tokio::task::JoinError> for Error {
    fn from(error: tokio::task::JoinError) -> Self {
        if error.is_cancelled() {
            Self::Cancelled
        } else {
            Self::Other(format!("task panicked: {error}"))
        }
    }
}
