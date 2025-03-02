use std::sync::Arc;
use log::{debug, error, info};
use rocket::{State, post, serde::json::Json, form::Form, http::Status};
use crate::utils::Xml;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::bot::backend::BackendClient;
use crate::bot::session::{MessageType, Session, SessionStore};
use crate::config::Config;
use crate::twilio::client::TwilioClient;
use crate::twilio::twiml::{create_hangup_response, create_voice_response, ends_with_sentence_punctuation};

/// Form data for Twilio webhook callbacks
#[derive(FromForm, Debug)]
pub struct TwilioCallbackForm {
    #[field(name = "CallSid")]
    call_sid: Option<String>,
    
    #[field(name = "CallStatus")]
    call_status: Option<String>,
    
    #[field(name = "From")]
    from_number: Option<String>,
    
    #[field(name = "SpeechResult")]
    speech_result: Option<String>,
    
    #[field(name = "UnstableSpeechResult")]
    unstable_speech_result: Option<String>,
}

/// Request for making a new outbound call
#[derive(Debug, Deserialize)]
pub struct MakeCallRequest {
    to_number: String,
    env_info: Option<serde_json::Value>,
}

/// Response for the make call endpoint
#[derive(Debug, Serialize)]
pub struct MakeCallResponse {
    message: String,
    session_id: String,
}

/// Handle incoming calls from Twilio
#[post("/incoming_callback", data = "<form>")]
pub async fn handle_incoming_call(
    form: Form<TwilioCallbackForm>,
    sessions: &State<Arc<RwLock<SessionStore>>>,
    config: &State<Config>,
) -> Xml<String> {
    let form = form.into_inner();
    let call_sid = form.call_sid.unwrap_or_default();
    let from_number = form.from_number.unwrap_or_default();
    
    debug!("Incoming call from {} with SID {}", from_number, call_sid);
    
    // Create a new backend client with circuit breaker enabled
    let backend_client = match BackendClient::new(
        &config.backend.url, 
        config.backend.authorization_token.clone(),
        config.backend.enable_circuit_breaker
    ) {
        Ok(client) => client,
        Err(e) => {
            error!("Failed to create backend client: {}", e);
            return Xml(create_hangup_response(
                Some("Sorry, we're experiencing technical difficulties."), 
                &config.twilio
            ));
        }
    };
    
    // Create a new session
    let mut session = Session::new(call_sid.clone(), from_number.clone(), "twilio".to_string(), Some(call_sid.clone()));
    
    // Initialize the session with the backend
    let args = vec![];
    let kwargs = serde_json::json!({}).as_object().unwrap().clone();
    
    match backend_client.open_session(
        &call_sid,
        &from_number,
        "twilio",
        Some(&call_sid),
        args,
        kwargs,
    ).await {
        Ok(response) => {
            // Extract greeting from response
            let greeting = if let Some(init_response) = response.metadata.get("initialization_response") {
                if let Some(greeting) = init_response.get("greeting") {
                    greeting.as_str().unwrap_or("Hello, welcome to our service.").to_string()
                } else {
                    "Hello, welcome to our service.".to_string()
                }
            } else {
                "Hello, welcome to our service.".to_string()
            };
            
            // Store session data
            session.metadata.insert("initialization_response".to_string(), 
                                    serde_json::json!({"greeting": greeting.clone()}));
            
            // Add session to store
            let mut store = sessions.write().await;
            store.add_session(session);
            
            debug!("Created new session for call {}", call_sid);
            Xml(create_voice_response(&greeting, &config.twilio, config.twilio.default_timeout, "auto"))
        },
        Err(e) => {
            error!("Failed to initialize session with backend: {}", e);
            Xml(create_hangup_response(
                Some("Sorry, we're experiencing technical difficulties."), 
                &config.twilio
            ))
        }
    }
}

/// Handle Twilio call status callbacks
#[post("/status_callback", data = "<form>")]
pub async fn handle_call_status(
    form: Form<TwilioCallbackForm>,
    sessions: &State<Arc<RwLock<SessionStore>>>,
    config: &State<Config>,
) -> Status {
    let form = form.into_inner();
    let call_status = form.call_status.unwrap_or_default();
    let call_sid = form.call_sid.unwrap_or_default();
    
    debug!("Call status update for {}: {}", call_sid, call_status);
    
    if call_status == "in-progress" {
        // Call is in progress, send greeting via TTS
        let store = sessions.read().await;
        if let Some(session) = store.get_session_by_conversation(&call_sid) {
            if let Some(greeting) = session.metadata.get("initialization_response")
                .and_then(|resp| resp.get("greeting"))
                .and_then(|greeting| greeting.as_str()) {
                
                // Create TwiML for greeting
                let twiml = create_voice_response(greeting, &config.twilio, config.twilio.default_timeout, "auto");
                
                // Update the call with the TwiML
                let twilio_client = match TwilioClient::new(
                    config.twilio.account_sid.clone(),
                    config.twilio.auth_token.clone(),
                    config.twilio.region.clone(),
                    config.twilio.edge.clone()
                ) {
                    Ok(client) => client,
                    Err(e) => {
                        error!("Failed to create Twilio client: {}", e);
                        return Status::InternalServerError;
                    }
                };
                
                // Use the retry-capable method with parameters from config
                if let Err(e) = twilio_client.update_call_with_retry(
                    &call_sid, 
                    &twiml,
                    config.backend.retry_attempts,
                    config.backend.retry_base_delay_ms
                ).await {
                    error!("Failed to update call with greeting: {}", e);
                    return Status::InternalServerError;
                }
            }
        }
    } else if ["completed", "busy", "no-answer", "canceled", "failed"].contains(&call_status.as_str()) {
        // Call has ended, close the session
        let session_id_option = {
            let store = sessions.read().await;
            store.conversation_to_session.get(&call_sid).cloned()
        };
        
        if let Some(session_id) = session_id_option {
            {
                let mut store = sessions.write().await;
                store.remove_session(&session_id);
            }
            debug!("Removed session {} for ended call {}", session_id, call_sid);
            
            // Close session with backend
            let backend_client = match BackendClient::new(
                &config.backend.url, 
                config.backend.authorization_token.clone(),
                config.backend.enable_circuit_breaker
            ) {
                Ok(client) => client,
                Err(e) => {
                    error!("Failed to create backend client: {}", e);
                    return Status::InternalServerError;
                }
            };
            
            // Use retry for closing sessions
            if let Err(e) = backend_client.run_with_retry(
                &session_id,
                "close_session", 
                serde_json::json!({ "status": call_status }).as_object().unwrap().clone(),
                config.backend.retry_attempts,
                config.backend.retry_base_delay_ms
            ).await {
                error!("Failed to close session with backend: {}", e);
            }
        }
    }
    
    Status::Ok
}

// Other handler methods would be similar, using the new client signatures 
// and the RwLock for accessing the session store
