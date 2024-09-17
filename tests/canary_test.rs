use std::sync::Arc;

use axum_test::TestServer;
use fms_guardrails_orchestr8::{
    config::OrchestratorConfig,
    orchestrator::Orchestrator,
    server::{get_health_app, ServerState},
};
use hyper::StatusCode;
use serde_json::Value;

/// Checks if the health endpoint is working
#[tokio::test]
async fn test_health() {
    let config = OrchestratorConfig::load("config/test.config.yaml")
        .await
        .unwrap();
    let orchestrator = Orchestrator::new(config, false).await.unwrap();
    let shared_state = Arc::new(ServerState::new(orchestrator));
    let server = TestServer::new(get_health_app(shared_state)).unwrap();
    let response = server.get("/health").await;
    println!("{:#?}", response);
    let body: Value = serde_json::from_str(response.text().as_str()).unwrap();
    println!("{}", serde_json::to_string_pretty(&body).unwrap());
    response.assert_status(StatusCode::OK);
    let response = server.get("/info").await;
    println!("{:#?}", response);
    let body: Value = serde_json::from_str(response.text().as_str()).unwrap();
    println!("{}", serde_json::to_string_pretty(&body).unwrap());
    response.assert_status(StatusCode::OK);
}
