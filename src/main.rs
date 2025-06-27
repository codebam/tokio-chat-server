use chat_server::run_chat_server;
use tokio::runtime::Builder;

fn main() {
    let num_cores = num_cpus::get();
    println!("Detected {} CPU cores, optimizing for Ryzen 7 5700X3D", num_cores);
    
    let rt = Builder::new_multi_thread()
        .worker_threads(num_cores)
        .thread_stack_size(8 * 1024 * 1024)
        .enable_all()
        .thread_name("chat-worker")
        .build()
        .expect("Failed to create Tokio runtime");
    
    rt.block_on(async {
        let listener = tokio::net::TcpListener::bind("0.0.0.0:8080").await.unwrap();
        println!("High-performance chat server listening on 0.0.0.0:8080");
        run_chat_server(listener).await;
    });
}
