use sideseat_server::app::CoreApp;

#[tokio::main]
async fn main() {
    if let Err(e) = CoreApp::run().await {
        eprintln!("\nError: {}\n", e);
        std::process::exit(1);
    }
}
