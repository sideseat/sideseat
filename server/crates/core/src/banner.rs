//! Startup banner and URL display.

use std::path::Path;

use super::config::is_all_interfaces;
use super::constants::APP_NAME;
use crate::utils::terminal::terminal_link;

/// Values rendered in the startup banner.
pub struct StartupBanner<'a> {
    pub host: &'a str,
    pub port: u16,
    pub auth_enabled: bool,
    pub bootstrap_token: &'a str,
    pub grpc_enabled: bool,
    pub grpc_port: u16,
    pub data_dir: &'a Path,
    pub mcp_enabled: bool,
}

/// Print the startup banner and service URLs.
pub fn print_banner(banner: StartupBanner<'_>) {
    let StartupBanner {
        host,
        port,
        auth_enabled,
        bootstrap_token,
        grpc_enabled,
        grpc_port,
        data_dir,
        mcp_enabled,
    } = banner;

    let display_host = if is_all_interfaces(host) {
        "localhost"
    } else {
        host
    };

    println!();
    println!(
        "  \x1b[1m\x1b[36m{}\x1b[0m \x1b[90mv{}\x1b[0m",
        APP_NAME,
        env!("CARGO_PKG_VERSION")
    );
    println!();

    let local_url = if auth_enabled {
        format!(
            "http://{}:{}/ui?token={}",
            display_host, port, bootstrap_token
        )
    } else {
        format!("http://{}:{}", display_host, port)
    };
    // "OpenTelemetry HTTP:" is the longest label.
    const W: usize = 21;

    println!(
        "  \x1b[32m➜\x1b[0m  \x1b[1m{:<W$}\x1b[0m {}",
        "Web UI:",
        terminal_link(&local_url)
    );

    println!(
        "  \x1b[33m➜\x1b[0m  \x1b[1m{:<W$}\x1b[0m http://{}:{}/otel/default",
        "OpenTelemetry HTTP:", display_host, port
    );
    if grpc_enabled {
        println!(
            "  \x1b[33m➜\x1b[0m  \x1b[1m{:<W$}\x1b[0m {}:{} \x1b[90m(x-sideseat-project-id header)\x1b[0m",
            "OpenTelemetry gRPC:", host, grpc_port
        );
    }

    if mcp_enabled {
        println!(
            "  \x1b[35m➜\x1b[0m  \x1b[1m{:<W$}\x1b[0m http://{}:{}/api/v1/projects/default/mcp",
            "MCP:", display_host, port
        );
    }

    if host == "127.0.0.1" || host == "localhost" {
        println!(
            "  \x1b[90m➜  {:<W$} use --host 0.0.0.0 to expose\x1b[0m",
            "Network:"
        );
    } else if is_all_interfaces(host) {
        if let Ok(interfaces) = local_ip_address::list_afinet_netifas() {
            for (_, ip) in interfaces
                .iter()
                .filter(|(_, ip)| ip.is_ipv4() && !ip.is_loopback())
            {
                let network_url = format!("http://{}:{}", ip, port);
                println!(
                    "  \x1b[32m➜\x1b[0m  \x1b[1m{:<W$}\x1b[0m {}",
                    "Network:",
                    terminal_link(&network_url)
                );
            }
        }
    } else {
        let network_url = format!("http://{}:{}", host, port);
        println!(
            "  \x1b[32m➜\x1b[0m  \x1b[1m{:<W$}\x1b[0m {}",
            "Network:",
            terminal_link(&network_url)
        );
    }
    println!("  \x1b[90m➜  {:<W$} {}\x1b[0m", "Data:", data_dir.display());

    println!();
}

/// Print an update notification after the banner.
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
