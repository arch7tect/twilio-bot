pub mod client;
pub mod twiml;
pub mod handlers;

use rocket::{Route, routes};

/// Get all routes for the Twilio module
pub fn routes() -> Vec<Route> {
    routes![
        handlers::handle_incoming_call,
        handlers::handle_call_status,
        handlers::handle_call_transcription,
        handlers::handle_partial_callback,
        handlers::handle_call_queue,
        handlers::make_call,
    ]
}
