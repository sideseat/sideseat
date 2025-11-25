use clap::{Parser, Subcommand};
use sideseat::core::CliConfig;
use sideseat::core::constants::{APP_NAME_LOWER, ENV_LOG};

#[derive(Parser)]
#[command(name = "sideseat")]
#[command(version, about = "AI Development Toolkit", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Server host address [env: SIDESEAT_HOST] [default: 127.0.0.1]
    #[arg(long, short = 'H', global = true)]
    host: Option<String>,

    /// Server port [env: SIDESEAT_PORT] [default: 5001]
    #[arg(long, short = 'p', global = true)]
    port: Option<u16>,

    /// Disable authentication (for development)
    #[arg(long, global = true)]
    no_auth: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the server (default command)
    Start,
}

#[tokio::main]
async fn main() {
    // Load environment variables from .env file FIRST
    // This allows .env to set SIDESEAT_* env vars before config initialization
    dotenvy::dotenv().ok();

    // Build default log filter using app name
    let default_filter = format!("info,{}=info", APP_NAME_LOWER);

    // Initialize tracing subscriber with compact, colorful formatting
    // Uses SIDESEAT_LOG env var, falls back to RUST_LOG, then default
    tracing_subscriber::fmt()
        .with_target(false)
        .with_thread_ids(false)
        .with_level(true)
        .with_ansi(true)
        .compact()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env(ENV_LOG)
                .or_else(|_| tracing_subscriber::EnvFilter::try_from_default_env())
                .unwrap_or_else(|_| default_filter.into()),
        )
        .init();

    let cli = Cli::parse();

    // Build CLI config from parsed arguments
    let cli_config = CliConfig { host: cli.host, port: cli.port, no_auth: cli.no_auth };

    match cli.command {
        Some(Commands::Start) | None => {
            if let Err(e) = sideseat::run(cli_config).await {
                eprintln!("\n❌ Error: {}\n", e);
                std::process::exit(1);
            }
        }
    }
}
