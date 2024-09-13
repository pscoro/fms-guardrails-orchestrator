use axum_test::TestServer;
use fms_guardrails_orchestr8::orchestrator::Orchestrator;
use fms_guardrails_orchestr8::server::{get_health_app, ServerState};
use hyper::StatusCode;
use std::sync::Arc;

/// Checks if the health endpoint is working
#[tokio::test]
async fn test_health() {
    let orchestrator = Orchestrator::default();
    let shared_state = Arc::new(ServerState::new(orchestrator));
    let server = TestServer::new(get_health_app(shared_state)).unwrap();
    let response = server.get("/health").await;
    response.assert_status(StatusCode::OK);
    let response = server.get("/info").await;
    response.assert_status(StatusCode::OK);
}
