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
async fn test_multicore_high_throughput() -> WsResult<()> {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("ws://127.0.0.1:{}", addr.port());

    tokio::spawn(async move {
        run_chat_server(listener).await;
    });

    sleep(Duration::from_millis(200)).await;


    let concurrent_clients = 50;
    let messages_per_client = 1000;
    let total_messages_sent = concurrent_clients * messages_per_client;
    
    let total_received = Arc::new(AtomicUsize::new(0));
    let start_time = Instant::now();

    let mut join_set = JoinSet::new();

    for client_id in 0..concurrent_clients {
        let client_url = url.clone();
        let received_counter = total_received.clone();
        
        join_set.spawn(async move {
            let stream = TcpStream::connect(addr).await.unwrap();
            let url = Url::parse(&client_url).unwrap();
            let (ws_stream, _) = client_async(url, stream).await.unwrap();
            let (mut sender, mut receiver) = ws_stream.split();

            let send_task = tokio::spawn(async move {
                for i in 0..messages_per_client {
                    let message = format!("Client-{}-Message-{}", client_id, i);
                    if sender.send(Message::Text(message)).await.is_err() {
                        break;
                    }
                    
                    if i % 100 == 0 {
                        sleep(Duration::from_micros(5)).await;
                    }
                }
            });

            let receive_task = tokio::spawn(async move {
                let mut local_count = 0;
                while let Some(msg) = receiver.next().await {
                    match msg {
                        Ok(Message::Text(text)) => {
                            if let Ok(messages) = serde_json::from_str::<Vec<ChatMessage>>(&text) {
                                local_count += messages.len();
                                received_counter.fetch_add(messages.len(), Ordering::Relaxed);
                            }
                        }
                        _ => break,
                    }
                    
                    if local_count >= messages_per_client * 80 / 100 {
                        break;
                    }
                }
                local_count
            });

            let _ = send_task.await;
            let received = timeout(Duration::from_secs(30), receive_task).await
                .unwrap_or(Ok(0))
                .unwrap_or(0);
            
            received
        });
    }

    let mut client_results = Vec::new();
    while let Some(result) = join_set.join_next().await {
        if let Ok(received) = result {
            client_results.push(received);
        }
    }

    let elapsed = start_time.elapsed();
    let total_received_final = total_received.load(Ordering::Relaxed);
    let messages_per_second = total_received_final as f64 / elapsed.as_secs_f64();

    println!("Multicore high throughput test results:");
    println!("Concurrent clients: {}", concurrent_clients);
    println!("Messages per client: {}", messages_per_client);
    println!("Total messages sent: {}", total_messages_sent);
    println!("Total messages received: {}", total_received_final);
    println!("Time elapsed: {:?}", elapsed);
    println!("Messages per second: {:.2}", messages_per_second);
    println!("Efficiency: {:.1}%", (total_received_final as f64 / total_messages_sent as f64) * 100.0);

    assert!(total_received_final >= total_messages_sent * 60 / 100, 
            "Less than 60% of messages received: {}/{}", total_received_final, total_messages_sent);
    
    assert!(messages_per_second >= 40000.0, 
            "Performance target not met: {:.2} msg/sec < 40000 msg/sec", 
            messages_per_second);

    Ok(())
}

#[tokio::test]
async fn test_connection_scaling() -> WsResult<()> {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("ws://127.0.0.1:{}", addr.port());

    tokio::spawn(async move {
        run_chat_server(listener).await;
    });

    sleep(Duration::from_millis(100)).await;

    let max_connections = 100;
    let mut connections = Vec::new();

    let start_time = Instant::now();
    
    for i in 0..max_connections {
        let stream = TcpStream::connect(addr).await.unwrap();
        let url = Url::parse(&url).unwrap();
        let (ws_stream, _) = client_async(url, stream).await.unwrap();
        connections.push(ws_stream);
        
        if i % 10 == 0 {
            println!("Established {} connections", i + 1);
        }
    }

    let connection_time = start_time.elapsed();
    println!("Connection scaling test results:");
    println!("Connections established: {}", max_connections);
    println!("Time to establish all connections: {:?}", connection_time);
    println!("Connections per second: {:.2}", max_connections as f64 / connection_time.as_secs_f64());

    assert!(connection_time.as_secs() < 10, "Connection establishment took too long");
    
    Ok(())
}