use std::sync::Arc;
use log::{debug, error, info};
use rocket::{State, post, serde::json::Json, form::Form, http::Status};
use crate::utils::Xml;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use std::collections::HashMap;

use crate::bot::backend::BackendClient;
use crate::bot::session::{MessageType, Session, SessionStore};
use crate::config::Config;
use crate::twilio::client::TwilioClient;
use crate::twilio::twiml::{create_hangup_response, create_voice_response, ends_with_sentence_punctuation};
use crate::bot::ws_client::WebSocketManager;

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
    pub to_number: String,
    pub env_info: Option<serde_json::Value>,
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
    ws_manager: &State<Arc<WebSocketManager>>,
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
    let kwargs = HashMap::new();
    
    match backend_client.open_session(
        &call_sid,
        &from_number,
        "twilio",
        Some(&call_sid),
        args,
        kwargs
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
            let session_id = {
                let mut store = sessions.write().await;
                store.add_session(session)
            };
            
            // Create WebSocket client for this session if needed
            if !config.backend.ws_url.is_empty() {
                ws_manager.get_or_create_client(
                    &response.session.session_id,
                    &config.backend.ws_url,
                    sessions.inner().clone()
                ).await;
            }
            
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
        let greeting = {
            let store = sessions.read().await;
            if let Some(session) = store.get_session_by_conversation(&call_sid) {
                session.metadata.get("initialization_response")
                    .and_then(|resp| resp.get("greeting"))
                    .and_then(|greeting| greeting.as_str())
                    .map(|s| s.to_string())
            } else {
                None
            }
        };
        
        if let Some(greeting_text) = greeting {
            // Create TwiML for greeting
            let twiml = create_voice_response(&greeting_text, &config.twilio, config.twilio.default_timeout, "auto");
            
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
    } else if ["completed", "busy", "no-answer", "canceled", "failed"].contains(&call_status.as_str()) {
        // Call has ended, close the session
        let session_id_option = {
            let store = sessions.read().await;
            store.get_session_id_by_conversation(&call_sid)
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
            
            if let Err(e) = backend_client.close_session(&session_id, Some(&call_status)).await {
                error!("Failed to close session with backend: {}", e);
            }
        }
    }
    
    Status::Ok
}

/// Handle transcription callbacks from Twilio
#[post("/transcription_callback", data = "<form>")]
pub async fn handle_call_transcription(
    form: Form<TwilioCallbackForm>,
    sessions: &State<Arc<RwLock<SessionStore>>>,
    config: &State<Config>,
) -> Xml<String> {
    let form = form.into_inner();
    let call_sid = form.call_sid.unwrap_or_default();
    let transcription = form.speech_result.unwrap_or_default();
    
    debug!("Transcription for call {}: {}", call_sid, transcription);
    
    // Check if session exists and get necessary state
    let (session_id, session_ends, is_same_result, has_generation) = {
        let mut store = sessions.write().await;
        
        if let Some(session) = store.get_session_by_conversation_mut(&call_sid) {
            if session.session_ends {
                debug!("Session for call {} has already ended", call_sid);
                return Xml(create_hangup_response(None, &config.twilio));
            }
            
            // Check if we need to generate new response
            let is_same = session.unstable_speech_result_is_the_same(&transcription);
            let has_gen = session.generation;
            
            (
                session.session_id.clone(),
                session.session_ends,
                is_same,
                has_gen
            )
        } else {
            // Session not found
            error!("No session found for call {}", call_sid);
            return Xml(create_hangup_response(Some("Sorry, your session has expired."), &config.twilio));
        }
    };
    
    // Check if we need to generate new response
    let should_generate = if has_generation {
        !is_same_result
    } else {
        true
    };
    
    if should_generate {
        // Create backend client
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
        
        // Update session state
        {
            let mut store = sessions.write().await;
            if let Some(session) = store.get_session_mut(&session_id) {
                session.run_in_progress = true;
                session.speech_in_progress = false;
                session.unstable_speech_result = Some(transcription.clone());
                session.generation = true;
            }
        }
        
        // Send transcription to backend with retry
        let kwargs = HashMap::new();
        match backend_client.run_with_retry(
            &session_id, 
            &transcription, 
            kwargs,
            config.backend.retry_attempts,
            config.backend.retry_base_delay_ms
        ).await {
            Ok(result) => {
                // Update session state
                let session_should_end = {
                    let mut store = sessions.write().await;
                    if let Some(session) = store.get_session_mut(&session_id) {
                        session.generation = false;
                        
                        // Check if session should end
                        let ends = result.get("metadata")
                            .and_then(|m| m.get("SESSION_ENDS"))
                            .and_then(|e| e.as_bool())
                            .unwrap_or(false);
                            
                        if ends {
                            session.session_ends = true;
                            debug!("Session for call {} will end after this response", call_sid);
                        }
                        
                        ends
                    } else {
                        false
                    }
                };
                
                if session_should_end {
                    if let Some(response) = result.get("response").and_then(|r| r.as_str()) {
                        return Xml(create_hangup_response(Some(response), &config.twilio));
                    } else {
                        return Xml(create_hangup_response(None, &config.twilio));
                    }
                }
                
                // Check for special code response format
                if let Some(response) = result.get("response").and_then(|r| r.as_str()) {
                    if response.starts_with("Code:") {
                        // Handle DTMF code
                        let code = &response[5..].trim();
                        debug!("Returning DTMF code: {}", code);
                        
                        // Build TwiML with play digits
                        let mut twiml = crate::twilio::twiml::TwiML::new();
                        let action_url = format!("{}{}", config.inner().twilio.webhook_url, "/transcription_callback");
                        let partial_callback_url = format!("{}{}", config.inner().twilio.webhook_url, "/partial_callback");

                        let gather_options = crate::twilio::twiml::GatherOptions {
                            input: Some("speech"),
                            action: Some(&action_url),  // Reference to longer-lived string
                            method: Some("POST"),
                            timeout: Some(10),
                            speech_timeout: Some("auto"),
                            barge_in: Some(true),
                            partial_result_callback: Some(&partial_callback_url),  // Reference to longer-lived string
                            speech_model: Some(&config.inner().twilio.speech_model),
                            language: config.inner().twilio.language.as_deref(),
                            say_text: Some(code),
                            voice: Some(&config.inner().twilio.voice),
                        };
                        
                        twiml = twiml.gather(gather_options);
                        twiml = twiml.play_digits(code);
                        
                        return Xml(twiml.build());
                    } else {
                        // Normal text response
                        return Xml(create_voice_response(response, &config.twilio, config.twilio.default_timeout, "auto"));
                    }
                }
                
                // Default response if no response text found
                Xml(create_voice_response(
                    "I'm sorry, I didn't understand that.", 
                    &config.twilio, 
                    config.twilio.default_timeout, 
                    "auto"
                ))
            },
            Err(e) => {
                // Update session state
                {
                    let mut store = sessions.write().await;
                    if let Some(session) = store.get_session_mut(&session_id) {
                        session.generation = false;
                    }
                }
                
                error!("Failed to run backend command: {}", e);
                Xml(create_voice_response(
                    "I'm sorry, I'm having trouble processing your request right now.", 
                    &config.twilio, 
                    config.twilio.default_timeout, 
                    "auto"
                ))
            }
        }
    } else {
        // Re-use previous response
        Xml(create_voice_response(
            "Could you please repeat that?", 
            &config.twilio, 
            config.twilio.default_timeout, 
            "auto"
        ))
    }
}

/// Handle partial speech results from Twilio
#[post("/partial_callback", data = "<form>")]
pub async fn handle_partial_callback(
    form: Form<TwilioCallbackForm>,
    sessions: &State<Arc<RwLock<SessionStore>>>,
    config: &State<Config>,
) -> Status {
    let form = form.into_inner();
    
    if !config.twilio.partial_processing {
        return Status::Ok;
    }
    
    let call_sid = form.call_sid.unwrap_or_default();
    let unstable_speech_result = form.unstable_speech_result.unwrap_or_default();
    
    debug!("Partial speech result for call {}: {}", call_sid, unstable_speech_result);
    
    // Check if speech ends with sentence punctuation
    if !ends_with_sentence_punctuation(&unstable_speech_result) {
        return Status::Ok;
    }
    
    // Get session info with write lock
    let (session_id, should_process) = {
        let mut store = sessions.write().await;
        
        if let Some(session) = store.get_session_by_conversation_mut(&call_sid) {
            if session.session_ends {
                return Status::Ok;
            }
            
            let should_process = !session.generation || 
                                !session.unstable_speech_result_is_the_same(&unstable_speech_result);
            
            if should_process {
                // Update session state
                session.run_in_progress = true;
                session.speech_in_progress = false;
                session.unstable_speech_result = Some(unstable_speech_result.clone());
                session.generation = true;
            }
            
            (session.session_id.clone(), should_process)
        } else {
            return Status::Ok;
        }
    };
    
    if should_process {
        // Start speculative generation
        debug!("Starting speculative generation for partial result: {}", unstable_speech_result);
        
        // Create backend client
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
        
        // Send unstable speech result to backend as a "start" command
        if let Err(e) = backend_client.start(&session_id, &unstable_speech_result).await {
            error!("Failed to start backend generation: {}", e);
            
            // Reset generation flag on error
            let mut store = sessions.write().await;
            if let Some(session) = store.get_session_mut(&session_id) {
                session.generation = false;
            }
            
            return Status::InternalServerError;
        }
    }
    
    Status::Ok
}

/// Handle queue callback from Twilio
#[post("/queue_callback", data = "<form>")]
pub async fn handle_call_queue(
    form: Form<TwilioCallbackForm>,
    sessions: &State<Arc<RwLock<SessionStore>>>,
    config: &State<Config>,
) -> Xml<String> {
    let form = form.into_inner();
    let call_sid = form.call_sid.unwrap_or_default();
    
    debug!("Queue callback for call {}", call_sid);
    
    let mut buffer = Vec::new();
    let mut eoc = false;
    let mut eos = false;
    
    // Process message queue
    {
        let mut store = sessions.write().await;
        
        if let Some(session) = store.get_session_by_conversation_mut(&call_sid) {
            // In a real implementation, would process the queue here
            // For now, just check if there are any pending messages
            
            // Example of how to process the queue:
            let mut messages = Vec::new();
            while let Ok(message) = session.message_rx.try_recv() {
                messages.push(message);
            }
            
            for message in messages {
                match message {
                    MessageType::Text(text) => buffer.push(text),
                    MessageType::EndOfConversation => eoc = true,
                    MessageType::EndOfStream => eos = true,
                }
            }
        }
    }
    
    let text = buffer.join(" ");
    
    if eoc {
        Xml(create_hangup_response(if text.is_empty() { None } else { Some(&text) }, &config.twilio))
    } else {
        let timeout = if eos { config.twilio.default_timeout } else { 1 };
        let speech_timeout = if eos { "auto" } else { "1" };
        
        let twiml = if text.is_empty() {
            create_voice_response("", &config.twilio, timeout, speech_timeout)
        } else {
            let mut response = create_voice_response(&text, &config.twilio, timeout, speech_timeout);
            
            // Add redirect
            response = response.replace("</Response>", 
                &format!("<Redirect>{}/queue_callback</Redirect></Response>", config.twilio.webhook_url));
            
            response
        };
        
        Xml(twiml)
    }
}

/// Make a new outbound call
#[post("/call", format = "json", data = "<request>")]
pub async fn make_call(
    request: Json<MakeCallRequest>,
    sessions: &State<Arc<RwLock<SessionStore>>>,
    ws_manager: &State<Arc<WebSocketManager>>,
    config: &State<Config>,
) -> Result<Json<MakeCallResponse>, Status> {
    let request = request.into_inner();
    
    debug!("Making outbound call to {}", request.to_number);
    
    // Create a new session
    let mut session = Session::new(
        "".to_string(),
        request.to_number.clone(), 
        "twilio".to_string(), 
        None
    );
    
    // Create backend client
    let backend_client = match BackendClient::new(
        &config.backend.url, 
        config.backend.authorization_token.clone(),
        config.backend.enable_circuit_breaker
    ) {
        Ok(client) => client,
        Err(e) => {
            error!("Failed to create backend client: {}", e);
            return Err(Status::InternalServerError);
        }
    };
    
    // Initialize session with backend
    let args = vec![];
    let kwargs = if let Some(env_info) = request.env_info {
        if let Some(obj) = env_info.as_object() {
            // Convert serde_json::Map to HashMap
            let mut map = HashMap::new();
            for (key, value) in obj {
                map.insert(key.clone(), value.clone());
            }
            map
        } else {
            HashMap::new()
        }
    } else {
        HashMap::new()
    };

    let session_response = match backend_client.open_session(
        "", 
        &request.to_number, 
        "twilio", 
        None,
        args,
        kwargs
    ).await {
        Ok(response) => response,
        Err(e) => {
            error!("Failed to initialize session with backend: {}", e);
            return Err(Status::InternalServerError);
        }
    };
    
    // Initialize WebSocket connection for session
    if !config.backend.ws_url.is_empty() {
        ws_manager.get_or_create_client(
            &session_response.session.session_id,
            &config.backend.ws_url,
            sessions.inner().clone()
        ).await;
    }
    
    // Create Twilio client
    let twilio_client = match TwilioClient::new(
        config.twilio.account_sid.clone(),
        config.twilio.auth_token.clone(),
        config.twilio.region.clone(),
        config.twilio.edge.clone()
    ) {
        Ok(client) => client,
        Err(e) => {
            error!("Failed to create Twilio client: {}", e);
            return Err(Status::InternalServerError);
        }
    };
    
    // Create empty TwiML response
    let twiml = create_voice_response("", &config.twilio, config.twilio.default_timeout, "auto");
    
    // Make the call with retry
    let call = match twilio_client.create_call_with_retry(
        &request.to_number,
        &config.twilio.from_number,
        &twiml,
        &format!("{}{}", config.twilio.webhook_url, "/status_callback"),
        config.backend.retry_attempts,
        config.backend.retry_base_delay_ms
    ).await {
        Ok(call) => call,
        Err(e) => {
            error!("Failed to create call: {}", e);
            return Err(Status::InternalServerError);
        }
    };
    
    // Update session with call SID
    session.conversation_id = Some(call.sid.clone());
    
    // Add session to store
    {
        let mut store = sessions.write().await;
        store.add_session(session);
    }
    
    // Update backend session with call SID
    if let Err(e) = backend_client.update_session(
        &session_response.session.session_id, 
        Some(&call.sid)
    ).await {
        error!("Failed to update session with call SID: {}", e);
    }
    
    Ok(Json(MakeCallResponse {
        message: "ok".to_string(),
        session_id: call.sid,
    }))
}