use chat_server::{run_chat_server, ChatMessage};
use std::time::Duration;
use tokio::{
    net::{TcpListener, TcpStream},
    time::{sleep, timeout},
};
use tokio_tungstenite::{
    client_async, 
    tungstenite::{Message, Result as WsResult},
};
use futures_util::{SinkExt, StreamExt};
use url::Url;

#[tokio::test]
async fn test_server_accepts_connections() -> WsResult<()> {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("ws://127.0.0.1:{}", addr.port());

    tokio::spawn(async move {
        run_chat_server(listener).await;
    });

    sleep(Duration::from_millis(100)).await;

    let stream = TcpStream::connect(addr).await.unwrap();
    let url = Url::parse(&url).unwrap();
    let (ws_stream, _) = client_async(url, stream).await?;
    
    // Connection successful if we get here
    assert!(true);
    Ok(())
}

#[tokio::test]
async fn test_message_broadcast() -> WsResult<()> {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("ws://127.0.0.1:{}", addr.port());

    tokio::spawn(async move {
        run_chat_server(listener).await;
    });

    sleep(Duration::from_millis(100)).await;

    // Create client 1 (sender)
    let stream1 = TcpStream::connect(addr).await.unwrap();
    let url1 = Url::parse(&url).unwrap();
    let (ws1, _) = client_async(url1, stream1).await?;
    let (mut sender1, _) = ws1.split();

    // Create client 2 (receiver)
    let stream2 = TcpStream::connect(addr).await.unwrap();
    let url2 = Url::parse(&url).unwrap();
    let (ws2, _) = client_async(url2, stream2).await?;
    let (_, mut receiver2) = ws2.split();

    sleep(Duration::from_millis(50)).await;

    // Send message from client 1
    sender1.send(Message::Text("Hello from client 1".to_string())).await?;

    // Receive message on client 2
    let result = timeout(Duration::from_secs(2), receiver2.next()).await;
    assert!(result.is_ok());
    
    if let Some(msg) = result.unwrap() {
        match msg? {
            Message::Text(text) => {
                let messages: Vec<ChatMessage> = serde_json::from_str(&text).unwrap();
                assert!(!messages.is_empty());
                assert!(messages[0].content.contains("Hello from client 1"));
            }
            _ => panic!("Expected text message"),
        }
    }

    Ok(())
}

#[tokio::test]
async fn test_sender_does_not_receive_own_message() -> WsResult<()> {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("ws://127.0.0.1:{}", addr.port());

    tokio::spawn(async move {
        run_chat_server(listener).await;
    });

    sleep(Duration::from_millis(100)).await;

    let stream = TcpStream::connect(addr).await.unwrap();
    let url = Url::parse(&url).unwrap();
    let (ws_stream, _) = client_async(url, stream).await?;
    let (mut sender, mut receiver) = ws_stream.split();

    sleep(Duration::from_millis(50)).await;

    // Send message from same client
    sender.send(Message::Text("Self message".to_string())).await?;

    // Should not receive own message (timeout expected)
    let result = timeout(Duration::from_millis(500), receiver.next()).await;
    assert!(result.is_err(), "Should not receive own message");
    
    Ok(())
}

#[tokio::test]
async fn test_multiple_clients_receive_messages() -> WsResult<()> {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("ws://127.0.0.1:{}", addr.port());

    tokio::spawn(async move {
        run_chat_server(listener).await;
    });

    sleep(Duration::from_millis(100)).await;

    // Create sender
    let stream_sender = TcpStream::connect(addr).await.unwrap();
    let url_sender = Url::parse(&url).unwrap();
    let (ws_sender, _) = client_async(url_sender, stream_sender).await?;
    let (mut sender, _) = ws_sender.split();

    // Create receiver 1
    let stream1 = TcpStream::connect(addr).await.unwrap();
    let url1 = Url::parse(&url).unwrap();
    let (ws1, _) = client_async(url1, stream1).await?;
    let (_, mut receiver1) = ws1.split();

    // Create receiver 2
    let stream2 = TcpStream::connect(addr).await.unwrap();
    let url2 = Url::parse(&url).unwrap();
    let (ws2, _) = client_async(url2, stream2).await?;
    let (_, mut receiver2) = ws2.split();

    sleep(Duration::from_millis(50)).await;

    // Send broadcast message
    sender.send(Message::Text("Broadcast message".to_string())).await?;

    // Both receivers should get the message
    let result1 = timeout(Duration::from_secs(2), receiver1.next()).await;
    let result2 = timeout(Duration::from_secs(2), receiver2.next()).await;

    assert!(result1.is_ok());
    assert!(result2.is_ok());
    
    // Verify message content
    if let Some(msg1) = result1.unwrap() {
        match msg1? {
            Message::Text(text) => {
                let messages: Vec<ChatMessage> = serde_json::from_str(&text).unwrap();
                assert!(messages[0].content.contains("Broadcast message"));
            }
            _ => panic!("Expected text message"),
        }
    }
    
    if let Some(msg2) = result2.unwrap() {
        match msg2? {
            Message::Text(text) => {
                let messages: Vec<ChatMessage> = serde_json::from_str(&text).unwrap();
                assert!(messages[0].content.contains("Broadcast message"));
            }
            _ => panic!("Expected text message"),
        }
    }

    Ok(())
}

#[tokio::test]
async fn test_client_disconnect_handling() -> WsResult<()> {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("ws://127.0.0.1:{}", addr.port());

    tokio::spawn(async move {
        run_chat_server(listener).await;
    });

    sleep(Duration::from_millis(100)).await;

    // Connect and immediately disconnect a client
    {
        let stream = TcpStream::connect(addr).await.unwrap();
        let url_client = Url::parse(&url).unwrap();
        let (ws_stream, _) = client_async(url_client, stream).await?;
        drop(ws_stream); // Disconnect
    }

    sleep(Duration::from_millis(100)).await;

    // Server should still accept new connections
    let stream2 = TcpStream::connect(addr).await.unwrap();
    let url2 = Url::parse(&url).unwrap();
    let (ws_stream2, _) = client_async(url2, stream2).await?;
    assert!(true); // Connection successful
    
    Ok(())
}

#[tokio::test]
async fn test_throughput_client_to_client() -> WsResult<()> {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("ws://127.0.0.1:{}", addr.port());

    tokio::spawn(async move {
        run_chat_server(listener).await;
    });

    sleep(Duration::from_millis(200)).await;

    // Create sender
    let stream_sender = TcpStream::connect(addr).await.unwrap();
    let url_sender = Url::parse(&url).unwrap();
    let (ws_sender, _) = client_async(url_sender, stream_sender).await?;
    let (mut sender, _) = ws_sender.split();

    // Create receiver
    let stream_receiver = TcpStream::connect(addr).await.unwrap();
    let url_receiver = Url::parse(&url).unwrap();
    let (ws_receiver, _) = client_async(url_receiver, stream_receiver).await?;
    let (_, mut receiver) = ws_receiver.split();

    sleep(Duration::from_millis(100)).await;

    let message_count = 500;
    let start_time = std::time::Instant::now();

    // Send messages
    for i in 0..message_count {
        let message = format!("Throughput test {}", i);
        sender.send(Message::Text(message)).await?;
        
        if i % 25 == 0 {
            sleep(Duration::from_micros(100)).await;
        }
    }

    // Receive messages
    let mut received_count = 0;
    let timeout_duration = Duration::from_secs(10);
    
    while received_count < message_count {
        match timeout(timeout_duration, receiver.next()).await {
            Ok(Some(msg)) => {
                match msg? {
                    Message::Text(text) => {
                        if let Ok(messages) = serde_json::from_str::<Vec<ChatMessage>>(&text) {
                            received_count += messages.len();
                        }
                    }
                    _ => {}
                }
            }
            _ => break,
        }
    }

    let elapsed = start_time.elapsed();

    println!("Throughput test results:");
    println!("Messages sent: {}", message_count);
    println!("Messages received: {}", received_count);
    println!("Time elapsed: {:?}", elapsed);
    println!("Messages per second: {:.2}", received_count as f64 / elapsed.as_secs_f64());

    assert!(received_count >= message_count * 8 / 10, "Less than 80% of messages were received: {}/{}", received_count, message_count);
    assert!(elapsed.as_secs() < 30);
    
    Ok(())
}