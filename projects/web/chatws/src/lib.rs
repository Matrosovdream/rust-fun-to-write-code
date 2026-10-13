use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, Mutex};
use uuid::Uuid;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub room: String,
    pub user: String,
    pub text: String,
}

#[derive(Clone)]
pub struct ChatServer {
    rooms: Arc<Mutex<HashMap<String, broadcast::Sender<Message>>>>,
}

impl ChatServer {
    pub fn new() -> Self {
        Self {
            rooms: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn get_or_create_room(&self, room_name: String) -> broadcast::Sender<Message> {
        let mut rooms = self.rooms.lock().await;
        rooms
            .entry(room_name)
            .or_insert_with(|| {
                let (tx, _) = broadcast::channel(100);
                tx
            })
            .clone()
    }

    pub async fn list_rooms(&self) -> Vec<String> {
        self.rooms
            .lock()
            .await
            .keys()
            .cloned()
            .collect()
    }
}

impl Default for ChatServer {
    fn default() -> Self {
        Self::new()
    }
}

pub fn generate_user_id() -> String {
    format!("user_{}", Uuid::new_v4().to_string().split('-').next().unwrap_or("unknown"))
}
