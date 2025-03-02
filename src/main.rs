#[macro_use] extern crate rocket;

use std::sync::Arc;
use dotenv::dotenv;
use log::{info, error, LevelFilter};
use rocket::{Build, Rocket};
use tokio::sync::RwLock;

mod config;
mod twilio;
mod bot;
mod api;
mod utils;

use crate::bot::session::{SessionStore, start_session_cleanup_task};
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
    let config = match config::Config::from_env() {
        Ok(config) => config,
        Err(e) => {
            error!("Configuration error: {}", e);
            std::process::exit(1);
        }
    };
    info!("Configuration loaded and validated");

    // Create session store
    let session_store = Arc::new(RwLock::new(SessionStore::new()));
    info!("Session store initialized");

    // Start the session cleanup task
    start_session_cleanup_task(
        session_store.clone(), 
        config.session.cleanup_interval_minutes,
        config.session.max_age_minutes
    );
    info!("Session cleanup task started");

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