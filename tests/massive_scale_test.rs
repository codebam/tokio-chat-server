use chat_server::{run_chat_server, ChatMessage};
use tokio::{
    net::{TcpListener, TcpStream},
    time::{sleep, timeout, Duration, Instant},
    task::JoinSet,
};
use tokio_tungstenite::{
    client_async, 
    tungstenite::{Message, Result as WsResult},
};
use futures_util::{SinkExt, StreamExt};
use url::Url;
use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};

#[tokio::test]
async fn test_massive_concurrent_clients() -> WsResult<()> {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("ws://127.0.0.1:{}", addr.port());

    tokio::spawn(async move {
        run_chat_server(listener).await;
    });

    sleep(Duration::from_millis(500)).await;

    // Scale test: simulate realistic load  
    let client_count = 500; // Optimize for CI environment
    let test_duration = Duration::from_secs(15);
    
    let total_messages_sent = Arc::new(AtomicUsize::new(0));
    let total_messages_received = Arc::new(AtomicUsize::new(0));
    let active_connections = Arc::new(AtomicUsize::new(0));
    
    println!("Starting massive scale test with {} clients for {:?}", client_count, test_duration);
    let start_time = Instant::now();

    let mut join_set = JoinSet::new();

    for client_id in 0..client_count {
        let client_url = url.clone();
        let sent_counter = total_messages_sent.clone();
        let received_counter = total_messages_received.clone();
        let connection_counter = active_connections.clone();
        
        join_set.spawn(async move {
            let mut local_sent = 0;
            let mut local_received = 0;
            
            // Establish connection
            let stream = match TcpStream::connect(addr).await {
                Ok(s) => s,
                Err(_) => return (local_sent, local_received),
            };
            
            let url = match Url::parse(&client_url) {
                Ok(u) => u,
                Err(_) => return (local_sent, local_received),
            };
            
            let (ws_stream, _) = match client_async(url, stream).await {
                Ok(ws) => ws,
                Err(_) => return (local_sent, local_received),
            };
            
            connection_counter.fetch_add(1, Ordering::Relaxed);
            let (mut sender, mut receiver) = ws_stream.split();

            // Send 1 message per second (realistic rate)
            let send_task = tokio::spawn(async move {
                let mut message_timer = tokio::time::interval(Duration::from_millis(1000));
                let client_start = Instant::now();
                
                while client_start.elapsed() < test_duration {
                    message_timer.tick().await;
                    
                    let message = format!("Client-{}-msg-{}", client_id, local_sent);
                    match sender.send(Message::Text(message)).await {
                        Ok(_) => {
                            local_sent += 1;
                            sent_counter.fetch_add(1, Ordering::Relaxed);
                        }
                        Err(_) => break,
                    }
                }
                local_sent
            });

            // Receive messages from other clients
            let receive_task = tokio::spawn(async move {
                let client_start = Instant::now();
                
                while client_start.elapsed() < test_duration {
                    match timeout(Duration::from_millis(100), receiver.next()).await {
                        Ok(Some(Ok(Message::Text(text)))) => {
                            if let Ok(message) = serde_json::from_str::<ChatMessage>(&text) {
                                local_received += 1;
                                received_counter.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                        Ok(Some(Ok(Message::Close(_)))) => break,
                        Ok(Some(Err(_))) => break,
                        Ok(None) => break,
                        Err(_) => continue, // Timeout, continue
                        _ => continue, // Other message types
                    }
                }
                local_received
            });

            let sent = send_task.await.unwrap_or(0);
            let received = receive_task.await.unwrap_or(0);
            connection_counter.fetch_sub(1, Ordering::Relaxed);
            
            (sent, received)
        });
    }

    // Monitor progress
    let monitor_sent = total_messages_sent.clone();
    let monitor_received = total_messages_received.clone();
    let monitor_connections = active_connections.clone();
    
    let monitor_task = tokio::spawn(async move {
        let mut monitor_timer = tokio::time::interval(Duration::from_secs(5));
        let monitor_start = Instant::now();
        
        while monitor_start.elapsed() < test_duration {
            monitor_timer.tick().await;
            let sent = monitor_sent.load(Ordering::Relaxed);
            let received = monitor_received.load(Ordering::Relaxed);
            let connections = monitor_connections.load(Ordering::Relaxed);
            
            println!("Progress: {} active connections, {} sent, {} received", 
                     connections, sent, received);
        }
    });

    // Wait for test completion
    sleep(test_duration + Duration::from_secs(5)).await;
    monitor_task.abort();

    // Collect results
    let mut total_client_sent = 0;
    let mut total_client_received = 0;
    let mut completed_clients = 0;

    while let Some(result) = join_set.join_next().await {
        if let Ok((sent, received)) = result {
            total_client_sent += sent;
            total_client_received += received;
            completed_clients += 1;
        }
    }

    let elapsed = start_time.elapsed();
    let final_sent = total_messages_sent.load(Ordering::Relaxed);
    let final_received = total_messages_received.load(Ordering::Relaxed);
    
    let messages_per_second = final_sent as f64 / elapsed.as_secs_f64();
    let efficiency = if final_sent > 0 { 
        (final_received as f64 / (final_sent as f64 * client_count as f64)) * 100.0 
    } else { 
        0.0 
    };

    println!("\n=== Massive Scale Test Results ===");
    println!("Test duration: {:?}", elapsed);
    println!("Target clients: {}", client_count);
    println!("Completed clients: {}", completed_clients);
    println!("Total messages sent: {}", final_sent);
    println!("Total messages received: {}", final_received);
    println!("Messages per second: {:.2}", messages_per_second);
    println!("Broadcast efficiency: {:.2}%", efficiency);
    println!("Average msgs/client: {:.1}", final_sent as f64 / completed_clients as f64);

    // Assertions for realistic performance
    assert!(completed_clients >= client_count * 90 / 100, 
            "Less than 90% clients completed: {}/{}", completed_clients, client_count);
    
    assert!(final_sent >= client_count * 10, // At least 10 messages per client in 15s
            "Insufficient messages sent: {} < {}", final_sent, client_count * 10);
    
    assert!(efficiency >= 80.0, 
            "Broadcast efficiency too low: {:.2}% < 80%", efficiency);

    Ok(())
}

#[tokio::test]
async fn test_connection_stability() -> WsResult<()> {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("ws://127.0.0.1:{}", addr.port());

    tokio::spawn(async move {
        run_chat_server(listener).await;
    });

    sleep(Duration::from_millis(100)).await;

    let max_connections = 100;
    let mut connections = Vec::new();
    let connect_start = Instant::now();

    // Test rapid connection establishment
    for i in 0..max_connections {
        let stream = TcpStream::connect(addr).await?;
        let url = Url::parse(&url).unwrap();
        let (ws_stream, _) = client_async(url, stream).await?;
        connections.push(ws_stream);
        
        if i % 20 == 0 {
            println!("Established {} connections", i + 1);
        }
    }

    let connect_time = connect_start.elapsed();
    println!("Connection establishment: {} connections in {:?}", max_connections, connect_time);
    println!("Connection rate: {:.2} connections/sec", max_connections as f64 / connect_time.as_secs_f64());

    // Keep connections alive for a while
    sleep(Duration::from_secs(5)).await;

    assert!(connect_time.as_secs() < 10, "Connection establishment too slow");
    assert!(connections.len() == max_connections, "Not all connections established");

    Ok(())
}