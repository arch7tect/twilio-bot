use std::env;
use serde::{Deserialize, Serialize};

/// Application configuration loaded from environment variables
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    // Twilio configuration
    pub twilio_account_sid: String,
    pub twilio_auth_token: String,
    pub from_number: String,
    pub webhook_url: String,
    pub webhook_port: u16,
    pub twilio_voice: String,
    pub speech_model: String,
    pub default_timeout: u32,
    pub partial_processing: bool,
    pub twilio_language: Option<String>,
    pub twilio_region: Option<String>,
    pub twilio_edge: Option<String>,
    
    // Backend configuration
    pub backend_url: String,
    pub authorization_token: Option<String>,
    pub backend_ws_url: String,
}

impl Config {
    /// Create configuration from environment variables
    pub fn from_env() -> Self {
        Config {
            // Twilio configuration
            twilio_account_sid: env::var("TWILIO_ACCOUNT_SID")
                .expect("TWILIO_ACCOUNT_SID must be set"),
            twilio_auth_token: env::var("TWILIO_AUTH_TOKEN")
                .expect("TWILIO_AUTH_TOKEN must be set"),
            from_number: env::var("FROM_NUMBER")
                .expect("FROM_NUMBER must be set"),
            webhook_url: env::var("TWILIO_WEBHOOK_URL")
                .expect("TWILIO_WEBHOOK_URL must be set"),
            webhook_port: env::var("FLAMETREE_CALLBACK_PORT")
                .unwrap_or_else(|_| "8000".to_string())
                .parse()
                .expect("FLAMETREE_CALLBACK_PORT must be a valid port number"),
            twilio_voice: env::var("TWILIO_VOICE")
                .unwrap_or_else(|_| "Polly.Salli".to_string()),
            speech_model: env::var("SPEECH_MODEL")
                .unwrap_or_else(|_| "googlev2_telephony".to_string()),
            default_timeout: env::var("DEFAULT_TIMEOUT")
                .unwrap_or_else(|_| "10".to_string())
                .parse()
                .expect("DEFAULT_TIMEOUT must be a valid number"),
            partial_processing: env::var("PARTIAL_PROCESSING")
                .unwrap_or_else(|_| "true".to_string())
                .to_lowercase() == "true",
            twilio_language: env::var("TWILIO_LANGUAGE").ok(),
            twilio_region: env::var("TWILIO_REGION")
                .ok()
                .filter(|s| !s.is_empty()),
            twilio_edge: env::var("TWILIO_EDGE")
                .ok()
                .filter(|s| !s.is_empty()),
                
            // Backend configuration
            backend_url: env::var("BACKEND_URL")
                .expect("BACKEND_URL must be set"),
            authorization_token: env::var("AUTHORIZATION_TOKEN").ok(),
            backend_ws_url: env::var("BACKEND_WS_URL")
                .expect("BACKEND_WS_URL must be set"),
        }
    }
}
