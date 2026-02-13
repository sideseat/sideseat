//! Startup banner and URL display

use super::constants::APP_NAME;
use crate::utils::terminal::terminal_link;

/// Print the startup banner with URLs
#[allow(clippy::too_many_arguments)]
pub fn print_banner(
    host: &str,
    port: u16,
    auth_enabled: bool,
    bootstrap_token: &str,
    grpc_enabled: bool,
    grpc_port: u16,
    data_dir: &str,
    mcp_enabled: bool,
) {
    println!();
    println!(
        "  \x1b[1m\x1b[36m{}\x1b[0m \x1b[90mv{}\x1b[0m",
        APP_NAME,
        env!("CARGO_PKG_VERSION")
    );
    println!();

    // Show local URL (with token if auth is enabled)
    let local_url = if auth_enabled {
        format!("http://{}:{}/ui?token={}", host, port, bootstrap_token)
    } else {
        format!("http://{}:{}", host, port)
    };
    println!(
        "  \x1b[32m➜\x1b[0m  \x1b[1mWeb UI:\x1b[0m   {}",
        terminal_link(&local_url)
    );

    // Show OTLP endpoints (using "default" project)
    println!(
        "  \x1b[33m➜\x1b[0m  \x1b[1mOpenTelemetry HTTP:\x1b[0m http://{}:{}/otel/default",
        host, port
    );
    if grpc_enabled {
        println!(
            "  \x1b[33m➜\x1b[0m  \x1b[1mOpenTelemetry gRPC:\x1b[0m {}:{} \x1b[90m(x-sideseat-project-id header)\x1b[0m",
            host, grpc_port
        );
    }

    if mcp_enabled {
        println!(
            "  \x1b[35m➜\x1b[0m  \x1b[1mMCP:\x1b[0m          http://{}:{}/api/v1/projects/default/mcp",
            host, port
        );
    }

    // Show network info based on bind address
    if host == "127.0.0.1" || host == "localhost" {
        println!("  \x1b[90m➜  Network:  use --host 0.0.0.0 to expose\x1b[0m");
    } else {
        // Show actual network addresses when exposed
        if let Ok(interfaces) = local_ip_address::list_afinet_netifas() {
            for (_, ip) in interfaces
                .iter()
                .filter(|(_, ip)| ip.is_ipv4() && !ip.is_loopback())
            {
                let network_url = format!("http://{}:{}", ip, port);
                println!(
                    "  \x1b[32m➜\x1b[0m  \x1b[1mNetwork:\x1b[0m  {}",
                    terminal_link(&network_url)
                );
            }
        }
    }
    println!("  \x1b[90m➜  Data: {}\x1b[0m", data_dir);

    println!();
}

/// Print update notification after banner
pub fn print_update_available(current: &str, new_version: &str) {
    let npm_url = "https://www.npmjs.com/package/sideseat";
    println!(
        "  \x1b[33m[Update available]\x1b[0m v{} -> v{}",
        current, new_version
    );
    println!("  Run: \x1b[36mnpm install -g sideseat\x1b[0m");
    println!("  {}", terminal_link(npm_url));
    println!();
}
