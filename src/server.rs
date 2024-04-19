use crate::{config, models, utils, ErrorResponse, GuardrailsResponse};
use std::{net::SocketAddr};
use axum::{
    extract::Extension,
    http::{HeaderMap, Method, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
// sse -> server side events
use axum::response::sse::{Event, KeepAlive, Sse};
use futures::stream::Stream;
use serde::{Serialize};
use serde_json::{json, Value};
use tokio::{signal};
use tracing::info;
use std::convert::Infallible;

// ========================================== Constants and Dummy Variables ==========================================
const API_PREFIX: &'static str = r#"/api/v1/task"#;
const TGIS_PORT: u16 = 8033;

// TODO: Change with real object
struct InnerResponse {
    sample: bool
}
struct SampleResponse {
    response: InnerResponse
}

// TODO: Dummy streaming response object
#[derive(Serialize)]
pub(crate) struct StreamResponse {
    pub generated_text: String,
    pub processed_index: i32,
}

const DUMMY_RESPONSE: [&'static str; 9] = ["This", "is", "very", "good", "news,", "streaming", "is", "working", "!"];

// ========================================== Handler functions ==========================================


// Server shared state
#[derive(Clone)]
pub(crate) struct ServerState {
}

#[allow(clippy::too_many_arguments)]
pub async fn run(
    rest_addr: SocketAddr,
    // tls_key_pair: Option<(String, String)>,
    orchestrator_config: config::OrchestratorConfig,
) {

    // TODO: Configure TLS if requested

    // Configure TGIS
    let tgis_servicer = utils::configure_tgis(
        orchestrator_config.tgis_config,
        TGIS_PORT
    );

    // Build and await on the HTTP server
    let app = Router::new()
        .route("/health", get(health))
        .route(&format!("{}/classification-with-text-generation", API_PREFIX), post(classification_with_generation))
        .route(&format!("{}/server-streaming-classification-with-text-generation", API_PREFIX), post(stream_classification_with_gen));

    let server = axum::Server::bind(&rest_addr)
        .serve(app.into_make_service())
        .with_graceful_shutdown(shutdown_signal());

    info!("HTTP server started on port {}", rest_addr.port());
    server.await.unwrap();
    info!("HTTP server shutdown complete");

}

async fn health() -> Result<(), ()> {
    // TODO: determine how to detect if orchestrator is healthy or not
    Ok(())
}

// #[debug_handler]
// TODO: Improve Bad Request error handling by implementing Validate middleware
async fn classification_with_generation(
    Json(payload): Json<models::GuardrailsHttpRequest>) -> Json<GuardrailsResponse> {

    // TODO: note this function currently is not doing .await and hence is blocking
    let token_class_result = models::TextGenTokenClassificationResults::new();
    let input_token_count = 2;
    let response = models::ClassifiedGeneratedTextResult::new(token_class_result, input_token_count);
    Json(GuardrailsResponse::SuccessfulResponse(response))

}


async fn stream_classification_with_gen(
    // state: Extension<ServerState>,
    Json(payload): Json<models::GuardrailsHttpRequest>) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {

    let on_message_callback = |stream_token: StreamResponse| {
        let event = Event::default();
        event.json_data(stream_token).unwrap()
    };

    let response_stream =
        generate_stream_response(Json(payload), on_message_callback).await;
    let sse = Sse::new(response_stream).keep_alive(KeepAlive::default());
    sse
}

async fn generate_stream_response(
    Json(payload): Json<models::GuardrailsHttpRequest>,
    on_message_callback: impl Fn(StreamResponse) -> Event,
) -> impl Stream<Item = Result<Event, Infallible>> {


    let mut dummy_response_iterator = DUMMY_RESPONSE.iter();

    let mut index: i32 = 0;
    let stream = async_stream::stream! {
        // Server sending event stream
        while let Some(&token) = dummy_response_iterator.next() {
            let stream_token = StreamResponse {
                generated_text: token.to_string(),
                processed_index: index
            };
            index += 1;
            let event = on_message_callback(stream_token);
            yield Ok(event);
        }
    };
    stream
}


/// Shutdown signal handler
async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    info!("signal received, starting graceful shutdown");
}
