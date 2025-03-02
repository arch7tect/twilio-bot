use rocket::{get, http::Status, serde::json::Json, State};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

use crate::bot::backend::BackendClient;
use crate::config::Config;

/// Health status enum
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "UPPERCASE")]
pub enum HealthStatus {
    Up,
    Down,
    Unknown,
}

/// Health check for a specific component
#[derive(Debug, Serialize, Deserialize)]
pub struct HealthCheck {
    pub name: String,
    pub status: HealthStatus,
}

/// Overall health check response
#[derive(Debug, Serialize, Deserialize)]
pub struct HealthResponse {
    pub status: HealthStatus,
    pub checks: Vec<HealthCheck>,
}

/// Health check endpoint
#[get("/health")]
pub async fn health(config: &State<Config>) -> (Status, Json<HealthResponse>) {
    // Create a backend client
    let backend_client = match BackendClient::new(
        &config.backend_url,
        config.authorization_token.clone(),
    ) {
        Ok(client) => client,
        Err(_) => {
            return (
                Status::ServiceUnavailable,
                Json(HealthResponse {
                    status: HealthStatus::Down,
                    checks: vec![HealthCheck {
                        name: "BOT_BACK".to_string(),
                        status: HealthStatus::Down,
                    }],
                }),
            );
        }
    };

    // Check if the backend is healthy
    let backend_health = get_backend_health(&backend_client).await;
    let self_health = HealthCheck {
        name: "TWILIO_BOT".to_string(),
        status: HealthStatus::Up,
    };

    // Combine health checks
    let mut checks = vec![self_health, backend_health];
    
    // Determine overall status
    let overall_status = if checks.iter().any(|check| check.status == HealthStatus::Down) {
        HealthStatus::Down
    } else if checks.iter().any(|check| check.status == HealthStatus::Unknown) {
        HealthStatus::Unknown
    } else {
        HealthStatus::Up
    };

    // Create response
    let response = HealthResponse {
        status: overall_status,
        checks,
    };

    // Determine HTTP status code
    let status_code = if overall_status == HealthStatus::Up {
        Status::Ok
    } else {
        Status::ServiceUnavailable
    };

    (status_code, Json(response))
}

/// Check the health of the backend API
async fn get_backend_health(client: &BackendClient) -> HealthCheck {
    match client.run_command("HEALTH_CHECK", vec![]).await {
        Ok(_) => HealthCheck {
            name: "BOT_BACK".to_string(),
            status: HealthStatus::Up,
        },
        Err(_) => HealthCheck {
            name: "BOT_BACK".to_string(),
            status: HealthStatus::Down,
        },
    }
}
