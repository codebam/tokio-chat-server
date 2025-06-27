use chat_server::run_chat_server;
use std::time::Duration;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream},
    time::{sleep, timeout},
};

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
async fn test_message_broadcast() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        run_chat_server(listener).await;
    });

    sleep(Duration::from_millis(100)).await;

    let mut client1 = TcpStream::connect(addr).await.unwrap();
    let mut client2 = TcpStream::connect(addr).await.unwrap();

    sleep(Duration::from_millis(50)).await;

    client1.write_all(b"Hello from client 1\n").await.unwrap();

    let (reader, _) = client2.split();
    let mut reader = BufReader::new(reader);
    let mut buffer = String::new();

    let result = timeout(Duration::from_secs(1), reader.read_line(&mut buffer)).await;
    assert!(result.is_ok());
    assert_eq!(buffer, "Hello from client 1\n");
}

#[tokio::test]
async fn test_sender_does_not_receive_own_message() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        run_chat_server(listener).await;
    });

    sleep(Duration::from_millis(100)).await;

    let mut client = TcpStream::connect(addr).await.unwrap();

    sleep(Duration::from_millis(50)).await;

    client.write_all(b"Self message\n").await.unwrap();

    let (reader, _) = client.split();
    let mut reader = BufReader::new(reader);
    let mut buffer = String::new();

    let result = timeout(Duration::from_millis(500), reader.read_line(&mut buffer)).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_multiple_clients_receive_messages() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        run_chat_server(listener).await;
    });

    sleep(Duration::from_millis(100)).await;

    let mut sender = TcpStream::connect(addr).await.unwrap();
    let mut receiver1 = TcpStream::connect(addr).await.unwrap();
    let mut receiver2 = TcpStream::connect(addr).await.unwrap();

    sleep(Duration::from_millis(50)).await;

    sender.write_all(b"Broadcast message\n").await.unwrap();

    let (reader1, _) = receiver1.split();
    let (reader2, _) = receiver2.split();
    let mut reader1 = BufReader::new(reader1);
    let mut reader2 = BufReader::new(reader2);
    let mut buffer1 = String::new();
    let mut buffer2 = String::new();

    let result1 = timeout(Duration::from_secs(1), reader1.read_line(&mut buffer1)).await;
    let result2 = timeout(Duration::from_secs(1), reader2.read_line(&mut buffer2)).await;

    assert!(result1.is_ok());
    assert!(result2.is_ok());
    assert_eq!(buffer1, "Broadcast message\n");
    assert_eq!(buffer2, "Broadcast message\n");
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
async fn test_throughput_client_to_client() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        run_chat_server(listener).await;
    });

    sleep(Duration::from_millis(100)).await;

    let mut sender = TcpStream::connect(addr).await.unwrap();
    let mut receiver = TcpStream::connect(addr).await.unwrap();

    sleep(Duration::from_millis(100)).await;

    let message_count = 2000;
    let test_message = "Throughput test\n";

    let start_time = std::time::Instant::now();

    for i in 0..message_count {
        sender.write_all(test_message.as_bytes()).await.unwrap();
        
        if i == 0 {
            sleep(Duration::from_millis(50)).await;
        }
        if i % 50 == 0 && i > 0 {
            sender.flush().await.unwrap();
        }
    }
    sender.flush().await.unwrap();

    let (reader, _) = receiver.split();
    let mut reader = BufReader::new(reader);
    let mut buffer = String::new();
    let mut received_count = 0;

    for _ in 0..message_count {
        buffer.clear();
        match timeout(Duration::from_secs(10), reader.read_line(&mut buffer)).await {
            Ok(Ok(bytes_read)) if bytes_read > 0 => {
                received_count += 1;
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
}