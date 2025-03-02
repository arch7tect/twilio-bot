use std::sync::{Arc, Mutex};
use log::{debug, error, info};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_tungstenite::tungstenite::Message;
use futures::StreamExt;

use crate::bot::session::{MessageType, SessionStore};

/// Message received from the backend WebSocket
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WsMessage {
    /// Message type (e.g., "message", "eos", "timeout")
    pub r#type: String,
    /// Message content
    #[serde(default)]
    pub message: String,
    /// Optional metadata
    #[serde(default)]
    pub metadata: Value,
}

/// WebSocket client for a session
pub struct WebSocketClient {
    /// Session ID
    pub session_id: String,
    /// WebSocket URL
    pub ws_url: String,
    /// Whether the client is connected
    pub connected: bool,
}

impl WebSocketClient {
    /// Create a new WebSocket client
    pub fn new(session_id: String, ws_url: String) -> Self {
        WebSocketClient {
            session_id,
            ws_url,
            connected: false,
        }
    }
    
    /// Start the WebSocket client
    pub async fn start(&mut self, sessions: Arc<Mutex<SessionStore>>) {
        let url = format!("{}?session_id={}", self.ws_url, self.session_id);
        info!("Connecting to WebSocket server at {}", url);
        
        // Use tokio-tungstenite to connect
        match tokio_tungstenite::connect_async(&url).await {
            Ok((ws_stream, _)) => {
                info!("Connected to WebSocket server for session {}", self.session_id);
                self.connected = true;
                
                // Split the WebSocket stream - we only need the read part
                let (_, read) = ws_stream.split();
                
                // Clone sessions for async tasks
                let sessions_clone = sessions.clone();
                let session_id_clone = self.session_id.clone();
                
                // Spawn task for receiving messages
                let mut reader = read;
                tokio::spawn(async move {
                    while let Some(msg_result) = reader.next().await {
                        match msg_result {
                            Ok(msg) => {
                                if let Message::Text(text) = msg {
                                    debug!("Received WebSocket message: {}", text);
                                    
                                    // Parse the message
                                    if let Ok(ws_msg) = serde_json::from_str::<WsMessage>(&text) {
                                        let mut store = sessions_clone.lock().unwrap();
                                        if let Some(session) = store.get_session_mut(&session_id_clone) {
                                            match ws_msg.r#type.as_str() {
                                                "message" => {
                                                    if let Err(e) = session.message_tx.try_send(MessageType::Text(ws_msg.message)) {
                                                        error!("Failed to forward WebSocket message: {}", e);
                                                    }
                                                },
                                                "eos" => {
                                                    if let Err(e) = session.message_tx.try_send(MessageType::EndOfStream) {
                                                        error!("Failed to forward EOS: {}", e);
                                                    }
                                                },
                                                "timeout" => {
                                                    if let Err(e) = session.message_tx.try_send(MessageType::EndOfConversation) {
                                                        error!("Failed to forward timeout: {}", e);
                                                    }
                                                },
                                                _ => debug!("Unknown WebSocket message type: {}", ws_msg.r#type),
                                            }
                                        }
                                    }
                                }
                            },
                            Err(e) => {
                                error!("WebSocket error: {}", e);
                                break;
                            }
                        }
                    }
                    debug!("WebSocket receiver task ended for session {}", session_id_clone);
                });
            },
            Err(e) => {
                error!("Failed to connect to WebSocket server: {}", e);
            }
        }
    }
}

/// WebSocket client manager
pub struct WebSocketManager {
    clients: Arc<Mutex<std::collections::HashMap<String, Arc<Mutex<WebSocketClient>>>>>,
}

impl WebSocketManager {
    /// Create a new WebSocket manager
    pub fn new() -> Self {
        WebSocketManager {
            clients: Arc::new(Mutex::new(std::collections::HashMap::new())),
        }
    }
    
    /// Get or create a WebSocket client for a session
    pub async fn get_or_create_client(
        &self,
        session_id: &str,
        ws_url: &str,
        sessions: Arc<Mutex<SessionStore>>,
    ) -> Arc<Mutex<WebSocketClient>> {
        let mut clients = self.clients.lock().unwrap();
        
        if let Some(client) = clients.get(session_id) {
            return client.clone();
        }
        
        // Create a new client
        let client = WebSocketClient::new(
            session_id.to_string(),
            ws_url.to_string(),
        );
        
        let client_arc = Arc::new(Mutex::new(client));
        clients.insert(session_id.to_string(), client_arc.clone());
        
        // Start the client in a background task
        let client_clone = client_arc.clone();
        let sessions_clone = sessions.clone();
        
        tokio::spawn(async move {
            let mut client = client_clone.lock().unwrap();
            client.start(sessions_clone).await;
        });
        
        client_arc
    }
    
    /// Remove a client
    pub fn remove_client(&self, session_id: &str) {
        let mut clients = self.clients.lock().unwrap();
        clients.remove(session_id);
    }
}
