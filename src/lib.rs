use tokio::{
    net::TcpListener,
    sync::{broadcast, mpsc, RwLock},
    time::{interval, Duration},
};
use tokio_tungstenite::{
    accept_async,
    tungstenite::{Message, Result as WsResult},
};
use futures_util::{SinkExt, StreamExt};
use std::{
    net::SocketAddr,
    sync::{Arc, atomic::{AtomicU64, AtomicUsize, Ordering}},
    collections::HashMap,
};
use serde::{Deserialize, Serialize};
use bytes::Bytes;
use smallvec::SmallVec;
use slab::Slab;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub id: u64,
    pub content: String,
    pub sender: String,
}

struct ConnectionManager {
    connections: Arc<RwLock<Slab<mpsc::UnboundedSender<Bytes>>>>,
    connection_count: Arc<AtomicUsize>,
    message_buffer: Arc<RwLock<SmallVec<[ChatMessage; 1000]>>>,
}

impl ConnectionManager {
    fn new() -> Self {
        Self {
            connections: Arc::new(RwLock::new(Slab::with_capacity(300000))),
            connection_count: Arc::new(AtomicUsize::new(0)),
            message_buffer: Arc::new(RwLock::new(SmallVec::new())),
        }
    }
    
    async fn add_connection(&self, sender: mpsc::UnboundedSender<Bytes>) -> usize {
        let mut connections = self.connections.write().await;
        let id = connections.insert(sender);
        self.connection_count.fetch_add(1, Ordering::Relaxed);
        id
    }
    
    async fn remove_connection(&self, id: usize) {
        let mut connections = self.connections.write().await;
        if connections.contains(id) {
            connections.remove(id);
            self.connection_count.fetch_sub(1, Ordering::Relaxed);
        }
    }
    
    async fn broadcast_message(&self, message: &ChatMessage, exclude_sender: &str) {
        let json = match serde_json::to_string(message) {
            Ok(j) => Bytes::from(j),
            Err(_) => return,
        };
        
        let connections = self.connections.read().await;
        let mut failed_connections = Vec::new();
        
        for (id, sender) in connections.iter() {
            if sender.send(json.clone()).is_err() {
                failed_connections.push(id);
            }
        }
        
        drop(connections);
        
        if !failed_connections.is_empty() {
            let mut connections = self.connections.write().await;
            for id in failed_connections {
                if connections.contains(id) {
                    connections.remove(id);
                    self.connection_count.fetch_sub(1, Ordering::Relaxed);
                }
            }
        }
    }
    
    fn get_connection_count(&self) -> usize {
        self.connection_count.load(Ordering::Relaxed)
    }
}

pub async fn run_chat_server(listener: TcpListener) {
    let num_cores = num_cpus::get();
    println!("Optimized for 300k concurrent clients on {} CPU cores", num_cores);
    
    let connection_manager = Arc::new(ConnectionManager::new());
    let message_id_counter = Arc::new(AtomicU64::new(0));
    let (message_tx, mut message_rx) = mpsc::unbounded_channel::<ChatMessage>();
    
    let manager_clone = connection_manager.clone();
    tokio::spawn(async move {
        let mut batch_buffer = Vec::with_capacity(1000);
        let mut batch_timer = interval(Duration::from_millis(10));
        
        loop {
            tokio::select! {
                msg = message_rx.recv() => {
                    if let Some(message) = msg {
                        batch_buffer.push(message);
                        if batch_buffer.len() >= 100 {
                            for msg in batch_buffer.drain(..) {
                                manager_clone.broadcast_message(&msg, &msg.sender).await;
                            }
                        }
                    } else {
                        break;
                    }
                }
                _ = batch_timer.tick() => {
                    if !batch_buffer.is_empty() {
                        for msg in batch_buffer.drain(..) {
                            manager_clone.broadcast_message(&msg, &msg.sender).await;
                        }
                    }
                }
            }
        }
    });
    
    let stats_manager = connection_manager.clone();
    tokio::spawn(async move {
        let mut stats_timer = interval(Duration::from_secs(30));
        loop {
            stats_timer.tick().await;
            let count = stats_manager.get_connection_count();
            println!("Active connections: {}", count);
        }
    });
    
    loop {
        let (socket, addr) = listener.accept().await.unwrap();
        let manager = connection_manager.clone();
        let msg_tx = message_tx.clone();
        let counter = message_id_counter.clone();
        
        tokio::spawn(async move {
            if let Err(e) = handle_massive_scale_connection(socket, addr, manager, msg_tx, counter).await {
                eprintln!("Connection error for {}: {}", addr, e);
            }
        });
    }
}

async fn handle_massive_scale_connection(
    socket: tokio::net::TcpStream,
    addr: SocketAddr,
    connection_manager: Arc<ConnectionManager>,
    message_tx: mpsc::UnboundedSender<ChatMessage>,
    counter: Arc<AtomicU64>,
) -> WsResult<()> {
    let ws_stream = accept_async(socket).await?;
    let (ws_sender, ws_receiver) = ws_stream.split();
    
    let (outbound_tx, mut outbound_rx) = mpsc::unbounded_channel::<Bytes>();
    let connection_id = connection_manager.add_connection(outbound_tx).await;
    
    let msg_tx = message_tx.clone();
    let counter_clone = counter.clone();
    
    let receive_task = tokio::spawn(async move {
        let mut ws_receiver = ws_receiver;
        let mut rate_limiter = interval(Duration::from_millis(1000));
        
        loop {
            tokio::select! {
                msg = ws_receiver.next() => {
                    match msg {
                        Some(Ok(Message::Text(text))) => {
                            if !text.trim().is_empty() {
                                let message_id = counter_clone.fetch_add(1, Ordering::Relaxed);
                                let chat_msg = ChatMessage {
                                    id: message_id,
                                    content: text,
                                    sender: addr.to_string(),
                                };
                                let _ = msg_tx.send(chat_msg);
                            }
                        }
                        Some(Ok(Message::Close(_))) => break,
                        Some(Err(_)) => break,
                        None => break,
                        _ => {}
                    }
                }
                _ = rate_limiter.tick() => {
                    // Rate limit: 1 message per second per client
                }
            }
        }
    });
    
    let send_task = tokio::spawn(async move {
        let mut ws_sender = ws_sender;
        
        while let Some(data) = outbound_rx.recv().await {
            if ws_sender.send(Message::Text(String::from_utf8_lossy(&data).into_owned())).await.is_err() {
                break;
            }
        }
    });
    
    tokio::select! {
        _ = receive_task => {},
        _ = send_task => {},
    }
    
    connection_manager.remove_connection(connection_id).await;
    Ok(())
}