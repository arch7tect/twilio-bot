use std::collections::HashMap;
use std::sync::Arc;
use chrono::{DateTime, Utc, Duration};
use regex::Regex;
use rocket::tokio::sync::mpsc::{channel, Receiver, Sender};
use serde_json::Value;
use uuid::Uuid;
use log::{debug, info, error};

/// Types of messages that can be sent through the message queue
#[derive(Debug, Clone)]
pub enum MessageType {
    /// Text message
    Text(String),
    /// End of conversation signal
    EndOfConversation,
    /// End of stream signal
    EndOfStream,
}

/// Session state for a bot conversation
pub struct Session {
    /// Unique session identifier
    pub session_id: String,
    /// User identifier
    pub user_id: String,
    /// User name or phone number
    pub name: String,
    /// Bot type (e.g., "twilio")
    pub bot_type: String,
    /// External conversation identifier (e.g., Twilio CallSid)
    pub conversation_id: Option<String>,
    /// Sender for message queue
    pub message_tx: Sender<MessageType>,
    /// Receiver for message queue
    pub message_rx: Receiver<MessageType>,
    /// Session creation time
    pub creation_time: DateTime<Utc>,
    /// Last activity time
    pub last_activity_time: DateTime<Utc>,
    /// Whether speech is currently being processed
    pub speech_in_progress: bool,
    /// Whether a run operation is in progress
    pub run_in_progress: bool,
    /// Current unstable speech result
    pub unstable_speech_result: Option<String>,
    /// Whether generation is in progress
    pub generation: bool,
    /// Whether the session is ending
    pub session_ends: bool,
    /// Session metadata
    pub metadata: HashMap<String, Value>,
}

impl Session {
    /// Create a new session
    pub fn new(user_id: String, name: String, bot_type: String, conversation_id: Option<String>) -> Self {
        let (tx, rx) = channel(100);
        let now = Utc::now();
        
        Session {
            session_id: Uuid::new_v4().to_string(),
            user_id,
            name,
            bot_type,
            conversation_id,
            message_tx: tx,
            message_rx: rx,
            creation_time: now,
            last_activity_time: now,
            speech_in_progress: false,
            run_in_progress: false,
            unstable_speech_result: None,
            generation: false,
            session_ends: false,
            metadata: HashMap::new(),
        }
    }
    
    /// Check if the unstable speech result is the same as the previous one
    pub fn unstable_speech_result_is_the_same(&self, unstable_speech_result: &str) -> bool {
        if let Some(ref last_result) = self.unstable_speech_result {
            let normalize = |s: &str| {
                s.to_lowercase()
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .trim()
                    .to_string()
            };
            
            normalize(last_result) == normalize(unstable_speech_result)
        } else {
            false
        }
    }
    
    /// Check if the text ends with sentence punctuation
    pub fn ends_with_sentence_punctuation(text: &str) -> bool {
        let re = Regex::new(r".*[.!?]$").unwrap();
        re.is_match(text.trim())
    }
    
    /// Update the last activity time
    pub fn update_activity_time(&mut self) {
        self.last_activity_time = Utc::now();
    }
    
    /// Check if the session has expired
    pub fn is_expired(&self, max_age: Duration) -> bool {
        Utc::now() - self.last_activity_time > max_age
    }
}

/// Store for managing multiple sessions
pub struct SessionStore {
    /// Sessions indexed by session ID
    sessions: HashMap<String, Session>,
    /// Mapping from conversation ID to session ID
    conversation_to_session: HashMap<String, String>,
    /// Mapping from session ID to conversation ID
    session_to_conversation: HashMap<String, String>,
}

impl SessionStore {
    /// Create a new session store
    pub fn new() -> Self {
        SessionStore {
            sessions: HashMap::new(),
            conversation_to_session: HashMap::new(),
            session_to_conversation: HashMap::new(),
        }
    }

    /// Get the session ID for a given conversation ID
    pub fn get_session_id_by_conversation(&self, conversation_id: &str) -> Option<String> {
        self.conversation_to_session.get(conversation_id).cloned()
    }

    /// Add a session to the store
    pub fn add_session(&mut self, session: Session) -> String {
        let session_id = session.session_id.clone();
        
        if let Some(conversation_id) = &session.conversation_id {
            self.set_conversation_mapping(conversation_id.clone(), session_id.clone());
        }
        
        self.sessions.insert(session_id.clone(), session);
        session_id
    }
    
    /// Get a session by session ID
    pub fn get_session(&self, session_id: &str) -> Option<&Session> {
        self.sessions.get(session_id)
    }
    
    /// Get a mutable reference to a session by session ID
    pub fn get_session_mut(&mut self, session_id: &str) -> Option<&mut Session> {
        if let Some(session) = self.sessions.get_mut(session_id) {
            session.update_activity_time();
            Some(session)
        } else {
            None
        }
    }
    
    /// Get a session by conversation ID
    pub fn get_session_by_conversation(&self, conversation_id: &str) -> Option<&Session> {
        self.conversation_to_session
            .get(conversation_id)
            .and_then(|session_id| self.sessions.get(session_id))
    }
    
    /// Get a mutable reference to a session by conversation ID
    pub fn get_session_by_conversation_mut(&mut self, conversation_id: &str) -> Option<&mut Session> {
        let session_id = match self.conversation_to_session.get(conversation_id) {
            Some(id) => id.clone(),
            None => return None,
        };
        
        if let Some(session) = self.sessions.get_mut(&session_id) {
            session.update_activity_time();
            Some(session)
        } else {
            None
        }
    }
    
    /// Remove a session from the store
    pub fn remove_session(&mut self, session_id: &str) -> Option<Session> {
        if let Some(conversation_id) = self.session_to_conversation.remove(session_id) {
            self.conversation_to_session.remove(&conversation_id);
        }
        
        self.sessions.remove(session_id)
    }
    
    /// Set mapping between conversation ID and session ID
    pub fn set_conversation_mapping(&mut self, conversation_id: String, session_id: String) {
        self.conversation_to_session.insert(conversation_id.clone(), session_id.clone());
        self.session_to_conversation.insert(session_id, conversation_id);
    }
    
    /// Clean up expired sessions
    pub fn cleanup_expired_sessions(&mut self, max_age: Duration) {
        let expired_sessions: Vec<String> = self.sessions
            .iter()
            .filter(|(_, session)| session.is_expired(max_age))
            .map(|(id, _)| id.clone())
            .collect();
        
        for session_id in expired_sessions {
            info!("Removing expired session: {}", session_id);
            self.remove_session(&session_id);
        }
    }
}

/// Start a periodic session cleanup task
pub fn start_session_cleanup_task(
    session_store: Arc<tokio::sync::RwLock<SessionStore>>,
    interval_minutes: u64,
    max_age_minutes: i64
) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(interval_minutes * 60));

        loop {
            interval.tick().await;
            let max_age = Duration::minutes(max_age_minutes);

            // Get write lock without pattern matching
            let mut store = session_store.write().await;
            store.cleanup_expired_sessions(max_age);
            debug!("Session cleanup completed");
        }
    });
}
