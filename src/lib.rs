use tokio::{
    net::TcpListener,
    sync::{broadcast, RwLock},
    time::{Duration, Instant},
};
use tokio_tungstenite::{
    accept_async,
    tungstenite::{Message, Result as WsResult},
};
use futures_util::{SinkExt, StreamExt};
use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{Arc, atomic::{AtomicU64, AtomicUsize, Ordering}},
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub id: u64,
    pub content: String,
    pub sender: String,
    #[serde(default)]
    pub timestamp: u64,
}

#[derive(Debug)]
struct ClientMetrics {
    messages_sent: AtomicUsize,
    messages_received: AtomicUsize,
    last_activity: RwLock<Instant>,
}

pub async fn run_chat_server(listener: TcpListener) {
    let num_cores = num_cpus::get();
    println!("Detected {} CPU cores, optimizing for high fan-out performance", num_cores);
    
    // Increased channel capacity for better fan-out buffering
    let (broadcast_tx, _broadcast_rx) = broadcast::channel::<ChatMessage>(num_cores * 50000);
    let message_id_counter = Arc::new(AtomicU64::new(0));
    let client_metrics = Arc::new(RwLock::new(HashMap::<String, ClientMetrics>::new()));
    let active_connections = Arc::new(AtomicUsize::new(0));
    
    // Spawn metrics reporting task
    let _metrics_clone = client_metrics.clone();
    let connections_clone = active_connections.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(10));
        loop {
            interval.tick().await;
            let conn_count = connections_clone.load(Ordering::Relaxed);
            if conn_count > 0 {
                println!("Active connections: {}, Fan-out multiplier: {}x", conn_count, conn_count.saturating_sub(1));
            }
        }
    });
    
    loop {
        let (socket, addr) = listener.accept().await.unwrap();
        let tx = broadcast_tx.clone();
        let counter = message_id_counter.clone();
        let metrics = client_metrics.clone();
        let connections = active_connections.clone();
        
        tokio::spawn(async move {
            connections.fetch_add(1, Ordering::Relaxed);
            if let Err(e) = handle_connection_fanout_optimized(socket, addr, tx, counter, metrics).await {
                eprintln!("Connection error for {}: {}", addr, e);
            }
            connections.fetch_sub(1, Ordering::Relaxed);
        });
    }
}

async fn handle_connection_fanout_optimized(
    socket: tokio::net::TcpStream,
    addr: SocketAddr,
    broadcast_tx: broadcast::Sender<ChatMessage>,
    counter: Arc<AtomicU64>,
    client_metrics: Arc<RwLock<HashMap<String, ClientMetrics>>>,
) -> WsResult<()> {
    let ws_stream = accept_async(socket).await?;
    let (ws_sender, ws_receiver) = ws_stream.split();
    
    let client_id = addr.to_string();
    
    // Initialize client metrics
    {
        let mut metrics = client_metrics.write().await;
        metrics.insert(client_id.clone(), ClientMetrics {
            messages_sent: AtomicUsize::new(0),
            messages_received: AtomicUsize::new(0),
            last_activity: RwLock::new(Instant::now()),
        });
    }
    
    let tx = broadcast_tx.clone();
    let counter_clone = counter.clone();
    let metrics_clone = client_metrics.clone();
    let client_id_clone = client_id.clone();
    
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
                            sender: client_id_clone.clone(),
                            timestamp: std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_millis() as u64,
                        };
                        
                        // Update metrics
                        {
                            let metrics = metrics_clone.read().await;
                            if let Some(client_metrics) = metrics.get(&client_id_clone) {
                                client_metrics.messages_sent.fetch_add(1, Ordering::Relaxed);
                                *client_metrics.last_activity.write().await = Instant::now();
                            }
                        }
                        
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
    let metrics_clone2 = client_metrics.clone();
    let client_id_clone2 = client_id.clone();
    
    let send_task = tokio::spawn(async move {
        let mut ws_sender = ws_sender;
        // Optimized for fan-out: larger buffer capacity, smarter batching
        let mut message_buffer = Vec::with_capacity(500);
        let mut flush_timer = tokio::time::interval(Duration::from_micros(500)); // More aggressive flushing
        let mut last_flush = Instant::now();
        
        loop {
            tokio::select! {
                msg_result = rx.recv() => {
                    match msg_result {
                        Ok(message) => {
                            if message.sender != client_id_clone2 {
                                message_buffer.push(message);
                                
                                // Dynamic batching based on message rate and age
                                let should_flush = message_buffer.len() >= 200 || 
                                    (message_buffer.len() >= 50 && last_flush.elapsed() > Duration::from_millis(2)) ||
                                    (message_buffer.len() >= 10 && last_flush.elapsed() > Duration::from_millis(10));
                                
                                if should_flush {
                                    if let Ok(json) = serde_json::to_string(&message_buffer) {
                                        if ws_sender.send(Message::Text(json)).await.is_ok() {
                                            // Update metrics
                                            {
                                                let metrics = metrics_clone2.read().await;
                                                if let Some(client_metrics) = metrics.get(&client_id_clone2) {
                                                    client_metrics.messages_received.fetch_add(message_buffer.len(), Ordering::Relaxed);
                                                }
                                            }
                                        } else {
                                            break;
                                        }
                                    }
                                    message_buffer.clear();
                                    last_flush = Instant::now();
                                }
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(count)) => {
                            eprintln!("Client {} lagged by {} messages - fan-out overload", client_id_clone2, count);
                            continue;
                        }
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
                _ = flush_timer.tick() => {
                    if !message_buffer.is_empty() {
                        if let Ok(json) = serde_json::to_string(&message_buffer) {
                            if ws_sender.send(Message::Text(json)).await.is_ok() {
                                // Update metrics
                                {
                                    let metrics = metrics_clone2.read().await;
                                    if let Some(client_metrics) = metrics.get(&client_id_clone2) {
                                        client_metrics.messages_received.fetch_add(message_buffer.len(), Ordering::Relaxed);
                                    }
                                }
                            } else {
                                break;
                            }
                        }
                        message_buffer.clear();
                        last_flush = Instant::now();
                    }
                }
            }
        }
    });
    
    tokio::select! {
        _ = receive_task => {},
        _ = send_task => {},
    }
    
    // Cleanup client metrics
    {
        let mut metrics = client_metrics.write().await;
        metrics.remove(&client_id);
    }
    
    Ok(())
}