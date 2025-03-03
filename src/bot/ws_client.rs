use std::sync::Arc;
use log::{debug, error, info};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_tungstenite::tungstenite::Message;
use futures::StreamExt;
use tokio::sync::RwLock;

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
    /// Last reconnect attempt time
    pub last_reconnect_attempt: std::time::Instant,
    /// Number of consecutive connection failures
    pub consecutive_failures: usize,
}

impl WebSocketClient {
    /// Create a new WebSocket client
    pub fn new(session_id: String, ws_url: String) -> Self {
        WebSocketClient {
            session_id,
            ws_url,
            connected: false,
            last_reconnect_attempt: std::time::Instant::now(),
            consecutive_failures: 0,
        }
    }
    
    /// Check if the client is connected and reconnect if needed
    pub async fn ensure_connected(&mut self, sessions: Arc<RwLock<SessionStore>>) -> bool {
        if !self.connected {
            // Rate limit reconnect attempts
            let now = std::time::Instant::now();
            let elapsed = now.duration_since(self.last_reconnect_attempt).as_secs();
            
            // Implement exponential backoff
            let backoff_seconds = if self.consecutive_failures > 0 {
                let base_delay = 5;  // 5 seconds base delay
                std::cmp::min(300, base_delay * (2_u64.pow(self.consecutive_failures as u32 - 1)))
            } else {
                0  // No delay for first attempt
            };
            
            if elapsed < backoff_seconds {
                return false;
            }
            
            self.last_reconnect_attempt = now;
            self.start(sessions).await;
        }
        
        self.connected
    }
    
    /// Start the WebSocket client
    pub async fn start(&mut self, sessions: Arc<RwLock<SessionStore>>) {
        const MAX_RECONNECT_ATTEMPTS: usize = 5;
        
        let url = format!("{}?session_id={}", self.ws_url, self.session_id);
        info!("Connecting to WebSocket server at {}", url);
        
        match tokio_tungstenite::connect_async(&url).await {
            Ok((ws_stream, _)) => {
                info!("Connected to WebSocket server for session {}", self.session_id);
                self.connected = true;
                self.consecutive_failures = 0;
                
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
                                        let mut store = sessions_clone.write().await;
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
                
                // Start heartbeat
                self.start_heartbeat().await;
            },
            Err(e) => {
                error!("Failed to connect to WebSocket server: {}", e);
                self.connected = false;
                self.consecutive_failures += 1;
                
                if self.consecutive_failures >= MAX_RECONNECT_ATTEMPTS {
                    error!("Maximum consecutive reconnect attempts reached for session {}", self.session_id);
                }
            }
        }
    }
    
    /// Start a heartbeat to keep the connection alive
    pub async fn start_heartbeat(&self) {
        let session_id = self.session_id.clone();
        
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(30));
            
            loop {
                interval.tick().await;
                debug!("Sending heartbeat for session {}", session_id);
                // In a real implementation, you would send a WebSocket ping frame
                // or a custom keep-alive message depending on the backend protocol
            }
        });
    }
}

/// WebSocket client manager
pub struct WebSocketManager {
    clients: Arc<RwLock<std::collections::HashMap<String, Arc<RwLock<WebSocketClient>>>>>,
}

impl WebSocketManager {
    /// Create a new WebSocket manager
    pub fn new() -> Self {
        WebSocketManager {
            clients: Arc::new(RwLock::new(std::collections::HashMap::new())),
        }
    }
    
    /// Get or create a WebSocket client for a session
    pub async fn get_or_create_client(
        &self,
        session_id: &str,
        ws_url: &str,
        sessions: Arc<RwLock<SessionStore>>,
    ) -> Arc<RwLock<WebSocketClient>> {
        let clients_read = self.clients.read().await;
        
        if let Some(client) = clients_read.get(session_id) {
            return client.clone();
        }
        
        // Release read lock before acquiring write lock
        drop(clients_read);
        
        // Acquire write lock to create a new client
        let mut clients_write = self.clients.write().await;
        
        // Check again in case another thread created the client
        if let Some(client) = clients_write.get(session_id) {
            return client.clone();
        }
        
        // Create a new client
        let client = WebSocketClient::new(
            session_id.to_string(),
            ws_url.to_string(),
        );
        
        let client_arc = Arc::new(RwLock::new(client));
        clients_write.insert(session_id.to_string(), client_arc.clone());
        
        // Start the client in a background task
        let client_clone = client_arc.clone();
        let sessions_clone = sessions.clone();
        
        tokio::spawn(async move {
            let mut client = client_clone.write().await;
            client.start(sessions_clone).await;
        });
        
        client_arc
    }
    
    /// Remove a client
    pub async fn remove_client(&self, session_id: &str) {
        let mut clients = self.clients.write().await;
        clients.remove(session_id);
    }
    
    /// Check and reconnect all disconnected clients
    pub async fn check_connections(&self, sessions: Arc<RwLock<SessionStore>>) {
        let clients_read = self.clients.read().await;
        
        for (session_id, client_arc) in clients_read.iter() {
            let mut client = client_arc.write().await;
            if !client.connected {
                info!("Attempting to reconnect WebSocket for session {}", session_id);
                client.ensure_connected(sessions.clone()).await;
            }
        }
    }
    
    /// Start a periodic connection check task
    pub fn start_connection_checker(self: &Arc<Self>, sessions: Arc<RwLock<SessionStore>>) {
        let self_clone = self.clone();
        let sessions_clone = sessions.clone();
        
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(60));
            
            loop {
                interval.tick().await;
                self_clone.check_connections(sessions_clone.clone()).await;
            }
        });
    }
}