use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter},
    net::TcpListener,
    sync::broadcast,
    time::{interval, Duration},
};

pub async fn run_chat_server(listener: TcpListener) {
    let (tx, _rx) = broadcast::channel(1000);

    loop {
        let (mut socket, addr) = listener.accept().await.unwrap();
        let tx = tx.clone();
        let mut rx = tx.subscribe();

        tokio::spawn(async move {
            let (reader, writer) = socket.split();
            let mut reader = BufReader::new(reader);
            let mut writer = BufWriter::with_capacity(8192, writer);
            let mut line = String::new();
            let mut message_batch = Vec::new();
            let mut flush_interval = interval(Duration::from_millis(1));

            loop {
                tokio::select! {
                    result = reader.read_line(&mut line) => {
                        if result.unwrap_or(0) == 0 {
                            break;
                        }
                        let _ = tx.send((line.clone(), addr));
                        line.clear();
                    }
                    result = rx.recv() => {
                        match result {
                            Ok((msg, other_addr)) => {
                                if addr != other_addr {
                                    message_batch.push(msg);
                                    if message_batch.len() >= 10 {
                                        for batched_msg in message_batch.drain(..) {
                                            let _ = writer.write_all(batched_msg.as_bytes()).await;
                                        }
                                        let _ = writer.flush().await;
                                    }
                                }
                            }
                            Err(broadcast::error::RecvError::Lagged(_)) => {
                                continue;
                            }
                            Err(broadcast::error::RecvError::Closed) => {
                                break;
                            }
                        }
                    }
                    _ = flush_interval.tick() => {
                        if !message_batch.is_empty() {
                            for batched_msg in message_batch.drain(..) {
                                let _ = writer.write_all(batched_msg.as_bytes()).await;
                            }
                            let _ = writer.flush().await;
                        }
                    }
                }
            }
        });
    }
}