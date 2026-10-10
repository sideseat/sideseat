//! Every documented route with path parameters accepts its own: a request with each parameter filled in reaches
//! its handler and never fails as a server error.
//!
//! An extractor that reads fewer path parameters than its route carries rejects every request at run time - the
//! metric detail read answered 500 to every caller, because nothing asked it a question. Here the server binary
//! is started over an empty store, its OpenAPI document lists the routes, and each is asked once, with the
//! default project and organisation for their parameters and a placeholder for the rest: a missing resource is a
//! 404 or a refusal, never a 500.

use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Three free ports in a row: the server listens on its API, UI and gRPC ports.
fn free_ports() -> u16 {
    for _ in 0..50 {
        let base = TcpListener::bind("127.0.0.1:0")
            .and_then(|probe| probe.local_addr())
            .map(|address| address.port())
            .expect("a free port");
        if base > 65_000 {
            continue;
        }
        let held: Vec<_> = (base..base + 3)
            .map(|port| TcpListener::bind(("127.0.0.1", port)))
            .collect();
        if held.iter().all(Result::is_ok) {
            return base;
        }
    }
    panic!("no three free ports in a row");
}

struct Server {
    child: Child,
    base: String,
    _data: tempfile::TempDir,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

async fn start() -> Server {
    let data = tempfile::TempDir::new().expect("data dir");
    let port = free_ports();
    let child = Command::new(env!("CARGO_BIN_EXE_sideseat"))
        .arg("--no-auth")
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("HOME", data.path())
        .env("SIDESEAT_DATA_DIR", data.path())
        .env("SIDESEAT_SECRETS_BACKEND", "file")
        .env("SIDESEAT_PORT", port.to_string())
        .env("SIDESEAT_UI_PORT", (port + 1).to_string())
        .env("SIDESEAT_OTEL_GRPC_PORT", (port + 2).to_string())
        .env("SIDESEAT_RATE_LIMIT_ENABLED", "false")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("the server starts");
    let mut server = Server {
        child,
        base: format!("http://127.0.0.1:{port}"),
        _data: data,
    };
    let client = reqwest::Client::new();
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        if let Ok(response) = client
            .get(format!("{}/api/v1/health", server.base))
            .send()
            .await
            && response.status().is_success()
        {
            break;
        }
        if let Ok(Some(status)) = server.child.try_wait() {
            panic!("the server exited before it was healthy: {status}");
        }
        assert!(
            Instant::now() < deadline,
            "the server did not become healthy"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    // A listener left by something else would answer the health check too; this one must still be running.
    assert!(server.child.try_wait().expect("status").is_none());
    server
}

/// `path` with each `{parameter}` filled: the default project and organisation, a placeholder otherwise.
fn filled(path: &str) -> String {
    let mut out = String::new();
    let mut rest = path;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let close = rest[open..].find('}').expect("a closing brace") + open;
        out.push_str(match &rest[open + 1..close] {
            "project_id" | "org_id" => "default",
            _ => "route-walk",
        });
        rest = &rest[close + 1..];
    }
    out.push_str(rest);
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_route_accepts_its_own_path_parameters() {
    let server = start().await;
    let client = reqwest::Client::new();
    let document: serde_json::Value = client
        .get(format!("{}/api/openapi.json", server.base))
        .send()
        .await
        .expect("the OpenAPI document")
        .json()
        .await
        .expect("JSON");
    let mut routes: Vec<(String, String)> = document["paths"]
        .as_object()
        .expect("paths")
        .iter()
        .filter(|(path, _)| path.contains('{'))
        .flat_map(|(path, item)| {
            item.as_object()
                .expect("a path item")
                .keys()
                .filter(|method| {
                    ["get", "post", "put", "patch", "delete"].contains(&method.as_str())
                })
                .map(move |method| (method.clone(), path.clone()))
        })
        .collect();
    // Deletions last, the deepest first, so no deletion removes what a later request needs.
    routes.sort_by_key(|(method, path)| {
        (
            method == "delete",
            std::cmp::Reverse(path.matches('/').count()),
            path.clone(),
        )
    });
    assert!(
        routes.len() > 30,
        "only {} routes with path parameters",
        routes.len()
    );
    let mut failures = Vec::new();
    for (method, path) in &routes {
        let url = format!("{}{}", server.base, filled(path));
        let request = client
            .request(method.to_uppercase().parse().expect("a method"), &url)
            .header("content-type", "application/json")
            .body(if method == "get" { "" } else { "{}" });
        let response = request.send().await.expect("an answer");
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        if status.is_server_error() || body.contains("path arguments") {
            failures.push(format!(
                "{} {path}: {status} {}",
                method.to_uppercase(),
                body.chars().take(160).collect::<String>()
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "routes that fail on their own path parameters:\n  {}",
        failures.join("\n  ")
    );
}
