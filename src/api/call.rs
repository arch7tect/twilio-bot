use std::sync::{Arc, Mutex};
use log::{debug, error};
use rocket::{post, serde::json::Json, State, http::Status};
use serde::{Deserialize, Serialize};

use crate::bot::session::SessionStore;
use crate::config::Config;
use crate::twilio::client::TwilioClient;
use crate::twilio::twiml::create_voice_response;
use crate::twilio::handlers::MakeCallRequest;

/// Response for the make call API endpoint
#[derive(Debug, Serialize)]
pub struct MakeCallResponse {
    pub message: String,
    pub call_id: String,
}

/// Forward API endpoint for making outbound calls
#[post("/call", format = "json", data = "<request>")]
pub async fn make_call(
    request: Json<MakeCallRequest>,
    config: &State<Config>,
) -> Result<Json<MakeCallResponse>, Status> {
    debug!("API call request for {}", request.to_number);
    
    // Create Twilio client
    let twilio_client = match TwilioClient::new(
        config.twilio_account_sid.clone(),
        config.twilio_auth_token.clone(),
        config.twilio_region.clone(),
        config.twilio_edge.clone()
    ) {
        Ok(client) => client,
        Err(e) => {
            error!("Failed to create Twilio client: {}", e);
            return Err(Status::InternalServerError);
        }
    };
    
    // Create empty TwiML response
    let twiml = create_voice_response("", &config, config.default_timeout, "auto");
    
    // Make the call
    let call = match twilio_client.create_call(
        &request.to_number,
        &config.from_number,
        &twiml,
        &format!("{}{}", config.webhook_url, "/status_callback")
    ).await {
        Ok(call) => call,
        Err(e) => {
            error!("Failed to create call: {}", e);
            return Err(Status::InternalServerError);
        }
    };
    
    Ok(Json(MakeCallResponse {
        message: "Call initiated successfully".to_string(),
        call_id: call.sid,
    }))
}
