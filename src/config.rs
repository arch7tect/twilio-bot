use std::env;
use serde::{Deserialize, Serialize};

/// Twilio-specific configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TwilioConfig {
    pub account_sid: String,
    pub auth_token: String,
    pub from_number: String,
    pub webhook_url: String,
    pub webhook_port: u16,
    pub voice: String,
    pub speech_model: String,
    pub default_timeout: u32,
    pub partial_processing: bool,
    pub language: Option<String>,
    pub region: Option<String>,
    pub edge: Option<String>,
}

impl TwilioConfig {
    /// Validate Twilio configuration
    pub fn validate(&self) -> Result<(), String> {
        if self.account_sid.is_empty() {
            return Err("Twilio account SID cannot be empty".to_string());
        }
        if self.auth_token.is_empty() {
            return Err("Twilio auth token cannot be empty".to_string());
        }
        if self.from_number.is_empty() {
            return Err("From number cannot be empty".to_string());
        }
        if self.webhook_url.is_empty() {
            return Err("Webhook URL cannot be empty".to_string());
        }
        
        if self.webhook_port == 0 || self.webhook_port > 65535 {
            return Err("Webhook port must be a valid port number".to_string());
        }
        
        if self.default_timeout == 0 {
            return Err("Default timeout must be greater than 0".to_string());
        }
        
        Ok(())
    }
    
    /// Load Twilio configuration from environment variables
    pub fn from_env() -> Result<Self, String> {
        let config = TwilioConfig {
            account_sid: env::var("TWILIO_ACCOUNT_SID")
                .map_err(|_| "TWILIO_ACCOUNT_SID must be set".to_string())?,
            auth_token: env::var("TWILIO_AUTH_TOKEN")
                .map_err(|_| "TWILIO_AUTH_TOKEN must be set".to_string())?,
            from_number: env::var("FROM_NUMBER")
                .map_err(|_| "FROM_NUMBER must be set".to_string())?,
            webhook_url: env::var("TWILIO_WEBHOOK_URL")
                .map_err(|_| "TWILIO_WEBHOOK_URL must be set".to_string())?,
            webhook_port: env::var("FLAMETREE_CALLBACK_PORT")
                .unwrap_or_else(|_| "8000".to_string())
                .parse()
                .map_err(|_| "FLAMETREE_CALLBACK_PORT must be a valid port number".to_string())?,
            voice: env::var("TWILIO_VOICE")
                .unwrap_or_else(|_| "Polly.Salli".to_string()),
            speech_model: env::var("SPEECH_MODEL")
                .unwrap_or_else(|_| "googlev2_telephony".to_string()),
            default_timeout: env::var("DEFAULT_TIMEOUT")
                .unwrap_or_else(|_| "10".to_string())
                .parse()
                .map_err(|_| "DEFAULT_TIMEOUT must be a valid number".to_string())?,
            partial_processing: env::var("PARTIAL_PROCESSING")
                .unwrap_or_else(|_| "true".to_string())
                .to_lowercase() == "true",
            language: env::var("TWILIO_LANGUAGE").ok(),
            region: env::var("TWILIO_REGION")
                .ok()
                .filter(|s| !s.is_empty()),
            edge: env::var("TWILIO_EDGE")
                .ok()
                .filter(|s| !s.is_empty()),
        };
        
        config.validate()?;
        Ok(config)
    }
}

/// Backend-specific configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendConfig {
    pub url: String,
    pub authorization_token: Option<String>,
    pub ws_url: String,
    pub enable_circuit_breaker: bool,
    pub retry_attempts: usize,
    pub retry_base_delay_ms: u64,
}

impl BackendConfig {
    /// Validate backend configuration
    pub fn validate(&self) -> Result<(), String> {
        if self.url.is_empty() {
            return Err("Backend URL cannot be empty".to_string());
        }
        if self.ws_url.is_empty() {
            return Err("Backend WebSocket URL cannot be empty".to_string());
        }
        
        Ok(())
    }
    
    /// Load backend configuration from environment variables
    pub fn from_env() -> Result<Self, String> {
        let config = BackendConfig {
            url: env::var("BACKEND_URL")
                .map_err(|_| "BACKEND_URL must be set".to_string())?,
            authorization_token: env::var("AUTHORIZATION_TOKEN").ok(),
            ws_url: env::var("BACKEND_WS_URL")
                .map_err(|_| "BACKEND_WS_URL must be set".to_string())?,
            enable_circuit_breaker: env::var("ENABLE_CIRCUIT_BREAKER")
                .unwrap_or_else(|_| "true".to_string())
                .to_lowercase() == "true",
            retry_attempts: env::var("RETRY_ATTEMPTS")
                .unwrap_or_else(|_| "3".to_string())
                .parse()
                .unwrap_or(3),
            retry_base_delay_ms: env::var("RETRY_BASE_DELAY_MS")
                .unwrap_or_else(|_| "500".to_string())
                .parse()
                .unwrap_or(500),
        };
        
        config.validate()?;
        Ok(config)
    }
}

/// Session management configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionConfig {
    pub cleanup_interval_minutes: u64,
    pub max_age_minutes: i64,
}

impl SessionConfig {
    /// Load session configuration from environment variables
    pub fn from_env() -> Self {
        SessionConfig {
            cleanup_interval_minutes: env::var("SESSION_CLEANUP_INTERVAL_MINUTES")
                .unwrap_or_else(|_| "5".to_string())
                .parse()
                .unwrap_or(5),
            max_age_minutes: env::var("SESSION_MAX_AGE_MINUTES")
                .unwrap_or_else(|_| "30".to_string())
                .parse()
                .unwrap_or(30),
        }
    }
}

/// Combined application configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub twilio: TwilioConfig,
    pub backend: BackendConfig,
    pub session: SessionConfig,
}

impl Config {
    /// Validate the complete configuration
    pub fn validate(&self) -> Result<(), String> {
        self.twilio.validate()?;
        self.backend.validate()?;
        
        Ok(())
    }
    
    /// Create configuration from environment variables
    pub fn from_env() -> Result<Self, String> {
        let twilio = TwilioConfig::from_env()?;
        let backend = BackendConfig::from_env()?;
        let session = SessionConfig::from_env();
        
        let config = Config {
            twilio,
            backend,
            session,
        };
        
        config.validate()?;
        
        Ok(config)
    }
}