use chat_server::{run_chat_server_tls, create_test_tls_acceptor, ChatMessage};
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
use native_tls::TlsConnector;
use tokio_native_tls::TlsConnector as TokioTlsConnector;

#[tokio::test]
async fn test_websocket_high_throughput() -> WsResult<()> {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("wss://127.0.0.1:{}", addr.port());

    let tls_acceptor = create_test_tls_acceptor().expect("Failed to create TLS acceptor");
    tokio::spawn(async move {
        run_chat_server_tls(listener, tls_acceptor).await;
    });

    sleep(Duration::from_millis(100)).await;

    // Create TLS connector that accepts self-signed certificates for testing
    let mut tls_builder = TlsConnector::builder();
    tls_builder.danger_accept_invalid_certs(true);
    tls_builder.danger_accept_invalid_hostnames(true);
    let tls_connector = tls_builder.build().unwrap();
    let tokio_connector = TokioTlsConnector::from(tls_connector);

    let client_count = 100;
    let messages_per_client = 100;
    let total_expected_messages = client_count * messages_per_client;

    let mut handles = Vec::new();
    let start_time = Instant::now();

    for client_id in 0..client_count {
        let client_url = url.clone();
        let tokio_connector_clone = tokio_connector.clone();
        let handle = tokio::spawn(async move {
            let stream = TcpStream::connect(addr).await.unwrap();
            let tls_stream = tokio_connector_clone.connect("127.0.0.1", stream).await.unwrap();
            let url = Url::parse(&client_url).unwrap();
            let (ws_stream, _) = client_async(url, tls_stream).await.unwrap();
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

            let receive_task = tokio::spawn(async move {
                let mut received_count = 0;
                // Each client should receive messages from all OTHER clients (not itself)
                // With 100 clients sending 100 messages each, each client should receive ~9900 messages
                let expected_from_others = (client_count - 1) * messages_per_client;
                let timeout_duration = Duration::from_secs(60);
                let start = Instant::now();
                
                while received_count < expected_from_others && start.elapsed() < timeout_duration {
                    if let Ok(Some(msg)) = timeout(Duration::from_millis(100), receiver.next()).await {
                        match msg {
                            Ok(Message::Text(text)) => {
                                if let Ok(messages) = serde_json::from_str::<Vec<ChatMessage>>(&text) {
                                    received_count += messages.len();
                                }
                            }
                            _ => break,
                        }
                    }
                }
                received_count
            });

            let _ = send_task.await;
            let received = timeout(Duration::from_secs(35), receive_task).await
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

    assert!(messages_per_second >= 1000.0, 
            "Performance target not met: {:.2} msg/sec < 1000 msg/sec", 
            messages_per_second);

    Ok(())
}

#[tokio::test]
async fn test_websocket_basic_functionality() -> WsResult<()> {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("wss://127.0.0.1:{}", addr.port());

    let tls_acceptor = create_test_tls_acceptor().expect("Failed to create TLS acceptor");
    tokio::spawn(async move {
        run_chat_server_tls(listener, tls_acceptor).await;
    });

    sleep(Duration::from_millis(100)).await;

    // Create TLS connector that accepts self-signed certificates for testing
    let mut tls_builder = TlsConnector::builder();
    tls_builder.danger_accept_invalid_certs(true);
    tls_builder.danger_accept_invalid_hostnames(true);
    let tls_connector = tls_builder.build().unwrap();
    let tokio_connector = TokioTlsConnector::from(tls_connector);

    let stream1 = TcpStream::connect(addr).await.unwrap();
    let tls_stream1 = tokio_connector.connect("127.0.0.1", stream1).await.unwrap();
    let stream2 = TcpStream::connect(addr).await.unwrap();
    let tls_stream2 = tokio_connector.connect("127.0.0.1", stream2).await.unwrap();
    
    let url1 = Url::parse(&url).unwrap();
    let url2 = Url::parse(&url).unwrap();
    
    let (ws1, _) = client_async(url1, tls_stream1).await.unwrap();
    let (ws2, _) = client_async(url2, tls_stream2).await.unwrap();
    
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