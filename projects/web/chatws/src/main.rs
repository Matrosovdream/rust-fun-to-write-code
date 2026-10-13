use std::net::SocketAddr;
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message as WsMessage;
use futures_util::stream::StreamExt;
use futures_util::sink::SinkExt;
use chatws::{ChatServer, Message, generate_user_id};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr: SocketAddr = "127.0.0.1:8888".parse()?;
    let listener = TcpListener::bind(&addr).await?;
    let server = ChatServer::new();

    println!("Chat server listening on {}", addr);
    println!("Connect: wscat -c ws://127.0.0.1:8888/room/general");
    println!("Then send: {{\"text\":\"hello\"}}");

    loop {
        let (stream, _addr) = listener.accept().await?;
        let srv = server.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_websocket(stream, srv).await {
                eprintln!("Error: {}", e);
            }
        });
    }
}

async fn handle_websocket(
    stream: TcpStream,
    server: ChatServer,
) -> Result<(), Box<dyn std::error::Error>> {
    let ws_stream = tokio_tungstenite::accept_async(stream).await?;
    let user_id = generate_user_id();

    let (mut ws_tx, mut ws_rx) = ws_stream.split();
    let mut room_name = String::from("general");
    let tx = server.get_or_create_room(room_name.clone()).await;
    let mut broadcast_rx = tx.subscribe();

    println!("[{}] joined {}", user_id, room_name);

    loop {
        tokio::select! {
            msg = ws_rx.next() => {
                match msg {
                    Some(Ok(WsMessage::Text(text))) => {
                        if let Ok(chat_msg) = serde_json::from_str::<serde_json::Value>(&text) {
                            if let Some(room) = chat_msg.get("room").and_then(|v| v.as_str()) {
                                room_name = room.to_string();
                                broadcast_rx = server.get_or_create_room(room_name.clone()).await.subscribe();
                                println!("[{}] switched to {}", user_id, room_name);
                            } else if let Some(text) = chat_msg.get("text").and_then(|v| v.as_str()) {
                                let msg = Message {
                                    room: room_name.clone(),
                                    user: user_id.clone(),
                                    text: text.to_string(),
                                };
                                let tx = server.get_or_create_room(room_name.clone()).await;
                                let _ = tx.send(msg);
                            }
                        }
                    }
                    Some(Ok(WsMessage::Close(_))) => {
                        println!("[{}] left", user_id);
                        break;
                    }
                    Some(Err(_)) => break,
                    None => break,
                    _ => {}
                }
            }
            result = broadcast_rx.recv() => {
                if let Ok(msg) = result {
                    let json = serde_json::to_string(&msg).unwrap_or_default();
                    if let Err(_) = ws_tx.send(WsMessage::Text(json)).await {
                        break;
                    }
                } else {
                    break;
                }
            }
        }
    }

    Ok(())
}
