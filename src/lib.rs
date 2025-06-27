use tokio::{
    net::TcpListener,
    runtime::Handle,
};
use tokio_tungstenite::{
    accept_async,
    tungstenite::{Message, Result as WsResult},
};
use futures_util::{SinkExt, StreamExt};
use std::{
    net::SocketAddr,
    sync::{Arc, atomic::{AtomicU64, Ordering}},
};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub id: u64,
    pub content: String,
    pub sender: String,
}

pub async fn run_chat_server(listener: TcpListener) {
    let num_cores = num_cpus::get();
    println!("Detected {} CPU cores, optimizing for Ryzen 7 5700X3D", num_cores);
    
    let (broadcast_tx, _broadcast_rx) = broadcast::channel::<ChatMessage>(num_cores * 10000);
    let message_id_counter = Arc::new(AtomicU64::new(0));
    
    loop {
        let (socket, addr) = listener.accept().await.unwrap();
        let tx = broadcast_tx.clone();
        let counter = message_id_counter.clone();
        
        tokio::spawn(async move {
            if let Err(e) = handle_connection_ryzen_optimized(socket, addr, tx, counter).await {
                eprintln!("Connection error for {}: {}", addr, e);
            }
        });
    }
}

async fn handle_connection_ryzen_optimized(
    socket: tokio::net::TcpStream,
    addr: SocketAddr,
    broadcast_tx: broadcast::Sender<ChatMessage>,
    counter: Arc<AtomicU64>,
) -> WsResult<()> {
    let ws_stream = accept_async(socket).await?;
    let (ws_sender, ws_receiver) = ws_stream.split();
    
    let tx = broadcast_tx.clone();
    let counter_clone = counter.clone();
    
    let receive_task = tokio::spawn(async move {
        let mut ws_receiver = ws_receiver;
        
        while let Some(msg) = ws_receiver.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    if !text.trim().is_empty() {
                        let message_id = counter_clone.fetch_add(1, Ordering::Relaxed);
                        let chat_msg = ChatMessage {
                            id: message_id,
                            content: text,
                            sender: addr.to_string(),
                        };
                        let _ = tx.send(chat_msg);
                    }
                }
                Ok(Message::Close(_)) => break,
                Err(_) => break,
                _ => {}
            }
        }
    });
    
    let mut rx = broadcast_tx.subscribe();
    let send_task = tokio::spawn(async move {
        let mut ws_sender = ws_sender;
        let mut message_buffer = Vec::with_capacity(200);
        let mut flush_timer = tokio::time::interval(tokio::time::Duration::from_micros(800));
        
        loop {
            tokio::select! {
                msg_result = rx.recv() => {
                    match msg_result {
                        Ok(message) => {
                            if message.sender != addr.to_string() {
                                message_buffer.push(message);
                                if message_buffer.len() >= 100 {
                                    if let Ok(json) = serde_json::to_string(&message_buffer) {
                                        if ws_sender.send(Message::Text(json)).await.is_err() {
                                            break;
                                        }
                                    }
                                    message_buffer.clear();
                                }
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
                _ = flush_timer.tick() => {
                    if !message_buffer.is_empty() {
                        if let Ok(json) = serde_json::to_string(&message_buffer) {
                            if ws_sender.send(Message::Text(json)).await.is_err() {
                                break;
                            }
                        }
                        message_buffer.clear();
                    }
                }
            }
        }
    });
    
    tokio::select! {
        _ = receive_task => {},
        _ = send_task => {},
    }
    
    Ok(())
}