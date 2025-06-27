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
async fn test_simple_throughput() -> WsResult<()> {
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
    
    let (mut sender1, _receiver1) = ws1.split();
    let (_sender2, mut receiver2) = ws2.split();

    let message_count = 10000;
    let start_time = Instant::now();

    let send_task = tokio::spawn(async move {
        for i in 0..message_count {
            let message = format!("Test message {}", i);
            if sender1.send(Message::Text(message)).await.is_err() {
                break;
            }
            if i % 100 == 0 {
                sleep(Duration::from_micros(10)).await;
            }
        }
    });

    let mut received_count = 0;
    let receive_task = tokio::spawn(async move {
        while let Some(msg) = receiver2.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    if let Ok(messages) = serde_json::from_str::<Vec<ChatMessage>>(&text) {
                        received_count += messages.len();
                        if received_count >= message_count * 80 / 100 {
                            break;
                        }
                    }
                }
                _ => break,
            }
        }
        received_count
    });

    let _ = send_task.await;
    let received = timeout(Duration::from_secs(10), receive_task).await
        .unwrap_or(Ok(0))
        .unwrap_or(0);

    let elapsed = start_time.elapsed();
    let messages_per_second = received as f64 / elapsed.as_secs_f64();

    println!("Simple throughput test results:");
    println!("Messages sent: {}", message_count);
    println!("Messages received: {}", received);
    println!("Time elapsed: {:?}", elapsed);
    println!("Messages per second: {:.2}", messages_per_second);

    assert!(received >= message_count * 50 / 100, "Less than 50% of messages received");
    assert!(messages_per_second >= 50000.0, "Performance target not met: {:.2} msg/sec < 50000 msg/sec", messages_per_second);

    Ok(())
}