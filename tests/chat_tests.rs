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
async fn test_server_accepts_connections() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        run_chat_server(listener).await;
    });

    sleep(Duration::from_millis(100)).await;

    let stream = TcpStream::connect(addr).await;
    assert!(stream.is_ok());
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

    let stream1 = TcpStream::connect(addr).await.unwrap();
    let stream2 = TcpStream::connect(addr).await.unwrap();
    
    let url1 = Url::parse(&url).unwrap();
    let url2 = Url::parse(&url).unwrap();
    
    let (ws1, _) = client_async(url1, stream1).await.unwrap();
    let (ws2, _) = client_async(url2, stream2).await.unwrap();
    
    let (mut sender1, _) = ws1.split();
    let (_, mut receiver2) = ws2.split();

    sleep(Duration::from_millis(50)).await;

    sender1.send(Message::Text("Hello from client 1".to_string())).await.unwrap();

    let result = timeout(Duration::from_secs(1), receiver2.next()).await;
    assert!(result.is_ok());
    
    if let Some(Ok(Message::Text(text))) = result.unwrap() {
        let message: ChatMessage = serde_json::from_str(&text).unwrap();
        assert!(message.content.contains("Hello from client 1"));
    } else {
        panic!("Expected text message");
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
    let (ws, _) = client_async(url, stream).await.unwrap();
    let (mut sender, mut receiver) = ws.split();

    sleep(Duration::from_millis(50)).await;

    sender.send(Message::Text("Self message".to_string())).await.unwrap();

    let result = timeout(Duration::from_millis(500), receiver.next()).await;
    // Should timeout since sender doesn't receive own messages
    assert!(result.is_err());
    
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

    let stream_sender = TcpStream::connect(addr).await.unwrap();
    let stream_recv1 = TcpStream::connect(addr).await.unwrap();
    let stream_recv2 = TcpStream::connect(addr).await.unwrap();
    
    let url_sender = Url::parse(&url).unwrap();
    let url_recv1 = Url::parse(&url).unwrap();
    let url_recv2 = Url::parse(&url).unwrap();
    
    let (ws_sender, _) = client_async(url_sender, stream_sender).await.unwrap();
    let (ws_recv1, _) = client_async(url_recv1, stream_recv1).await.unwrap();
    let (ws_recv2, _) = client_async(url_recv2, stream_recv2).await.unwrap();
    
    let (mut sender, _) = ws_sender.split();
    let (_, mut receiver1) = ws_recv1.split();
    let (_, mut receiver2) = ws_recv2.split();

    sleep(Duration::from_millis(50)).await;

    sender.send(Message::Text("Broadcast message".to_string())).await.unwrap();

    let result1 = timeout(Duration::from_secs(1), receiver1.next()).await;
    let result2 = timeout(Duration::from_secs(1), receiver2.next()).await;

    assert!(result1.is_ok());
    assert!(result2.is_ok());
    
    if let Some(Ok(Message::Text(text1))) = result1.unwrap() {
        let message1: ChatMessage = serde_json::from_str(&text1).unwrap();
        assert!(message1.content.contains("Broadcast message"));
    }
    
    if let Some(Ok(Message::Text(text2))) = result2.unwrap() {
        let message2: ChatMessage = serde_json::from_str(&text2).unwrap();
        assert!(message2.content.contains("Broadcast message"));
    }
    
    Ok(())
}

#[tokio::test]
async fn test_client_disconnect_handling() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        run_chat_server(listener).await;
    });

    sleep(Duration::from_millis(100)).await;

    {
        let client = TcpStream::connect(addr).await.unwrap();
        drop(client);
    }

    sleep(Duration::from_millis(100)).await;

    let client2 = TcpStream::connect(addr).await;
    assert!(client2.is_ok());
}

#[tokio::test]
async fn test_throughput_client_to_client() -> WsResult<()> {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("ws://127.0.0.1:{}", addr.port());

    tokio::spawn(async move {
        run_chat_server(listener).await;
    });

    sleep(Duration::from_millis(100)).await;

    let stream_sender = TcpStream::connect(addr).await.unwrap();
    let stream_receiver = TcpStream::connect(addr).await.unwrap();
    
    let url_sender = Url::parse(&url).unwrap();
    let url_receiver = Url::parse(&url).unwrap();
    
    let (ws_sender, _) = client_async(url_sender, stream_sender).await.unwrap();
    let (ws_receiver, _) = client_async(url_receiver, stream_receiver).await.unwrap();
    
    let (mut sender, _) = ws_sender.split();
    let (_, mut receiver) = ws_receiver.split();

    sleep(Duration::from_millis(100)).await;

    let message_count = 100; // Reduced for WebSocket overhead
    let start_time = std::time::Instant::now();

    for i in 0..message_count {
        let test_message = format!("Throughput test {}", i);
        sender.send(Message::Text(test_message)).await.unwrap();
        
        if i == 0 {
            sleep(Duration::from_millis(50)).await;
        }
    }

    let mut received_count = 0;
    for _ in 0..message_count {
        match timeout(Duration::from_secs(10), receiver.next()).await {
            Ok(Some(Ok(Message::Text(text)))) => {
                if let Ok(_message) = serde_json::from_str::<ChatMessage>(&text) {
                    received_count += 1;
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

    assert!(received_count >= message_count * 8 / 10, "Less than 80% of messages were received");
    assert!(elapsed.as_secs() < 30);
    
    Ok(())
}