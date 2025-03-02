#[macro_use] extern crate rocket;

use std::sync::{Arc, Mutex};
use dotenv::dotenv;
use log::{info, LevelFilter};
use rocket::{Build, Rocket};

mod config;
mod twilio;
mod bot;
mod api;
mod utils;

use crate::bot::session::SessionStore;
use crate::bot::ws_client::WebSocketManager;

/// Application entry point
#[launch]
fn rocket() -> Rocket<Build> {
    // Initialize logging
    env_logger::builder()
        .filter_level(LevelFilter::Info)
        .parse_env("LOG_LEVEL")
        .init();

    // Load environment variables from .env file if it exists
    dotenv().ok();

    info!("Starting Twilio Bot service");

    // Load configuration from environment variables
    let config = config::Config::from_env();
    info!("Configuration loaded");

    // Create session store
    let session_store = Arc::new(Mutex::new(SessionStore::new()));
    info!("Session store initialized");

    // Create WebSocket manager
    let ws_manager = Arc::new(WebSocketManager::new());
    info!("WebSocket manager initialized");

    // Build Rocket instance with routes and state
    rocket::build()
        .manage(config)
        .manage(session_store)
        .manage(ws_manager)
        .mount("/", api::routes())
        .mount("/twilio", twilio::routes())
}