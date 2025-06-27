use chat_server::{run_chat_server, ChatMessage};
use tokio::{
    net::{TcpListener, TcpStream},
    time::{sleep, timeout, Duration, Instant},
};
use tokio_tungstenite::{
    client_async, 
    tungstenite::{Message, Result as WsResult},
};
use futures_util::{SinkExt, StreamExt};
use url::Url;

#[tokio::test]
async fn test_websocket_high_throughput() -> WsResult<()> {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("ws://127.0.0.1:{}", addr.port());

    tokio::spawn(async move {
        run_chat_server(listener).await;
    });

    sleep(Duration::from_millis(100)).await;

    let client_count = 10;
    let messages_per_client = 1000;
    let total_expected_messages = client_count * messages_per_client;

    let mut handles = Vec::new();
    let start_time = Instant::now();

    for client_id in 0..client_count {
        let client_url = url.clone();
        let handle = tokio::spawn(async move {
            let stream = TcpStream::connect(format!("127.0.0.1:{}", addr.port())).await.unwrap();
            let url = Url::parse(&client_url).unwrap();
            let (ws_stream, _) = client_async(url, stream).await.unwrap();
            let (mut sender, mut receiver) = ws_stream.split();

            let send_task = tokio::spawn(async move {
                for i in 0..messages_per_client {
                    let message = format!("Client {} message {}", client_id, i);
                    if let Err(_) = sender.send(Message::Text(message)).await {
                        break;
                    }
                    if i % 100 == 0 {
                        sleep(Duration::from_micros(10)).await;
                    }
                }
            });

            let mut received_count = 0;
            let receive_task = tokio::spawn(async move {
                while let Some(msg) = receiver.next().await {
                    match msg {
                        Ok(Message::Text(text)) => {
                            if let Ok(messages) = serde_json::from_str::<Vec<ChatMessage>>(&text) {
                                received_count += messages.len();
                            }
                        }
                        _ => break,
                    }
                }
                received_count
            });

            let _ = send_task.await;
            let received = timeout(Duration::from_secs(60), receive_task).await
                .unwrap_or(Ok(0))
                .unwrap_or(0);
            
            received
        });
        handles.push(handle);
    }

    let results = futures_util::future::join_all(handles).await;
    let total_received: usize = results.into_iter()
        .map(|r| r.unwrap_or(0))
        .sum();

    let elapsed = start_time.elapsed();
    let messages_per_second = total_received as f64 / elapsed.as_secs_f64();

    println!("High throughput WebSocket test results:");
    println!("Clients: {}", client_count);
    println!("Messages per client: {}", messages_per_client);
    println!("Total messages sent: {}", total_expected_messages);
    println!("Total messages received: {}", total_received);
    println!("Time elapsed: {:?}", elapsed);
    println!("Messages per second: {:.2}", messages_per_second);

    assert!(messages_per_second >= 10000.0, 
            "Performance target not met: {:.2} msg/sec < 10000 msg/sec", 
            messages_per_second);

    Ok(())
}

#[tokio::test]
async fn test_websocket_basic_functionality() -> WsResult<()> {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("ws://127.0.0.1:{}", addr.port());

    tokio::spawn(async move {
        run_chat_server(listener).await;
    });

    sleep(Duration::from_millis(100)).await;

    let stream1 = TcpStream::connect(addr).await.unwrap();
    let stream2 = TcpStream::connect(addr).await.unwrap();
    
    let url1 = Url::parse(&url).unwrap();
    let url2 = Url::parse(&url).unwrap();
    
    let (ws1, _) = client_async(url1, stream1).await.unwrap();
    let (ws2, _) = client_async(url2, stream2).await.unwrap();
    
    let (mut sender1, mut receiver1) = ws1.split();
    let (mut sender2, mut receiver2) = ws2.split();

    sender1.send(Message::Text("Hello from client 1".to_string())).await.unwrap();

    let msg = timeout(Duration::from_secs(5), receiver2.next()).await
        .unwrap()
        .unwrap()
        .unwrap();
    
    if let Message::Text(text) = msg {
        let messages: Vec<ChatMessage> = serde_json::from_str(&text).unwrap();
        assert!(!messages.is_empty());
        assert!(messages[0].content.contains("Hello from client 1"));
    } else {
        panic!("Expected text message");
    }

    Ok(())
}