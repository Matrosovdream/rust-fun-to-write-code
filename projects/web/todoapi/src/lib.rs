use std::sync::Arc;
use tokio::sync::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Todo {
    pub id: u64,
    pub title: String,
    pub done: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CreateTodoReq {
    pub title: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UpdateTodoReq {
    pub title: Option<String>,
    pub done: Option<bool>,
}

#[derive(Clone)]
pub struct Store {
    todos: Arc<Mutex<HashMap<u64, Todo>>>,
    next_id: Arc<Mutex<u64>>,
}

impl Store {
    pub fn new() -> Self {
        Self {
            todos: Arc::new(Mutex::new(HashMap::new())),
            next_id: Arc::new(Mutex::new(1)),
        }
    }

    pub async fn create(&self, req: CreateTodoReq) -> Todo {
        let mut next_id = self.next_id.lock().await;
        let id = *next_id;
        *next_id += 1;
        drop(next_id);

        let todo = Todo {
            id,
            title: req.title,
            done: false,
        };

        self.todos.lock().await.insert(id, todo.clone());
        todo
    }

    pub async fn list(&self) -> Vec<Todo> {
        self.todos
            .lock()
            .await
            .values()
            .cloned()
            .collect()
    }

    pub async fn get(&self, id: u64) -> Option<Todo> {
        self.todos.lock().await.get(&id).cloned()
    }

    pub async fn update(&self, id: u64, req: UpdateTodoReq) -> Option<Todo> {
        let mut todos = self.todos.lock().await;
        todos.get_mut(&id).map(|todo| {
            if let Some(title) = req.title {
                todo.title = title;
            }
            if let Some(done) = req.done {
                todo.done = done;
            }
            todo.clone()
        })
    }

    pub async fn delete(&self, id: u64) -> bool {
        self.todos.lock().await.remove(&id).is_some()
    }
}
