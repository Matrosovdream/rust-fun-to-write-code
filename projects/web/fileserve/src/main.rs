use std::net::SocketAddr;
use tokio::net::TcpListener;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use fileserve::FileServer;
use std::env;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = env::current_dir()?;
    let addr: SocketAddr = "127.0.0.1:3333".parse()?;
    let listener = TcpListener::bind(&addr).await?;
    let server = FileServer::new(root);

    println!("File server listening on {}", addr);
    println!("GET  /             — list current directory");
    println!("GET  /path/to/file — download file");
    println!("PUT  /path/to/file — upload file (body = file content)");

    loop {
        let (stream, _addr) = listener.accept().await?;
        let server = server.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_connection(stream, server).await {
                eprintln!("Error: {}", e);
            }
        });
    }
}

async fn handle_connection(
    mut stream: tokio::net::TcpStream,
    server: fileserve::FileServer,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut buf = vec![0; 8192];
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
    let clean_path = path.trim_start_matches('/');

    let response = match method {
        "GET" => {
            if clean_path.is_empty() {
                match server.list_dir(".").await {
                    Ok(entries) => {
                        let html = format_listing(entries);
                        format_response(&html, 200, "text/html")
                    }
                    Err(_) => format_error("Not found", 404),
                }
            } else {
                match server.read_file(clean_path).await {
                    Ok(data) => {
                        let content_type = guess_content_type(clean_path);
                        format_binary_response(&data, 200, content_type)
                    }
                    Err(_) => format_error("Not found", 404),
                }
            }
        }
        "PUT" => {
            if let Some(body_start) = request_str.find("\r\n\r\n") {
                let body_bytes = buf[body_start + 4..].to_vec();
                match server.write_file(clean_path, &body_bytes).await {
                    Ok(_) => format_response("{\"ok\": true}", 201, "application/json"),
                    Err(_) => format_error("Failed to write", 500),
                }
            } else {
                format_error("No body", 400)
            }
        }
        _ => format_error("Method not allowed", 405),
    };

    stream.write_all(response.as_bytes()).await?;
    Ok(())
}

fn format_response(body: &str, status: u16, content_type: &str) -> String {
    let status_text = match status {
        200 => "OK",
        201 => "Created",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        500 => "Internal Server Error",
        _ => "Unknown",
    };

    format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\n\r\n{}",
        status, status_text, content_type, body.len(), body
    )
}

fn format_binary_response(body: &[u8], status: u16, content_type: &str) -> String {
    let status_text = match status {
        200 => "OK",
        _ => "Unknown",
    };

    let header = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\n\r\n",
        status, status_text, content_type, body.len()
    );

    let mut response = header.into_bytes();
    response.extend_from_slice(body);

    String::from_utf8(response).unwrap_or_default()
}

fn format_error(message: &str, status: u16) -> String {
    let body = format!(r#"{{"error": "{}"}}"#, message);
    format_response(&body, status, "application/json")
}

fn format_listing(entries: Vec<fileserve::DirEntry>) -> String {
    let mut html = String::from("<html><body><h1>Directory listing</h1><ul>");
    for entry in entries {
        let icon = if entry.is_dir { "📁" } else { "📄" };
        let size_str = if entry.is_dir {
            String::new()
        } else {
            format!(" ({}B)", entry.size)
        };
        html.push_str(&format!("<li>{} {}{}</li>", icon, entry.name, size_str));
    }
    html.push_str("</ul></body></html>");
    html
}

fn guess_content_type(path: &str) -> &'static str {
    match path.split('.').last() {
        Some("html") | Some("htm") => "text/html",
        Some("txt") => "text/plain",
        Some("json") => "application/json",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("pdf") => "application/pdf",
        _ => "application/octet-stream",
    }
}
