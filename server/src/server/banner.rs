//! Startup banner and URL display

use crate::core::constants::APP_NAME;
use crate::core::utils::terminal_link;

/// Print the startup banner with URLs
pub fn print_banner(
    host: &str,
    port: u16,
    grpc_port: u16,
    auth_enabled: bool,
    bootstrap_token: &str,
    otel_enabled: bool,
    grpc_enabled: bool,
) {
    println!();
    println!("  \x1b[1m\x1b[36m{}\x1b[0m \x1b[90mv{}\x1b[0m", APP_NAME, env!("CARGO_PKG_VERSION"));
    println!();

    // Show local URL (with token if auth is enabled)
    let local_url = if auth_enabled {
        format!("http://{}:{}/ui?token={}", host, port, bootstrap_token)
    } else {
        format!("http://{}:{}", host, port)
    };
    println!("  \x1b[32m➜\x1b[0m  \x1b[1mWeb UI:\x1b[0m   {}", terminal_link(&local_url));

    // Show OTel endpoints if enabled (after Web UI, before Network)
    if otel_enabled {
        println!(
            "  \x1b[33m➜\x1b[0m  \x1b[1mOpen Telemetry HTTP:\x1b[0m http://{}:{}/otel/v1/traces",
            host, port
        );
        if grpc_enabled {
            println!(
                "  \x1b[33m➜\x1b[0m  \x1b[1mOpen Telemetry gRPC:\x1b[0m {}:{}",
                host, grpc_port
            );
        }
    }

    // Show network info based on bind address
    if host == "127.0.0.1" || host == "localhost" {
        println!("  \x1b[90m➜  Network:  use --host 0.0.0.0 to expose\x1b[0m");
    } else {
        // Show actual network addresses when exposed
        if let Ok(interfaces) = local_ip_address::list_afinet_netifas() {
            for (_, ip) in interfaces.iter().filter(|(_, ip)| ip.is_ipv4() && !ip.is_loopback()) {
                let network_url = format!("http://{}:{}", ip, port);
                println!(
                    "  \x1b[32m➜\x1b[0m  \x1b[1mNetwork:\x1b[0m  {}",
                    terminal_link(&network_url)
                );
            }
        }
    }
}
