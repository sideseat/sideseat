use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "sideseat")]
#[command(about = "AI Development Toolkit", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    Start,
    // TODO: Add proxy, otel, mcp, a2a, prompts subcommands
}

#[tokio::main]
async fn main() {
    // Load environment variables from .env file
    dotenvy::dotenv().ok();

    // Initialize tracing subscriber for logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,sideseat=debug".into()),
        )
        .init();

    tracing::info!("Starting SideSeat...");

    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Start) | None => {
            if let Err(e) = sideseat::run().await {
                tracing::error!("Application error: {}", e);
                std::process::exit(1);
            }
        }
    }
}
