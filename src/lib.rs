use tokio::{
    net::TcpListener,
    sync::broadcast,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub id: u64,
    pub content: String,
    pub sender: String,
}

pub async fn run_chat_server(listener: TcpListener) {
    let (tx, _rx) = broadcast::channel(100000);
    let message_id_counter = Arc::new(AtomicU64::new(0));

    loop {
        let (socket, addr) = listener.accept().await.unwrap();
        let tx = tx.clone();
        let counter = message_id_counter.clone();

        tokio::spawn(async move {
            if let Err(e) = handle_connection(socket, addr, tx, counter).await {
                eprintln!("Connection error for {}: {}", addr, e);
            }
        });
    }
}

async fn handle_connection(
    socket: tokio::net::TcpStream,
    addr: SocketAddr,
    tx: broadcast::Sender<ChatMessage>,
    counter: Arc<AtomicU64>,
) -> WsResult<()> {
    let ws_stream = accept_async(socket).await?;
    let (ws_sender, ws_receiver) = ws_stream.split();
    
    let tx_clone = tx.clone();
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
                        let _ = tx_clone.send(chat_msg);
                    }
                }
                Ok(Message::Close(_)) => break,
                Err(_) => break,
                _ => {}
            }
        }
    });
    
    let mut rx = tx.subscribe();
    let send_task = tokio::spawn(async move {
        let mut ws_sender = ws_sender;
        let mut message_batch = Vec::with_capacity(100);
        let mut flush_interval = tokio::time::interval(tokio::time::Duration::from_millis(1));
        
        loop {
            tokio::select! {
                msg_result = rx.recv() => {
                    match msg_result {
                        Ok(message) => {
                            if message.sender != addr.to_string() {
                                message_batch.push(message);
                                if message_batch.len() >= 100 {
                                    if let Ok(batch_json) = serde_json::to_string(&message_batch) {
                                        if ws_sender.send(Message::Text(batch_json)).await.is_err() {
                                            break;
                                        }
                                    }
                                    message_batch.clear();
                                }
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
                _ = flush_interval.tick() => {
                    if !message_batch.is_empty() {
                        if let Ok(batch_json) = serde_json::to_string(&message_batch) {
                            if ws_sender.send(Message::Text(batch_json)).await.is_err() {
                                break;
                            }
                        }
                        message_batch.clear();
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