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

/// Three free ports in a row: the server listens on its API, UI and gRPC ports. Chosen from a range the system
/// does not hand out for outgoing connections, so tests running at once do not race the allocator for them.
fn free_ports() -> u16 {
    // Each call starts its search somewhere else, so two servers starting at once never pick one base.
    static CALLS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seed = u64::from(std::process::id())
        + 1_000 * CALLS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    for attempt in 0..200_u64 {
        let base = 20_000 + ((seed.wrapping_mul(7_919) + attempt * 104_729) % 20_000) as u16;
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
    // Bounded, so a route that never answers fails the walk rather than hanging it.
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .expect("a client");
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
        // A stream answers for as long as the client listens: its status and headers are the answer here.
        let streams = response
            .headers()
            .get("content-type")
            .is_some_and(|value| value.as_bytes().starts_with(b"text/event-stream"));
        let body = if streams {
            String::new()
        } else {
            response.text().await.unwrap_or_default()
        };
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

// ---------------------------------------------------------------------------------------------------------------
// Every registered route is documented.
// ---------------------------------------------------------------------------------------------------------------

/// The routes that serve no API of their own, each with why the OpenAPI document leaves it out. Every other route
/// the router registers must be in the document, method by method.
const UNDOCUMENTED: [(&str, &str); 4] = [
    ("/", "redirects a browser to the UI"),
    ("/api/openapi.json", "the OpenAPI document itself"),
    ("/api/docs", "the Swagger UI over the document"),
    ("/api/docs/", "the Swagger UI over the document"),
];

/// Nests that register no route of their own, only a fallback, and why.
const FALLBACK_NESTS: [(&str, &str); 2] = [
    ("/ui", "the embedded web UI's assets"),
    (
        "/api/v1/projects/{project_id}/mcp",
        "the MCP server, a JSON-RPC transport the MCP protocol describes",
    ),
];

type Trees = Vec<proc_macro2::TokenTree>;

fn api_tokens(relative: &str) -> Option<Trees> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("crates/api/src")
        .join(relative);
    let text = std::fs::read_to_string(&path).ok()?;
    let stream: proc_macro2::TokenStream = text
        .parse()
        .unwrap_or_else(|error| panic!("{relative} does not lex: {error:?}"));
    Some(stream.into_iter().collect())
}

fn trees_of(group: &proc_macro2::Group) -> Trees {
    group.stream().into_iter().collect()
}

fn ident_is(token: Option<&proc_macro2::TokenTree>, word: &str) -> bool {
    matches!(token, Some(proc_macro2::TokenTree::Ident(ident)) if ident == word)
}

fn punct_is(token: Option<&proc_macro2::TokenTree>, c: char) -> bool {
    matches!(token, Some(proc_macro2::TokenTree::Punct(punct)) if punct.as_char() == c)
}

/// A string literal's text, if `token` is one.
fn literal_text(token: Option<&proc_macro2::TokenTree>) -> Option<String> {
    let proc_macro2::TokenTree::Literal(literal) = token? else {
        return None;
    };
    let text = literal.to_string();
    text.strip_prefix('"')?
        .strip_suffix('"')
        .map(str::to_string)
}

/// The body of `fn name` in `tokens`, at any depth.
fn function_body(tokens: &[proc_macro2::TokenTree], name: &str) -> Option<Trees> {
    for (index, token) in tokens.iter().enumerate() {
        if ident_is(Some(token), "fn") && ident_is(tokens.get(index + 1), name) {
            return tokens[index..].iter().find_map(|token| match token {
                proc_macro2::TokenTree::Group(group)
                    if group.delimiter() == proc_macro2::Delimiter::Brace =>
                {
                    Some(trees_of(group))
                }
                _ => None,
            });
        }
        if let proc_macro2::TokenTree::Group(group) = token
            && let Some(body) = function_body(&trees_of(group), name)
        {
            return Some(body);
        }
    }
    None
}

const METHODS: [&str; 6] = ["get", "post", "put", "patch", "delete", "head"];

/// Every `.route("path", get(..).post(..))` in `tokens`, at any depth, as its path and its methods.
fn route_calls(tokens: &[proc_macro2::TokenTree], into: &mut Vec<(String, Vec<String>)>) {
    for (index, token) in tokens.iter().enumerate() {
        if punct_is(Some(token), '.')
            && ident_is(tokens.get(index + 1), "route")
            && let Some(proc_macro2::TokenTree::Group(arguments)) = tokens.get(index + 2)
        {
            let arguments = trees_of(arguments);
            if let Some(path) = literal_text(arguments.first()) {
                let methods = arguments
                    .iter()
                    .enumerate()
                    .filter(|(at, token)| {
                        METHODS.iter().any(|method| ident_is(Some(token), method))
                            && matches!(
                                arguments.get(at + 1),
                                Some(proc_macro2::TokenTree::Group(_))
                            )
                            && (*at == 0 || !punct_is(arguments.get(at - 1), ':'))
                    })
                    .map(|(_, token)| token.to_string())
                    .collect();
                into.push((path, methods));
            }
        }
        if let proc_macro2::TokenTree::Group(group) = token {
            route_calls(&trees_of(group), into);
        }
    }
}

/// The source file of a module the server names by `segments`: `super::mcp` from the crate root, and a bare name
/// (`otel`, `otel::files`) from the route modules the server imports (`use super::routes::{..}`).
fn module_file(segments: &[String]) -> String {
    let from_root = segments
        .first()
        .is_some_and(|first| first == "super" || first == "crate");
    let segments: Vec<&str> = segments
        .iter()
        .map(String::as_str)
        .filter(|segment| !matches!(*segment, "super" | "crate" | "self"))
        .collect();
    let joined = if from_root {
        segments.join("/")
    } else {
        format!("routes/{}", segments.join("/"))
    };
    [format!("{joined}/mod.rs"), format!("{joined}.rs")]
        .into_iter()
        .find(|candidate| api_tokens(candidate).is_some())
        .unwrap_or_else(|| panic!("no source for module {joined}"))
}

/// The routes the server registers, as (method, full path): its own, and each nested router's under its prefix.
fn registered_routes() -> Vec<(String, String)> {
    let server = api_tokens("server.rs").expect("server.rs");
    let start = function_body(&server, "start").expect("ApiServer::start");
    // Each router the start function builds, by its binding: the module and function that make it.
    let mut builders: std::collections::BTreeMap<String, (String, String)> = Default::default();
    let mut index = 0;
    while index < start.len() {
        if ident_is(start.get(index), "let") {
            let end = (index..start.len())
                .find(|&at| punct_is(start.get(at), ';'))
                .unwrap_or(start.len());
            let statement = &start[index..end];
            let names: Vec<String> = match statement.get(1) {
                Some(proc_macro2::TokenTree::Ident(name)) => vec![name.to_string()],
                Some(proc_macro2::TokenTree::Group(pattern)) => trees_of(pattern)
                    .iter()
                    .filter_map(|token| match token {
                        proc_macro2::TokenTree::Ident(name) => Some(name.to_string()),
                        _ => None,
                    })
                    .collect(),
                _ => Vec::new(),
            };
            let call = (0..statement.len()).find(|&at| {
                matches!(statement.get(at), Some(proc_macro2::TokenTree::Ident(name))
                    if name.to_string().ends_with("routes"))
                    && matches!(statement.get(at + 1), Some(proc_macro2::TokenTree::Group(group))
                        if group.delimiter() == proc_macro2::Delimiter::Parenthesis)
            });
            if let (Some(name), Some(at)) = (names.first(), call)
                && !builders.contains_key(name)
            {
                let mut segments = Vec::new();
                let mut back = at;
                while back >= 3
                    && punct_is(statement.get(back - 1), ':')
                    && punct_is(statement.get(back - 2), ':')
                {
                    segments.insert(0, statement[back - 3].to_string());
                    back -= 3;
                }
                let file = if segments.is_empty() {
                    "server.rs".to_string()
                } else {
                    module_file(&segments)
                };
                builders.insert(name.clone(), (file, statement[at].to_string()));
            }
            index = end;
        }
        index += 1;
    }
    let mut routes = Vec::new();
    // The server's own routes: those its start function registers outside the routers it nests.
    let mut direct = Vec::new();
    route_calls(
        &start
            .iter()
            .filter(|token| {
                !matches!(token, proc_macro2::TokenTree::Group(group)
                if group.delimiter() == proc_macro2::Delimiter::Brace)
            })
            .cloned()
            .collect::<Vec<_>>(),
        &mut direct,
    );
    for (path, methods) in direct {
        routes.extend(methods.into_iter().map(|method| (method, path.clone())));
    }
    let mut nests = Vec::new();
    collect_nests(&start, &mut nests);
    for (prefix, binding) in nests {
        if FALLBACK_NESTS.iter().any(|(nest, _)| *nest == prefix) {
            continue;
        }
        let (file, function) = builders.get(&binding).unwrap_or_else(|| {
            panic!("the router nested at {prefix} ({binding}) is built nowhere")
        });
        let tokens = api_tokens(file).expect("the router's source");
        let body = function_body(&tokens, function)
            .unwrap_or_else(|| panic!("no fn {function} in {file}"));
        let mut found = Vec::new();
        route_calls(&body, &mut found);
        assert!(
            !found.is_empty(),
            "the router nested at {prefix} registers no route"
        );
        for (path, methods) in found {
            let full = if path == "/" {
                prefix.clone()
            } else {
                format!("{prefix}{path}")
            };
            routes.extend(methods.into_iter().map(|method| (method, full.clone())));
        }
    }
    routes.sort();
    routes.dedup();
    routes
}

/// Each `.nest("prefix", router)` in `tokens`, at any depth, with the binding the router is.
fn collect_nests(tokens: &[proc_macro2::TokenTree], into: &mut Vec<(String, String)>) {
    for (index, token) in tokens.iter().enumerate() {
        if punct_is(Some(token), '.')
            && ident_is(tokens.get(index + 1), "nest")
            && let Some(proc_macro2::TokenTree::Group(arguments)) = tokens.get(index + 2)
        {
            let arguments = trees_of(arguments);
            if let Some(prefix) = literal_text(arguments.first()) {
                let binding = arguments
                    .iter()
                    .skip(2)
                    .find_map(|token| match token {
                        proc_macro2::TokenTree::Ident(name) => Some(name.to_string()),
                        _ => None,
                    })
                    .unwrap_or_default();
                into.push((prefix, binding));
            }
        }
        if let proc_macro2::TokenTree::Group(group) = token {
            collect_nests(&trees_of(group), into);
        }
    }
}

/// The registered routes the document does not list, method by method, but the ones [`UNDOCUMENTED`] names.
fn undocumented(
    registered: &[(String, String)],
    documented: &std::collections::BTreeSet<(String, String)>,
) -> Vec<String> {
    registered
        .iter()
        .filter(|(_, path)| !UNDOCUMENTED.iter().any(|(exempt, _)| exempt == path))
        .filter(|route| !documented.contains(*route))
        .map(|(method, path)| format!("{} {path}", method.to_uppercase()))
        .collect()
}

fn documented_routes(document: &serde_json::Value) -> std::collections::BTreeSet<(String, String)> {
    document["paths"]
        .as_object()
        .expect("paths")
        .iter()
        .flat_map(|(path, item)| {
            item.as_object()
                .expect("a path item")
                .keys()
                .map(move |method| (method.clone(), path.clone()))
        })
        .collect()
}

/// Every route the router registers is in the OpenAPI document, method by method, but the few that serve no API
/// ([`UNDOCUMENTED`]): the route walk above asks only what the document lists, so an undocumented route would
/// otherwise go unasked.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_registered_route_is_documented() {
    let registered = registered_routes();
    assert!(
        registered.len() > 60,
        "only {} routes registered",
        registered.len()
    );
    let server = start().await;
    let document: serde_json::Value = reqwest::Client::new()
        .get(format!("{}/api/openapi.json", server.base))
        .send()
        .await
        .expect("the OpenAPI document")
        .json()
        .await
        .expect("JSON");
    let missing = undocumented(&registered, &documented_routes(&document));
    assert!(
        missing.is_empty(),
        "routes the OpenAPI document does not list:\n  {}",
        missing.join("\n  ")
    );
}

/// The check reads each route under the prefix its router is nested at: the OTLP collector's `/traces`, nested at
/// `/otel/{project_id}/v1`, is not the query API's `/traces`, and a document that lists only the latter leaves the
/// former out.
#[test]
fn the_route_check_reads_each_route_under_its_own_prefix() {
    let registered = registered_routes();
    let ingest = (
        "post".to_string(),
        "/otel/{project_id}/v1/traces".to_string(),
    );
    let query = (
        "get".to_string(),
        "/api/v1/project/{project_id}/otel/traces".to_string(),
    );
    assert!(registered.contains(&ingest), "{registered:?}");
    assert!(registered.contains(&query), "{registered:?}");
    let documented = std::collections::BTreeSet::from([query.clone()]);
    assert_eq!(
        undocumented(&[ingest, query], &documented),
        vec!["POST /otel/{project_id}/v1/traces".to_string()]
    );
}
