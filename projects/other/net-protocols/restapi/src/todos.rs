//! The todo API: plain functions from `(&Request, &SharedStore)` to
//! `Response`, wired to routes in [`routes`].

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::http::{Request, Response};
use crate::router::{Handler, Router};

/// `#[derive(Serialize, Deserialize)]` writes the JSON conversion for us:
/// `{"id":1,"title":"…","done":false}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Todo {
    pub id: u64,
    pub title: String,
    pub done: bool,
}

/// A `BTreeMap` rather than a `HashMap` so the list comes out ordered by id.
#[derive(Debug, Default)]
pub struct Store {
    last_id: u64,
    todos: BTreeMap<u64, Todo>,
}

/// Every worker thread holds a clone of the `Arc`; the `Mutex` makes sure
/// only one of them touches the store at a time.
pub type SharedStore = Arc<Mutex<Store>>;

/// POST body. A missing `title` is a deserialization error, which becomes 400.
#[derive(Deserialize)]
struct NewTodo {
    title: String,
}

/// PATCH body: every field optional, and unknown fields (typos) rejected.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TodoPatch {
    title: Option<String>,
    done: Option<bool>,
}

pub fn routes(store: SharedStore) -> Router {
    Router::new()
        .get("/todos", with(&store, list))
        .post("/todos", with(&store, create))
        .get("/todos/:id", with(&store, show))
        .patch("/todos/:id", with(&store, update))
        .delete("/todos/:id", with(&store, delete))
}

/// Turns `fn(&Request, &SharedStore)` into a `Handler`. The returned
/// closure owns its own clone of the `Arc` (`move`), so it can live as
/// long as the router.
fn with(store: &SharedStore, f: fn(&Request, &SharedStore) -> Response) -> impl Handler {
    let store = Arc::clone(store);
    move |req: &Request| f(req, &store)
}

/// `GET /todos`, optionally `?done=true` or `?done=false`.
pub fn list(req: &Request, store: &SharedStore) -> Response {
    let done = match req.query.get("done").map(String::as_str) {
        None => None,
        Some("true") => Some(true),
        Some("false") => Some(false),
        Some(_) => return Response::error(400, "done must be true or false"),
    };
    let store = store.lock().expect("store lock poisoned");
    let todos: Vec<&Todo> = store
        .todos
        .values()
        .filter(|t| done.is_none_or(|d| t.done == d))
        .collect();
    Response::json(200, &todos)
}

/// `POST /todos` with `{"title":"…"}`, answering 201 Created plus a
/// `Location` header that points at the new resource.
pub fn create(req: &Request, store: &SharedStore) -> Response {
    let new: NewTodo = match parse_json(&req.body) {
        Ok(new) => new,
        Err(resp) => return resp,
    };
    if new.title.trim().is_empty() {
        return Response::error(400, "title must not be empty");
    }
    let mut store = store.lock().expect("store lock poisoned");
    store.last_id += 1;
    let todo = Todo {
        id: store.last_id,
        title: new.title,
        done: false,
    };
    store.todos.insert(todo.id, todo.clone());
    Response::json(201, &todo).with_header("Location", &format!("/todos/{}", todo.id))
}

/// `GET /todos/:id`.
pub fn show(req: &Request, store: &SharedStore) -> Response {
    let store = store.lock().expect("store lock poisoned");
    match id(req).and_then(|id| store.todos.get(&id)) {
        Some(todo) => Response::json(200, todo),
        None => not_found(),
    }
}

/// `PATCH /todos/:id` with any of `{"title":"…","done":true}`.
pub fn update(req: &Request, store: &SharedStore) -> Response {
    let patch: TodoPatch = match parse_json(&req.body) {
        Ok(patch) => patch,
        Err(resp) => return resp,
    };
    if patch.title.as_ref().is_some_and(|t| t.trim().is_empty()) {
        return Response::error(400, "title must not be empty");
    }
    let mut store = store.lock().expect("store lock poisoned");
    let Some(todo) = id(req).and_then(|id| store.todos.get_mut(&id)) else {
        return not_found();
    };
    if let Some(title) = patch.title {
        todo.title = title;
    }
    if let Some(done) = patch.done {
        todo.done = done;
    }
    Response::json(200, todo)
}

/// `DELETE /todos/:id`: 204 No Content on success.
pub fn delete(req: &Request, store: &SharedStore) -> Response {
    let mut store = store.lock().expect("store lock poisoned");
    match id(req).and_then(|id| store.todos.remove(&id)) {
        Some(_) => Response::new(204),
        None => not_found(),
    }
}

/// `/todos/abc` can't name a todo, so a non-numeric id is simply not found.
fn id(req: &Request) -> Option<u64> {
    req.param("id")?.parse().ok()
}

fn not_found() -> Response {
    Response::error(404, "not found")
}

/// Generic over the target type: `T` is inferred from the `let` it's
/// assigned to. `DeserializeOwned` means "builds from JSON without
/// borrowing from it".
fn parse_json<T: DeserializeOwned>(body: &[u8]) -> Result<T, Response> {
    serde_json::from_slice(body).map_err(|e| Response::error(400, &format!("invalid JSON: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn req(method: &str, target: &str, id: Option<&str>, body: &str) -> Request {
        let mut req = Request::new(method, target);
        if let Some(id) = id {
            req.params.insert("id".into(), id.into());
        }
        req.body = body.as_bytes().to_vec();
        req
    }

    fn json_of(resp: &Response) -> Value {
        serde_json::from_slice(&resp.body).unwrap()
    }

    #[test]
    fn create_show_update_delete() {
        let store = SharedStore::default();
        let created = create(
            &req("POST", "/todos", None, r#"{"title":"write a router"}"#),
            &store,
        );
        assert_eq!(created.status, 201);
        assert_eq!(created.header("Location"), Some("/todos/1"));
        assert_eq!(
            json_of(&created),
            json!({ "id": 1, "title": "write a router", "done": false })
        );

        let patched = update(
            &req("PATCH", "/todos/1", Some("1"), r#"{"done":true}"#),
            &store,
        );
        assert_eq!(json_of(&patched)["done"], json!(true));
        assert_eq!(
            json_of(&show(&req("GET", "/todos/1", Some("1"), ""), &store))["done"],
            json!(true)
        );

        assert_eq!(
            delete(&req("DELETE", "/todos/1", Some("1"), ""), &store).status,
            204
        );
        let gone = show(&req("GET", "/todos/1", Some("1"), ""), &store);
        assert_eq!(
            (gone.status, json_of(&gone)),
            (404, json!({ "error": "not found" }))
        );
    }

    #[test]
    fn list_is_ordered_and_filterable() {
        let store = SharedStore::default();
        for title in ["a", "b", "c"] {
            create(
                &req(
                    "POST",
                    "/todos",
                    None,
                    &json!({ "title": title }).to_string(),
                ),
                &store,
            );
        }
        update(
            &req("PATCH", "/todos/2", Some("2"), r#"{"done":true}"#),
            &store,
        );
        let ids = |target: &str| -> Value {
            let all = json_of(&list(&req("GET", target, None, ""), &store));
            all.as_array()
                .unwrap()
                .iter()
                .map(|t| t["id"].clone())
                .collect()
        };
        assert_eq!(ids("/todos"), json!([1, 2, 3]));
        assert_eq!(ids("/todos?done=true"), json!([2]));
        assert_eq!(ids("/todos?done=false"), json!([1, 3]));
        assert_eq!(
            list(&req("GET", "/todos?done=maybe", None, ""), &store).status,
            400
        );
    }

    #[test]
    fn bad_input_is_400_and_unknown_ids_404() {
        let store = SharedStore::default();
        for body in ["", "not json", "{}", r#"{"title":42}"#, r#"{"title":"  "}"#] {
            let resp = create(&req("POST", "/todos", None, body), &store);
            assert_eq!(resp.status, 400, "{body:?}");
            assert!(json_of(&resp)["error"].is_string(), "{body:?}");
        }
        create(&req("POST", "/todos", None, r#"{"title":"x"}"#), &store);
        let typo = update(
            &req("PATCH", "/todos/1", Some("1"), r#"{"dun":true}"#),
            &store,
        );
        assert_eq!(typo.status, 400);
        for id in ["2", "abc", "-1"] {
            assert_eq!(
                show(&req("GET", "/", Some(id), ""), &store).status,
                404,
                "{id}"
            );
            assert_eq!(
                update(&req("PATCH", "/", Some(id), "{}"), &store).status,
                404,
                "{id}"
            );
            assert_eq!(
                delete(&req("DELETE", "/", Some(id), ""), &store).status,
                404,
                "{id}"
            );
        }
    }
}
