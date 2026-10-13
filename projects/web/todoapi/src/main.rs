use std::net::SocketAddr;
use tokio::net::TcpListener;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use serde_json::json;
use todoapi::{Store, CreateTodoReq, UpdateTodoReq};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr: SocketAddr = "127.0.0.1:8080".parse()?;
    let listener = TcpListener::bind(&addr).await?;
    let store = Store::new();

    println!("Todo API listening on {}", addr);
    println!("GET  /todos           — list all todos");
    println!("POST /todos           — create todo (body: {{\"title\":\"...\"}})");
    println!("GET  /todos/:id       — get single todo");
    println!("PATCH /todos/:id      — update todo (body: {{\"title\":\"...\",\"done\":true}})");
    println!("DELETE /todos/:id     — delete todo");

    loop {
        let (stream, addr) = listener.accept().await?;
        let store = store.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_connection(stream, addr, store).await {
                eprintln!("Error: {}", e);
            }
        });
    }
}

async fn handle_connection(
    mut stream: tokio::net::TcpStream,
    _addr: std::net::SocketAddr,
    store: Store,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut buf = vec![0; 4096];
    let n = stream.read(&mut buf).await?;
    buf.truncate(n);

    let request_str = String::from_utf8_lossy(&buf);
    let lines: Vec<&str> = request_str.lines().collect();
    if lines.is_empty() {
        return Ok(());
    }

    let request_line = lines[0];
    let parts: Vec<&str> = request_line.split_whitespace().collect();
    if parts.len() < 2 {
        return Ok(());
    }

    let method = parts[0];
    let path = parts[1];

    let response = match (method, path) {
        ("GET", "/todos") => {
            let todos = store.list().await;
            let body = serde_json::to_string(&todos)?;
            format_json_response(&body, 200)
        }
        ("POST", "/todos") => {
            if let Some(body_start) = request_str.find("\r\n\r\n") {
                let body_str = &request_str[body_start + 4..];
                if let Ok(req) = serde_json::from_str::<CreateTodoReq>(body_str) {
                    let todo = store.create(req).await;
                    let response_body = serde_json::to_string(&todo)?;
                    format!("HTTP/1.1 201 Created\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                        response_body.len(), response_body)
                } else {
                    format_error("Invalid request body", 400)
                }
            } else {
                format_error("No body", 400)
            }
        }
        (method, path) if path.starts_with("/todos/") => {
            let id_str = path.strip_prefix("/todos/").unwrap();
            if let Ok(id) = id_str.parse::<u64>() {
                match method {
                    "GET" => {
                        if let Some(todo) = store.get(id).await {
                            let body = serde_json::to_string(&todo)?;
                            format_json_response(&body, 200)
                        } else {
                            format_error("Not found", 404)
                        }
                    }
                    "PATCH" => {
                        if let Some(body_start) = request_str.find("\r\n\r\n") {
                            let body_str = &request_str[body_start + 4..];
                            if let Ok(req) = serde_json::from_str::<UpdateTodoReq>(body_str) {
                                if let Some(todo) = store.update(id, req).await {
                                    let response_body = serde_json::to_string(&todo)?;
                                    format_json_response(&response_body, 200)
                                } else {
                                    format_error("Not found", 404)
                                }
                            } else {
                                format_error("Invalid request body", 400)
                            }
                        } else {
                            format_error("No body", 400)
                        }
                    }
                    "DELETE" => {
                        if store.delete(id).await {
                            format_json_response("{}", 204)
                        } else {
                            format_error("Not found", 404)
                        }
                    }
                    _ => format_error("Method not allowed", 405),
                }
            } else {
                format_error("Invalid ID", 400)
            }
        }
        _ => format_error("Not found", 404),
    };

    stream.write_all(response.as_bytes()).await?;
    Ok(())
}

fn format_json_response(body: &str, status: u16) -> String {
    let status_text = match status {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "Unknown",
    };

    if status == 204 {
        format!(
            "HTTP/1.1 {} {}\r\nContent-Length: 0\r\n\r\n",
            status, status_text
        )
    } else {
        format!(
            "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            status,
            status_text,
            body.len(),
            body
        )
    }
}

fn format_error(message: &str, status: u16) -> String {
    let body = serde_json::to_string(&json!({"error": message})).unwrap();
    format_json_response(&body, status)
}
