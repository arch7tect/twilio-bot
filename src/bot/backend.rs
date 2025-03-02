use reqwest::{Client, ClientBuilder, StatusCode, Method};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, atomic::{AtomicUsize, AtomicU64, Ordering}};
use log::{debug, error, info};
use std::time::{SystemTime, UNIX_EPOCH};
use std::fmt;

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
    CircuitBreakerOpen,
    RetryExhausted(Box<BackendError>),
}

impl fmt::Display for BackendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BackendError::RequestError(err) => write!(f, "Request error: {}", err),
            BackendError::AuthError(msg) => write!(f, "Authentication error: {}", msg),
            BackendError::ApiError(msg) => write!(f, "API error: {}", msg),
            BackendError::JsonError(err) => write!(f, "JSON error: {}", err),
            BackendError::CircuitBreakerOpen => write!(f, "Circuit breaker is open"),
            BackendError::RetryExhausted(err) => write!(f, "Retry exhausted: {}", err),
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

/// Circuit breaker for preventing cascading failures
pub struct CircuitBreaker {
    failures: AtomicUsize,
    last_failure: AtomicU64,
    threshold: usize,
    reset_timeout_ms: u64,
}

impl CircuitBreaker {
    /// Create a new circuit breaker
    pub fn new(threshold: usize, reset_timeout_ms: u64) -> Self {
        CircuitBreaker {
            failures: AtomicUsize::new(0),
            last_failure: AtomicU64::new(0),
            threshold,
            reset_timeout_ms,
        }
    }
    
    /// Record a successful operation
    pub fn record_success(&self) {
        self.failures.store(0, Ordering::SeqCst);
    }
    
    /// Record a failed operation
    pub fn record_failure(&self) {
        self.failures.fetch_add(1, Ordering::SeqCst);
        self.last_failure.store(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64,
            Ordering::SeqCst
        );
    }
    
    /// Check if the circuit breaker is open (preventing requests)
    pub fn is_open(&self) -> bool {
        let failures = self.failures.load(Ordering::SeqCst);
        
        if failures >= self.threshold {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64;
            let last = self.last_failure.load(Ordering::SeqCst);
            
            // Circuit is open if we're within the reset timeout
            if now - last < self.reset_timeout_ms {
                return true;
            }
            
            // Otherwise, allow a test request
            self.failures.store(0, Ordering::SeqCst);
        }
        
        false
    }
}

/// Client for interacting with the backend API
pub struct BackendClient {
    client: Client,
    base_url: String,
    authorization_token: Option<String>,
    circuit_breaker: Option<Arc<CircuitBreaker>>,
}

impl BackendClient {
    /// Create a new backend client
    pub fn new(
        base_url: &str, 
        authorization_token: Option<String>,
        enable_circuit_breaker: bool,
    ) -> Result<Self, BackendError> {
        let client = ClientBuilder::new()
            .build()
            .map_err(BackendError::from)?;
        
        let circuit_breaker = if enable_circuit_breaker {
            Some(Arc::new(CircuitBreaker::new(5, 30000))) // 5 failures, 30s reset
        } else {
            None
        };
            
        Ok(BackendClient {
            client,
            base_url: base_url.to_string(),
            authorization_token,
            circuit_breaker,
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
    
    /// Generic API request method
    async fn make_api_request<T: serde::de::DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<T, BackendError> {
        // Check circuit breaker
        if let Some(cb) = &self.circuit_breaker {
            if cb.is_open() {
                return Err(BackendError::CircuitBreakerOpen);
            }
        }
        
        let url = format!("{}{}", self.base_url, path);
        
        let mut request = self.client.request(method, &url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json");
            
        request = self.add_auth_header(request);
        
        if let Some(body_data) = body {
            request = request.json(&body_data);
        }
        
        let response = match request.send().await {
            Ok(resp) => resp,
            Err(e) => {
                // Record failure
                if let Some(cb) = &self.circuit_breaker {
                    cb.record_failure();
                }
                return Err(BackendError::RequestError(e));
            }
        };
        
        let status = response.status();
        
        if status == StatusCode::FORBIDDEN {
            return Err(BackendError::AuthError("Permission denied".to_string()));
        } else if !status.is_success() {
            let error_text = response.text().await?;
            
            // Record failure
            if let Some(cb) = &self.circuit_breaker {
                cb.record_failure();
            }
            
            return Err(BackendError::ApiError(format!("API error: {} ({})", error_text, status)));
        }
        
        // Record success
        if let Some(cb) = &self.circuit_breaker {
            cb.record_success();
        }
        
        match response.json().await {
            Ok(result) => Ok(result),
            Err(e) => Err(BackendError::JsonError(e)),
        }
    }
    
    /// Run with retry capability
    pub async fn run_with_retry(
        &self,
        session_id: &str,
        message: &str,
        kwargs: HashMap<String, serde_json::Value>,
        max_retries: usize,
        base_delay_ms: u64,
    ) -> Result<serde_json::Value, BackendError> {
        let mut attempts = 0;
        let mut last_error = None;
        
        while attempts <= max_retries {
            match self.run(session_id, message, kwargs.clone()).await {
                Ok(result) => return Ok(result),
                Err(e) => {
                    // Don't retry certain errors
                    match &e {
                        BackendError::AuthError(_) => return Err(e),
                        BackendError::CircuitBreakerOpen => return Err(e),
                        _ => {
                            attempts += 1;
                            last_error = Some(e);
                            
                            if attempts <= max_retries {
                                let delay = base_delay_ms * 2u64.pow(attempts as u32 - 1);
                                debug!("Retrying backend call, attempt {}/{} after {}ms", 
                                       attempts, max_retries, delay);
                                tokio::time::sleep(tokio::time::Duration::from_millis(delay)).await;
                            }
                        }
                    }
                }
            }
        }
        
        Err(BackendError::RetryExhausted(Box::new(
            last_error.unwrap_or(BackendError::ApiError("Maximum retries exceeded".to_string()))
        )))
    }
    
    /// Run a command on an existing session
    pub async fn run_command(
        &self,
        session_id: &str,
        command: &str,
        args: Vec<String>,
    ) -> Result<serde_json::Value, BackendError> {
        let path = format!("/session/{}/command", session_id);
        
        let body = serde_json::json!({
            "command": command,
            "args": args
        });
        
        self.make_api_request(Method::POST, &path, Some(body)).await
    }
    
    /// Run a message on an existing session
    pub async fn run(
        &self,
        session_id: &str,
        message: &str,
        kwargs: HashMap<String, serde_json::Value>,
    ) -> Result<serde_json::Value, BackendError> {
        let path = format!("/session/{}/run", session_id);
        
        let body = serde_json::json!({
            "message": message,
            "kwargs": kwargs
        });
        
        self.make_api_request(Method::POST, &path, Some(body)).await
    }
    
    /// Start a message processing on an existing session
    pub async fn start(
        &self,
        session_id: &str,
        message: &str,
    ) -> Result<serde_json::Value, BackendError> {
        let path = format!("/session/{}/start", session_id);
        
        let body = serde_json::json!({
            "message": message,
            "kwargs": {}
        });
        
        self.make_api_request(Method::POST, &path, Some(body)).await
    }
    
    /// Commit a message processing on an existing session
    pub async fn commit(
        &self,
        session_id: &str,
    ) -> Result<serde_json::Value, BackendError> {
        let path = format!("/session/{}/commit", session_id);
        let body = serde_json::json!({});
        
        self.make_api_request(Method::POST, &path, Some(body)).await
    }
    
    /// Rollback a message processing on an existing session
    pub async fn rollback(
        &self,
        session_id: &str,
    ) -> Result<serde_json::Value, BackendError> {
        let path = format!("/session/{}/rollback", session_id);
        let body = serde_json::json!({});
        
        self.make_api_request(Method::POST, &path, Some(body)).await
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
    ) -> Result<SessionResponse, BackendError> {
        let path = "/session";
        
        let body = serde_json::json!({
            "user_id": user_id,
            "name": name,
            "type": bot_type,
            "conversation_id": conversation_id,
            "args": args,
            "kwargs": kwargs
        });
        
        let session_response: SessionResponse = self.make_api_request(
            Method::POST, 
            path, 
            Some(body)
        ).await?;
        
        info!("Opened session with ID: {}", session_response.session.session_id);
        
        Ok(session_response)
    }
    
    /// Update an existing session
    pub async fn update_session(
        &self,
        session_id: &str,
        conversation_id: Option<&str>,
    ) -> Result<serde_json::Value, BackendError> {
        let path = format!("/session/{}", session_id);
        
        let mut body = serde_json::json!({});
        
        if let Some(cid) = conversation_id {
            body = serde_json::json!({
                "conversation_id": cid
            });
        }
        
        self.make_api_request(Method::PUT, &path, Some(body)).await
    }
    
    /// Close an existing session
    pub async fn close_session(
        &self,
        session_id: &str,
        status: Option<&str>,
    ) -> Result<(), BackendError> {
        let mut path = format!("/session/{}", session_id);
        
        if let Some(status_str) = status {
            path = format!("{}?status={}", path, status_str);
        }
        
        debug!("Closing session {} with status {:?}", session_id, status);
        
        let _: serde_json::Value = self.make_api_request(Method::DELETE, &path, None).await?;
        
        info!("Successfully closed session {}", session_id);
        Ok(())
    }
}