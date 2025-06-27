use chat_server::run_chat_server;

#[tokio::main]
async fn main() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:8080").await.unwrap();
    println!("Chat server listening on 127.0.0.1:8080");
    run_chat_server(listener).await;
}
