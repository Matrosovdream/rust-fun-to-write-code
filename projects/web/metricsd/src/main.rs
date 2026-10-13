use std::net::SocketAddr;
use tokio::net::TcpListener;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use metricsd::MetricsCollector;
use serde_json::json;
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr: SocketAddr = "127.0.0.1:9999".parse()?;
    let listener = TcpListener::bind(&addr).await?;
    let collector = MetricsCollector::new();

    println!("Metrics server listening on {}", addr);
    println!("GET  /metrics       — list current metrics");
    println!("GET  /stream        — server-sent events stream");
    println!("POST /record        — record metric (body: {{\"name\":\"...\",\"value\":...}})");

    let collector_sampler = collector.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(2));
        let mut counter = 0;
        loop {
            interval.tick().await;
            counter += 1;
            collector_sampler.record("request_count".to_string(), counter as f64).await;
            collector_sampler.record("cpu_usage".to_string(), 50.0 + (counter % 30) as f64).await;
            collector_sampler.record("memory_mb".to_string(), 512.0 + (counter * 10 % 200) as f64).await;
        }
    });

    loop {
        let (stream, _addr) = listener.accept().await?;
        let collector = collector.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_connection(stream, collector).await {
                eprintln!("Error: {}", e);
            }
        });
    }
}

async fn handle_connection(
    mut stream: tokio::net::TcpStream,
    collector: MetricsCollector,
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

    match (method, path) {
        ("GET", "/metrics") => {
            let metrics = collector.get_all().await;
            let json_data: Vec<_> = metrics
                .iter()
                .map(|(k, v)| json!({"name": k, "value": v}))
                .collect();
            let body = serde_json::to_string(&json_data)?;
            let response = format_json_response(&body, 200);
            stream.write_all(response.as_bytes()).await?;
        }
        ("GET", "/stream") => {
            let response_header = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\n\r\n";
            stream.write_all(response_header.as_bytes()).await?;

            let mut rx = collector.subscribe();
            loop {
                match tokio::time::timeout(Duration::from_secs(30), rx.recv()).await {
                    Ok(Ok(metric)) => {
                        let json = serde_json::to_string(&metric).unwrap_or_default();
                        let event = format!("data: {}\n\n", json);
                        if stream.write_all(event.as_bytes()).await.is_err() {
                            break;
                        }
                    }
                    Ok(Err(_)) => break,
                    Err(_) => {
                        let keep_alive = ": keep-alive\n\n";
                        if stream.write_all(keep_alive.as_bytes()).await.is_err() {
                            break;
                        }
                    }
                }
            }
        }
        ("POST", "/record") => {
            if let Some(body_start) = request_str.find("\r\n\r\n") {
                let body_str = &request_str[body_start + 4..];
                if let Ok(data) = serde_json::from_str::<serde_json::Value>(body_str) {
                    if let (Some(name), Some(value)) = (
                        data.get("name").and_then(|v| v.as_str()),
                        data.get("value").and_then(|v| v.as_f64()),
                    ) {
                        collector.record(name.to_string(), value).await;
                        let response = format_json_response(r#"{"ok": true}"#, 201);
                        stream.write_all(response.as_bytes()).await?;
                    } else {
                        let response = format_error("Invalid fields", 400);
                        stream.write_all(response.as_bytes()).await?;
                    }
                } else {
                    let response = format_error("Invalid JSON", 400);
                    stream.write_all(response.as_bytes()).await?;
                }
            } else {
                let response = format_error("No body", 400);
                stream.write_all(response.as_bytes()).await?;
            }
        }
        _ => {
            let response = format_error("Not found", 404);
            stream.write_all(response.as_bytes()).await?;
        }
    }

    Ok(())
}

fn format_json_response(body: &str, status: u16) -> String {
    let status_text = match status {
        200 => "OK",
        201 => "Created",
        400 => "Bad Request",
        404 => "Not Found",
        _ => "Unknown",
    };

    format!(
        "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
        status, status_text, body.len(), body
    )
}

fn format_error(message: &str, status: u16) -> String {
    let body = format!(r#"{{"error": "{}"}}"#, message);
    format_json_response(&body, status)
}
