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
async fn test_single_sender_massive_fanout() -> WsResult<()> {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("ws://127.0.0.1:{}", addr.port());

    tokio::spawn(async move {
        run_chat_server(listener).await;
    });

    sleep(Duration::from_millis(300)).await;


    // Test 1 sender broadcasting to 100 receivers (high fan-out)
    let num_receivers = 100;
    let messages_to_send = 500;
    
    println!("Testing 1-to-{} fan-out with {} messages", num_receivers, messages_to_send);
    
    let total_received = Arc::new(AtomicUsize::new(0));
    let start_time = Instant::now();

    // Create sender
    let sender_stream = TcpStream::connect(addr).await.unwrap();
    let sender_url = Url::parse(&url).unwrap();
    let (sender_ws, _) = client_async(sender_url, sender_stream).await.unwrap();
    let (mut sender, _sender_receiver) = sender_ws.split();

    // Create receivers
    let mut join_set = JoinSet::new();
    for receiver_id in 0..num_receivers {
        let receiver_url = url.clone();
        let received_counter = total_received.clone();
        
        join_set.spawn(async move {
            let stream = TcpStream::connect(addr).await.unwrap();
            let url = Url::parse(&receiver_url).unwrap();
            let (ws_stream, _) = client_async(url, stream).await.unwrap();
            let (_sender, mut receiver) = ws_stream.split();

            let mut received_count = 0;
            let receive_timeout = Duration::from_secs(15);
            let start = Instant::now();
            
            while start.elapsed() < receive_timeout {
                if let Ok(Some(msg)) = timeout(Duration::from_millis(100), receiver.next()).await {
                    match msg {
                        Ok(Message::Text(text)) => {
                            if let Ok(messages) = serde_json::from_str::<Vec<ChatMessage>>(&text) {
                                received_count += messages.len();
                                received_counter.fetch_add(messages.len(), Ordering::Relaxed);
                                
                                if received_count >= messages_to_send {
                                    break;
                                }
                            }
                        }
                        _ => break,
                    }
                } else {
                    // Small delay to prevent busy waiting
                    sleep(Duration::from_millis(1)).await;
                }
            }
            
            println!("Receiver {} got {} messages", receiver_id, received_count);
            received_count
        });
    }

    // Wait for receivers to be ready
    sleep(Duration::from_millis(500)).await;

    // Send messages from single sender
    let send_start = Instant::now();
    for i in 0..messages_to_send {
        let message = format!("Fanout test message {}", i);
        if sender.send(Message::Text(message)).await.is_err() {
            break;
        }
        
        // Small delay to prevent overwhelming
        if i % 50 == 0 {
            sleep(Duration::from_micros(100)).await;
        }
    }
    let send_duration = send_start.elapsed();

    // Collect results
    let mut receiver_results = Vec::new();
    while let Some(result) = join_set.join_next().await {
        if let Ok(received) = result {
            receiver_results.push(received);
        }
    }

    let total_time = start_time.elapsed();
    let total_received_final = total_received.load(Ordering::Relaxed);
    let expected_total = messages_to_send * num_receivers;
    let fanout_efficiency = (total_received_final as f64 / expected_total as f64) * 100.0;
    let messages_per_second = total_received_final as f64 / total_time.as_secs_f64();

    println!("Fan-out test results:");
    println!("Senders: 1, Receivers: {}", num_receivers);
    println!("Messages sent: {}", messages_to_send);
    println!("Expected total received: {}", expected_total);
    println!("Actual total received: {}", total_received_final);
    println!("Send time: {:?}", send_duration);
    println!("Total time: {:?}", total_time);
    println!("Fan-out efficiency: {:.1}%", fanout_efficiency);
    println!("Messages per second: {:.2}", messages_per_second);
    println!("Fan-out multiplier: {}x", num_receivers);

    // Performance assertions for fan-out
    assert!(fanout_efficiency >= 80.0, 
            "Fan-out efficiency too low: {:.1}% < 80%", fanout_efficiency);
    
    assert!(messages_per_second >= 40000.0, 
            "Fan-out throughput too low: {:.2} msg/sec < 40k msg/sec", 
            messages_per_second);

    Ok(())
}

#[tokio::test]
async fn test_multiple_senders_fanout() -> WsResult<()> {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("ws://127.0.0.1:{}", addr.port());

    tokio::spawn(async move {
        run_chat_server(listener).await;
    });

    sleep(Duration::from_millis(300)).await;


    // Test N senders broadcasting to M receivers (complex fan-out)
    let num_senders = 10;
    let num_receivers = 50;
    let messages_per_sender = 200;
    
    println!("Testing {}-to-{} fan-out with {} messages per sender", 
             num_senders, num_receivers, messages_per_sender);
    
    let total_received = Arc::new(AtomicUsize::new(0));
    let start_time = Instant::now();

    let mut join_set = JoinSet::new();

    // Create receivers first
    for receiver_id in 0..num_receivers {
        let receiver_url = url.clone();
        let received_counter = total_received.clone();
        
        join_set.spawn(async move {
            let stream = TcpStream::connect(addr).await.unwrap();
            let url = Url::parse(&receiver_url).unwrap();
            let (ws_stream, _) = client_async(url, stream).await.unwrap();
            let (_sender, mut receiver) = ws_stream.split();

            let mut received_count = 0;
            let expected_messages = num_senders * messages_per_sender;
            let receive_timeout = Duration::from_secs(20);
            let start = Instant::now();
            
            while received_count < expected_messages && start.elapsed() < receive_timeout {
                if let Ok(Some(msg)) = timeout(Duration::from_millis(100), receiver.next()).await {
                    match msg {
                        Ok(Message::Text(text)) => {
                            if let Ok(messages) = serde_json::from_str::<Vec<ChatMessage>>(&text) {
                                received_count += messages.len();
                                received_counter.fetch_add(messages.len(), Ordering::Relaxed);
                            }
                        }
                        _ => break,
                    }
                }
            }
            
            received_count
        });
    }

    // Wait for receivers to be ready
    sleep(Duration::from_millis(700)).await;

    // Create senders
    for sender_id in 0..num_senders {
        let sender_url = url.clone();
        
        join_set.spawn(async move {
            let stream = TcpStream::connect(addr).await.unwrap();
            let url = Url::parse(&sender_url).unwrap();
            let (ws_stream, _) = client_async(url, stream).await.unwrap();
            let (mut sender, _receiver) = ws_stream.split();

            for i in 0..messages_per_sender {
                let message = format!("Sender-{}-Message-{}", sender_id, i);
                if sender.send(Message::Text(message)).await.is_err() {
                    break;
                }
                
                if i % 25 == 0 {
                    sleep(Duration::from_micros(200)).await;
                }
            }
            
            messages_per_sender
        });
    }

    // Collect results
    let mut results = Vec::new();
    while let Some(result) = join_set.join_next().await {
        if let Ok(count) = result {
            results.push(count);
        }
    }

    let total_time = start_time.elapsed();
    let total_received_final = total_received.load(Ordering::Relaxed);
    let total_sent = num_senders * messages_per_sender;
    let expected_total = total_sent * num_receivers;
    let fanout_efficiency = (total_received_final as f64 / expected_total as f64) * 100.0;
    let messages_per_second = total_received_final as f64 / total_time.as_secs_f64();

    println!("Complex fan-out test results:");
    println!("Senders: {}, Receivers: {}", num_senders, num_receivers);
    println!("Total messages sent: {}", total_sent);
    println!("Expected total received ({}x fan-out): {}", num_receivers, expected_total);
    println!("Actual total received: {}", total_received_final);
    println!("Total time: {:?}", total_time);
    println!("Fan-out efficiency: {:.1}%", fanout_efficiency);
    println!("Messages per second: {:.2}", messages_per_second);
    println!("Average fan-out multiplier: {}x", num_receivers);

    // Performance assertions for complex fan-out
    assert!(fanout_efficiency >= 75.0, 
            "Complex fan-out efficiency too low: {:.1}% < 75%", fanout_efficiency);
    
    assert!(messages_per_second >= 4000.0, 
            "Complex fan-out throughput too low: {:.2} msg/sec < 4k msg/sec", 
            messages_per_second);

    Ok(())
}

#[tokio::test]
async fn test_burst_fanout_performance() -> WsResult<()> {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("ws://127.0.0.1:{}", addr.port());

    tokio::spawn(async move {
        run_chat_server(listener).await;
    });

    sleep(Duration::from_millis(300)).await;


    // Test burst sending with high fan-out
    let num_receivers = 75;
    let burst_size = 1000;
    
    println!("Testing burst fan-out: {} messages to {} receivers instantly", 
             burst_size, num_receivers);
    
    let total_received = Arc::new(AtomicUsize::new(0));
    let start_time = Instant::now();

    // Create sender
    let sender_stream = TcpStream::connect(addr).await.unwrap();
    let sender_url = Url::parse(&url).unwrap();
    let (sender_ws, _) = client_async(sender_url, sender_stream).await.unwrap();
    let (mut sender, _sender_receiver) = sender_ws.split();

    // Create receivers
    let mut join_set = JoinSet::new();
    for receiver_id in 0..num_receivers {
        let receiver_url = url.clone();
        let received_counter = total_received.clone();
        
        join_set.spawn(async move {
            let stream = TcpStream::connect(addr).await.unwrap();
            let url = Url::parse(&receiver_url).unwrap();
            let (ws_stream, _) = client_async(url, stream).await.unwrap();
            let (_sender, mut receiver) = ws_stream.split();

            let mut received_count = 0;
            let receive_timeout = Duration::from_secs(10);
            let start = Instant::now();
            
            while start.elapsed() < receive_timeout && received_count < burst_size {
                if let Ok(Some(msg)) = timeout(Duration::from_millis(50), receiver.next()).await {
                    match msg {
                        Ok(Message::Text(text)) => {
                            if let Ok(messages) = serde_json::from_str::<Vec<ChatMessage>>(&text) {
                                received_count += messages.len();
                                received_counter.fetch_add(messages.len(), Ordering::Relaxed);
                            }
                        }
                        _ => break,
                    }
                }
            }
            
            received_count
        });
    }

    // Wait for receivers to be ready
    sleep(Duration::from_millis(500)).await;

    // Send burst of messages as fast as possible
    let burst_start = Instant::now();
    for i in 0..burst_size {
        let message = format!("Burst message {}", i);
        if sender.send(Message::Text(message)).await.is_err() {
            break;
        }
    }
    let burst_duration = burst_start.elapsed();

    // Collect results
    let mut receiver_results = Vec::new();
    while let Some(result) = join_set.join_next().await {
        if let Ok(received) = result {
            receiver_results.push(received);
        }
    }

    let total_time = start_time.elapsed();
    let total_received_final = total_received.load(Ordering::Relaxed);
    let expected_total = burst_size * num_receivers;
    let fanout_efficiency = (total_received_final as f64 / expected_total as f64) * 100.0;
    let messages_per_second = total_received_final as f64 / total_time.as_secs_f64();
    let burst_send_rate = burst_size as f64 / burst_duration.as_secs_f64();

    println!("Burst fan-out test results:");
    println!("Burst size: {}, Receivers: {}", burst_size, num_receivers);
    println!("Expected total received: {}", expected_total);
    println!("Actual total received: {}", total_received_final);
    println!("Burst send time: {:?}", burst_duration);
    println!("Total time: {:?}", total_time);
    println!("Burst send rate: {:.2} msg/sec", burst_send_rate);
    println!("Fan-out efficiency: {:.1}%", fanout_efficiency);
    println!("Fan-out throughput: {:.2} msg/sec", messages_per_second);

    // Performance assertions for burst fan-out
    assert!(fanout_efficiency >= 70.0, 
            "Burst fan-out efficiency too low: {:.1}% < 70%", fanout_efficiency);
    
    assert!(messages_per_second >= 50000.0, 
            "Burst fan-out throughput too low: {:.2} msg/sec < 50k msg/sec", 
            messages_per_second);

    Ok(())
}