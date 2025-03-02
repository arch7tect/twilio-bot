use reqwest::{Client, Error as ReqwestError};
use base64::{Engine as _, engine::general_purpose};
use serde::{Deserialize, Serialize};
use log::{debug, error, info};
use std::collections::HashMap;
use std::fmt;

/// Represents a Twilio call resource
#[derive(Debug, Deserialize)]
pub struct TwilioCall {
    pub sid: String,
    pub status: String,
}

/// Error type for Twilio client operations
#[derive(Debug)]
pub enum TwilioError {
    RequestError(ReqwestError),
    ApiError(String),
    StatusError(u16, String),
    RetryExhausted(Box<TwilioError>),
}

impl fmt::Display for TwilioError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TwilioError::RequestError(err) => write!(f, "Request error: {}", err),
            TwilioError::ApiError(err) => write!(f, "API error: {}", err),
            TwilioError::StatusError(status, msg) => write!(f, "Status {} error: {}", status, msg),
            TwilioError::RetryExhausted(err) => write!(f, "Retry exhausted: {}", err),
        }
    }
}

impl std::error::Error for TwilioError {}

impl From<ReqwestError> for TwilioError {
    fn from(err: ReqwestError) -> Self {
        TwilioError::RequestError(err)
    }
}

/// Twilio API client
pub struct TwilioClient {
    client: Client,
    account_sid: String,
    auth_token: String,
    region: Option<String>,
    edge: Option<String>,
}

impl TwilioClient {
    /// Create a new Twilio client
    pub fn new(
        account_sid: String,
        auth_token: String,
        region: Option<String>,
        edge: Option<String>,
    ) -> Result<Self, TwilioError> {
        let client = Client::builder()
            .build()
            .map_err(TwilioError::from)?;
            
        Ok(TwilioClient {
            client,
            account_sid,
            auth_token,
            region,
            edge,
        })
    }
    
    /// Get the base URL for Twilio API requests
    fn base_url(&self) -> String {
        let region_prefix = match &self.region {
            Some(region) if !region.is_empty() => format!("{}-", region),
            _ => String::new(),
        };
        
        let edge_prefix = match &self.edge {
            Some(edge) if !edge.is_empty() => format!("{}-", edge),
            _ => String::new(),
        };
        
        format!(
            "https://{}api.{}twilio.com/2010-04-01/Accounts/{}", 
            edge_prefix,
            region_prefix,
            self.account_sid
        )
    }
    
    /// Get the authorization header for Twilio API requests
    fn auth_header(&self) -> String {
        let credentials = format!("{}:{}", self.account_sid, self.auth_token);
        format!("Basic {}", general_purpose::STANDARD.encode(credentials))
    }
    
    /// Create a new outbound call
    pub async fn create_call(
        &self,
        to: &str,
        from: &str,
        twiml: &str,
        status_callback: &str,
    ) -> Result<TwilioCall, TwilioError> {
        let url = format!("{}/Calls.json", self.base_url());
        debug!("Creating call to {} from {}", to, from);
        
        let mut form = HashMap::new();
        form.insert("To", to);
        form.insert("From", from);
        form.insert("Twiml", twiml);
        form.insert("StatusCallback", status_callback);
        form.insert("StatusCallbackEvent", 
                   "initiated answered completed busy no-answer canceled failed");
        form.insert("StatusCallbackMethod", "POST");
        form.insert("Timeout", "600");
        
        let response = self.client.post(&url)
            .header("Authorization", self.auth_header())
            .form(&form)
            .send()
            .await?;
            
        let status = response.status();
        if !status.is_success() {
            let error_text = response.text().await?;
            error!("Failed to create call: {}", error_text);
            return Err(TwilioError::StatusError(status.as_u16(), error_text));
        }
        
        let call: TwilioCall = response.json().await?;
        info!("Created call with SID: {}", call.sid);
        Ok(call)
    }
    
    /// Create a new outbound call with retry capability
    pub async fn create_call_with_retry(
        &self,
        to: &str,
        from: &str,
        twiml: &str,
        status_callback: &str,
        max_retries: usize,
        base_delay_ms: u64,
    ) -> Result<TwilioCall, TwilioError> {
        let mut attempts = 0;
        let mut last_error = None;
        
        while attempts <= max_retries {
            match self.create_call(to, from, twiml, status_callback).await {
                Ok(result) => return Ok(result),
                Err(e) => {
                    attempts += 1;
                    last_error = Some(e);
                    
                    if attempts <= max_retries {
                        let delay = base_delay_ms * 2u64.pow(attempts as u32 - 1);
                        debug!("Retrying Twilio call creation, attempt {}/{} after {}ms", 
                              attempts, max_retries, delay);
                        tokio::time::sleep(tokio::time::Duration::from_millis(delay)).await;
                    }
                }
            }
        }
        
        Err(TwilioError::RetryExhausted(Box::new(
            last_error.unwrap_or(TwilioError::ApiError("Maximum retries exceeded".to_string()))
        )))
    }
    
    /// Update an existing call with new TwiML
    pub async fn update_call(&self, call_sid: &str, twiml: &str) -> Result<(), TwilioError> {
        let url = format!("{}/Calls/{}.json", self.base_url(), call_sid);
        debug!("Updating call {}", call_sid);
        
        let mut form = HashMap::new();
        form.insert("Twiml", twiml);
        
        let response = self.client.post(&url)
            .header("Authorization", self.auth_header())
            .form(&form)
            .send()
            .await?;
            
        let status = response.status();
        if !status.is_success() {
            let error_text = response.text().await?;
            error!("Failed to update call {}: {}", call_sid, error_text);
            return Err(TwilioError::StatusError(status.as_u16(), error_text));
        }
        
        debug!("Successfully updated call {}", call_sid);
        Ok(())
    }
    
    /// Update an existing call with new TwiML with retry capability
    pub async fn update_call_with_retry(
        &self,
        call_sid: &str,
        twiml: &str,
        max_retries: usize,
        base_delay_ms: u64,
    ) -> Result<(), TwilioError> {
        let mut attempts = 0;
        let mut last_error = None;
        
        while attempts <= max_retries {
            match self.update_call(call_sid, twiml).await {
                Ok(result) => return Ok(result),
                Err(e) => {
                    attempts += 1;
                    last_error = Some(e);
                    
                    if attempts <= max_retries {
                        let delay = base_delay_ms * 2u64.pow(attempts as u32 - 1);
                        debug!("Retrying call update, attempt {}/{} after {}ms", 
                              attempts, max_retries, delay);
                        tokio::time::sleep(tokio::time::Duration::from_millis(delay)).await;
                    }
                }
            }
        }
        
        Err(TwilioError::RetryExhausted(Box::new(
            last_error.unwrap_or(TwilioError::ApiError("Maximum retries exceeded".to_string()))
        )))
    }
    
    /// List phone numbers for a specific phone number
    pub async fn list_phone_numbers(&self, phone_number: &str) -> Result<Vec<serde_json::Value>, TwilioError> {
        let url = format!("{}/IncomingPhoneNumbers.json?PhoneNumber={}", 
                         self.base_url(), urlencoding::encode(phone_number));
        debug!("Listing phone numbers for {}", phone_number);
        
        let response = self.client.get(&url)
            .header("Authorization", self.auth_header())
            .send()
            .await?;
            
        let status = response.status();
        if !status.is_success() {
            let error_text = response.text().await?;
            error!("Failed to list phone numbers: {}", error_text);
            return Err(TwilioError::StatusError(status.as_u16(), error_text));
        }
        
        let result: serde_json::Value = response.json().await?;
        let numbers = result["incoming_phone_numbers"].as_array()
            .ok_or_else(|| TwilioError::ApiError("No phone numbers found".to_string()))?
            .clone();
            
        Ok(numbers)
    }
    
    /// Update phone number configuration
    pub async fn update_phone_number(
        &self, 
        phone_number_sid: &str, 
        voice_url: &str
    ) -> Result<serde_json::Value, TwilioError> {
        let url = format!("{}/IncomingPhoneNumbers/{}.json", self.base_url(), phone_number_sid);
        debug!("Updating phone number {} with voice URL {}", phone_number_sid, voice_url);
        
        let mut form = HashMap::new();
        form.insert("VoiceUrl", voice_url);
        form.insert("VoiceMethod", "POST");
        
        let response = self.client.post(&url)
            .header("Authorization", self.auth_header())
            .form(&form)
            .send()
            .await?;
            
        let status = response.status();
        if !status.is_success() {
            let error_text = response.text().await?;
            error!("Failed to update phone number: {}", error_text);
            return Err(TwilioError::StatusError(status.as_u16(), error_text));
        }
        
        let result: serde_json::Value = response.json().await?;
        info!("Updated phone number {} with voice URL {}", phone_number_sid, voice_url);
        Ok(result)
    }
}