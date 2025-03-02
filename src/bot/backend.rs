use reqwest::{Client, ClientBuilder, StatusCode};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use log::{debug, error, info};

use crate::bot::session::SessionStore;
use crate::config::Config;
use crate::bot::ws_client::WebSocketManager;

/// Response from the backend when opening a session
#[derive(Debug, Deserialize)]
pub struct SessionResponse {
    pub session: SessionInfo,
    #[serde(default)]
    pub metadata: serde_json::Value,
}

/// Session information returned by the backend
#[derive(Debug, Deserialize)]
pub struct SessionInfo {
    pub session_id: String,
}

/// Error type for backend client operations
#[derive(Debug)]
pub enum BackendError {
    RequestError(reqwest::Error),
    AuthError(String),
    ApiError(String),
    JsonError(serde_json::Error),
}

impl std::fmt::Display for BackendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BackendError::RequestError(err) => write!(f, "Request error: {}", err),
            BackendError::AuthError(msg) => write!(f, "Authentication error: {}", msg),
            BackendError::ApiError(msg) => write!(f, "API error: {}", msg),
            BackendError::JsonError(err) => write!(f, "JSON error: {}", err),
        }
    }
}

impl std::error::Error for BackendError {}

impl From<reqwest::Error> for BackendError {
    fn from(err: reqwest::Error) -> Self {
        BackendError::RequestError(err)
    }
}

impl From<serde_json::Error> for BackendError {
    fn from(err: serde_json::Error) -> Self {
        BackendError::JsonError(err)
    }
}

/// Client for interacting with the backend API
pub struct BackendClient {
    client: Client,
    base_url: String,
    authorization_token: Option<String>,
}

impl BackendClient {
    /// Create a new backend client
    pub fn new(base_url: &str, authorization_token: Option<String>) -> Result<Self, BackendError> {
        let client = ClientBuilder::new()
            .build()
            .map_err(BackendError::from)?;
            
        Ok(BackendClient {
            client,
            base_url: base_url.to_string(),
            authorization_token,
        })
    }
    
    /// Add authorization header to a request builder if a token is available
    fn add_auth_header(&self, builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        if let Some(token) = &self.authorization_token {
            builder.header("Authorization", format!("Bearer {}", token))
        } else {
            builder
        }
    }
    
    /// Open a new session with the backend
    pub async fn open_session(
        &self,
        user_id: &str,
        name: &str,
        bot_type: &str,
        conversation_id: Option<&str>,
        args: Vec<String>,
        kwargs: HashMap<String, serde_json::Value>,
        config: &Config,
        sessions: Arc<Mutex<SessionStore>>,
        ws_manager: Arc<WebSocketManager>,
    ) -> Result<SessionResponse, BackendError> {
        let url = format!("{}/session", self.base_url);
        debug!("Opening session for user {} of type {}", user_id, bot_type);
        
        let body = serde_json::json!({
            "user_id": user_id,
            "name": name,
            "type": bot_type,
            "conversation_id": conversation_id,
            "args": args,
            "kwargs": kwargs
        });
        
        let request = self.client.post(&url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json");
            
        let request = self.add_auth_header(request);
        
        let response = request
            .json(&body)
            .send()
            .await?;
            
        let status = response.status();
        
        if status == StatusCode::FORBIDDEN {
            return Err(BackendError::AuthError("Permission denied".to_string()));
        } else if !status.is_success() {
            let error_text = response.text().await?;
            return Err(BackendError::ApiError(format!("Failed to open session: {} ({})", error_text, status)));
        }
        
        let session_response: SessionResponse = response.json().await?;
        info!("Opened session with ID: {}", session_response.session.session_id);
        
        // Create a WebSocket client for this session
        if !config.backend_ws_url.is_empty() {
            ws_manager.get_or_create_client(
                &session_response.session.session_id,
                &config.backend_ws_url,
                sessions.clone(),
            ).await;
        }
        
        Ok(session_response)
    }
    
    /// Run a command on an existing session
    pub async fn run_command(
        &self,
        session_id: &str,
        command: &str,
        args: Vec<String>,
    ) -> Result<serde_json::Value, BackendError> {
        let url = format!("{}/session/{}/command", self.base_url, session_id);
        debug!("Running command '{}' on session {}", command, session_id);
        
        let body = serde_json::json!({
            "command": command,
            "args": args
        });
        
        let request = self.client.post(&url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json");
            
        let request = self.add_auth_header(request);
        
        let response = request
            .json(&body)
            .send()
            .await?;
            
        let status = response.status();
        
        if !status.is_success() {
            let error_text = response.text().await?;
            return Err(BackendError::ApiError(format!("Failed to run command: {} ({})", error_text, status)));
        }
        
        let result: serde_json::Value = response.json().await?;
        Ok(result)
    }
    
    /// Run a message on an existing session
    pub async fn run(
        &self,
        session_id: &str,
        message: &str,
        kwargs: HashMap<String, serde_json::Value>,
    ) -> Result<serde_json::Value, BackendError> {
        let url = format!("{}/session/{}/run", self.base_url, session_id);
        debug!("Running message on session {}: {}", session_id, message);
        
        let body = serde_json::json!({
            "message": message,
            "kwargs": kwargs
        });
        
        let request = self.client.post(&url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json");
            
        let request = self.add_auth_header(request);
        
        let response = request
            .json(&body)
            .send()
            .await?;
            
        let status = response.status();
        
        if !status.is_success() {
            let error_text = response.text().await?;
            return Err(BackendError::ApiError(format!("Failed to run message: {} ({})", error_text, status)));
        }
        
        let result: serde_json::Value = response.json().await?;
        Ok(result)
    }
    
    /// Start a message processing on an existing session
    pub async fn start(
        &self,
        session_id: &str,
        message: &str,
    ) -> Result<serde_json::Value, BackendError> {
        let url = format!("{}/session/{}/start", self.base_url, session_id);
        debug!("Starting message on session {}: {}", session_id, message);
        
        let body = serde_json::json!({
            "message": message,
            "kwargs": {}
        });
        
        let request = self.client.post(&url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json");
            
        let request = self.add_auth_header(request);
        
        let response = request
            .json(&body)
            .send()
            .await?;
            
        let status = response.status();
        
        if !status.is_success() {
            let error_text = response.text().await?;
            return Err(BackendError::ApiError(format!("Failed to start message: {} ({})", error_text, status)));
        }
        
        let result: serde_json::Value = response.json().await?;
        Ok(result)
    }
    
    /// Commit a message processing on an existing session
    pub async fn commit(
        &self,
        session_id: &str,
    ) -> Result<serde_json::Value, BackendError> {
        let url = format!("{}/session/{}/commit", self.base_url, session_id);
        debug!("Committing session {}", session_id);
        
        let body = serde_json::json!({});
        
        let request = self.client.post(&url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json");
            
        let request = self.add_auth_header(request);
        
        let response = request
            .json(&body)
            .send()
            .await?;
            
        let status = response.status();
        
        if !status.is_success() {
            let error_text = response.text().await?;
            return Err(BackendError::ApiError(format!("Failed to commit: {} ({})", error_text, status)));
        }
        
        let result: serde_json::Value = response.json().await?;
        Ok(result)
    }
    
    /// Rollback a message processing on an existing session
    pub async fn rollback(
        &self,
        session_id: &str,
    ) -> Result<serde_json::Value, BackendError> {
        let url = format!("{}/session/{}/rollback", self.base_url, session_id);
        debug!("Rolling back session {}", session_id);
        
        let body = serde_json::json!({});
        
        let request = self.client.post(&url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json");
            
        let request = self.add_auth_header(request);
        
        let response = request
            .json(&body)
            .send()
            .await?;
            
        let status = response.status();
        
        if !status.is_success() {
            let error_text = response.text().await?;
            return Err(BackendError::ApiError(format!("Failed to rollback: {} ({})", error_text, status)));
        }
        
        let result: serde_json::Value = response.json().await?;
        Ok(result)
    }
    
    /// Update an existing session
    pub async fn update_session(
        &self,
        session_id: &str,
        conversation_id: Option<&str>,
    ) -> Result<serde_json::Value, BackendError> {
        let url = format!("{}/session/{}", self.base_url, session_id);
        debug!("Updating session {} with conversation ID {:?}", session_id, conversation_id);
        
        let mut body = serde_json::json!({});
        
        if let Some(cid) = conversation_id {
            body = serde_json::json!({
                "conversation_id": cid
            });
        }
        
        let request = self.client.put(&url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json");
            
        let request = self.add_auth_header(request);
        
        let response = request
            .json(&body)
            .send()
            .await?;
            
        let status = response.status();
        
        if !status.is_success() {
            let error_text = response.text().await?;
            return Err(BackendError::ApiError(format!("Failed to update session: {} ({})", error_text, status)));
        }
        
        let result: serde_json::Value = response.json().await?;
        Ok(result)
    }
    
    /// Close an existing session
    pub async fn close_session(
        &self,
        session_id: &str,
        status: Option<&str>,
    ) -> Result<(), BackendError> {
        let mut url = format!("{}/session/{}", self.base_url, session_id);
        
        if let Some(status_str) = status {
            url = format!("{}?status={}", url, status_str);
        }
        
        debug!("Closing session {} with status {:?}", session_id, status);
        
        let request = self.client.delete(&url)
            .header("Accept", "application/json");
            
        let request = self.add_auth_header(request);
        
        let response = request
            .send()
            .await?;
            
        let status_code = response.status();
        
        if !status_code.is_success() {
            let error_text = response.text().await?;
            return Err(BackendError::ApiError(format!("Failed to close session: {} ({})", error_text, status_code)));
        }
        
        info!("Successfully closed session {}", session_id);
        Ok(())
    }
}
