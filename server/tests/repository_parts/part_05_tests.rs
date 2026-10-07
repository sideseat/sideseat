#[test]
fn migrated_clock_consumers_cannot_read_the_system_clock() {
    let repo = repo_root();
    for file in [
        "server/crates/core/src/utils/debug.rs",
        "server/crates/api/src/auth/api_key.rs",
        "server/crates/api/src/auth/jwt.rs",
        "server/crates/api/src/auth/manager.rs",
        "server/crates/api/src/routes/otel/messages.rs",
        "server/crates/api/src/routes/otlp_collector/mod.rs",
        "server/crates/api/src/routes/otlp_collector/traces.rs",
        "server/crates/api/src/routes/otlp_collector/metrics.rs",
        "server/crates/api/src/routes/otlp_collector/logs.rs",
        "server/crates/api/src/routes/otlp_collector/grpc.rs",
        "server/crates/api/src/routes/otel/stats.rs",
        "server/crates/domain/src/pricing/mod.rs",
        "server/crates/domain/src/rate_limit.rs",
    ] {
        let source = std::fs::read_to_string(repo.join(file))
            .unwrap_or_else(|e| panic!("{file} is readable: {e}"));
        assert!(
            !source.contains("Utc::now"),
            "{file} bypasses the injected Clock"
        );
    }

    let secrets = repo.join("server/crates/adapter-secrets/src");
    let mut stack = vec![secrets];
    let mut secret_sources = 0usize;
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("secrets directory is readable") {
            let path = entry.expect("secrets entry is readable").path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().and_then(|extension| extension.to_str()) != Some("rs") {
                continue;
            }
            secret_sources += 1;
            let source = std::fs::read_to_string(&path).expect("secret source is readable");
            assert!(
                !source.contains("Utc::now"),
                "{} bypasses the injected Clock",
                path.strip_prefix(repo).unwrap_or(&path).display()
            );
        }
    }
    assert!(
        secret_sources >= 10,
        "the secrets clock gate scanned suspiciously few Rust sources"
    );

    let mut transactional_sources = 0usize;
    for relative in [
        "server/crates/adapter-sqlite/src/repositories",
        "server/crates/adapter-postgres/src/repositories",
    ] {
        let mut stack = vec![repo.join(relative)];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).expect("repository directory is readable") {
                let path = entry.expect("repository entry is readable").path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().and_then(|extension| extension.to_str()) != Some("rs") {
                    continue;
                }
                transactional_sources += 1;
                let source =
                    std::fs::read_to_string(&path).expect("transactional source is readable");
                assert!(
                    !source.contains("Utc::now"),
                    "{} bypasses the injected Clock",
                    path.strip_prefix(repo).unwrap_or(&path).display()
                );
            }
        }
    }
    for relative in [
        "server/crates/adapter-sqlite/src/migrations.rs",
        "server/crates/adapter-postgres/src/migrations.rs",
    ] {
        transactional_sources += 1;
        let source = std::fs::read_to_string(repo.join(relative))
            .unwrap_or_else(|e| panic!("{relative} is readable: {e}"));
        assert!(
            !source.contains("Utc::now"),
            "{relative} bypasses the injected Clock"
        );
    }
    assert!(
        transactional_sources >= 24,
        "the transactional clock gate scanned suspiciously few Rust sources"
    );

    for relative in [
        "server/crates/adapter-duckdb/src/lib.rs",
        "server/crates/adapter-duckdb/src/migrations.rs",
        "server/crates/adapter-duckdb/src/retention.rs",
        "server/crates/adapter-duckdb/src/repository_impl.rs",
        "server/crates/adapter-duckdb/src/repositories/metric.rs",
        "server/crates/adapter-duckdb/src/repositories/span.rs",
        "server/crates/adapter-duckdb/src/repositories/stats.rs",
        "server/crates/adapter-clickhouse/src/lib.rs",
        "server/crates/adapter-clickhouse/src/repository_impl.rs",
        "server/crates/adapter-clickhouse/src/repositories/metric.rs",
        "server/crates/adapter-clickhouse/src/repositories/span.rs",
        "server/crates/adapter-clickhouse/src/repositories/stats.rs",
    ] {
        let source = std::fs::read_to_string(repo.join(relative))
            .unwrap_or_else(|e| panic!("{relative} is readable: {e}"));
        // Repository unit fixtures deliberately use real-looking timestamps, but the production module
        // must not. All listed files keep their unit module behind this exact marker.
        let production = source
            .split("\n#[cfg(test)]\nmod tests")
            .next()
            .expect("split always yields the production prefix");
        assert!(
            !production.contains("Utc::now"),
            "{relative} bypasses the injected Clock"
        );
    }

    let app = std::fs::read_to_string(repo.join("server/src/app.rs")).expect("app is readable");
    assert_eq!(
        app.matches("Arc::new(SystemClock)").count(),
        1,
        "the composition root must create exactly one production wall clock"
    );
    assert!(
        app.matches("Arc::clone(&clock)").count() >= 2,
        "the composition root stopped sharing its clock with consumers"
    );

    let server = std::fs::read_to_string(repo.join("server/crates/api/src/server.rs"))
        .expect("server is readable");
    assert!(
        server.matches("clock: app.clock.clone()").count() >= 10,
        "one or more HTTP auth/router states no longer receive the shared clock"
    );
}

#[test]
fn every_registered_signal_uses_the_shared_lifecycle_on_both_transports() {
    let repo = repo_root();
    let routes =
        std::fs::read_to_string(repo.join("server/crates/api/src/routes/otlp_collector/mod.rs"))
            .expect("OTLP route registry is readable");
    let grpc =
        std::fs::read_to_string(repo.join("server/crates/api/src/routes/otlp_collector/grpc.rs"))
            .expect("OTLP gRPC services are readable");

    for signal in sideseat_ingestion::signals::REGISTERED_SIGNAL_NAMES {
        let http_path = repo.join(format!(
            "server/crates/api/src/routes/otlp_collector/{signal}.rs"
        ));
        let http = std::fs::read_to_string(&http_path)
            .unwrap_or_else(|error| panic!("{} is readable: {error}", http_path.display()));
        assert!(
            http.contains("export_signal("),
            "registered signal {signal} bypasses the shared lifecycle on HTTP"
        );
        assert!(
            routes.contains(&format!(".route(\"/{signal}\", post({signal}::export))")),
            "registered signal {signal} is missing from the HTTP route table"
        );

        let service = match *signal {
            "traces" => "OtlpTraceService",
            "metrics" => "OtlpMetricsService",
            "logs" => "OtlpLogsService",
            other => {
                panic!("registered signal {other} has no gRPC service-name mapping in the gate")
            }
        };
        // The handler trait, not tonic's generated one: gRPC export goes through the raw-bytes codec, which is
        // what makes the stored record the producer's frame rather than a re-encoding of the decoded message.
        let export_start = grpc
            .find(&format!("impl RawExportHandler for {service}"))
            .unwrap_or_else(|| {
                panic!("registered signal {signal} has no raw-codec gRPC handler for {service}")
            });
        let export_source = &grpc[export_start..];
        let end = export_source
            .find("\n/// gRPC ")
            .unwrap_or(export_source.len());
        let export_source = &export_source[..end];
        assert!(
            export_source.contains("export_signal("),
            "registered signal {signal} bypasses the shared lifecycle on gRPC"
        );
        assert!(
            export_source.contains("Received { message: req, raw }"),
            "registered signal {signal} does not store the frame the exporter sent"
        );
    }
}

#[test]
fn public_vertex_ai_examples_use_the_current_google_genai_client() {
    let repo = repo_root();
    let examples = [
        "docs/src/components/marketing/CodeExample.astro",
        "docs/src/content/docs/docs/index.mdx",
        "docs/src/content/docs/docs/integrations/providers/google-gemini.mdx",
        "docs/src/content/docs/docs/integrations/providers/vertex-ai.mdx",
        "examples/python/vertex-ai/models.py",
        "sdk/python/README.md",
        "server/crates/api/src/mcp/tools.rs",
        "web/src/pages/configuration/telemetry-frameworks.ts",
    ];
    let retired = [
        "from vertexai",
        "import vertexai",
        "vertexai.generative_models",
        "VertexAIInstrumentor",
        "google-cloud-aiplatform",
        "opentelemetry-instrumentation-vertexai",
        "genai.Client(vertexai=True",
        "\"vertexai\": True",
    ];

    for relative in examples {
        let source = std::fs::read_to_string(repo.join(relative))
            .unwrap_or_else(|error| panic!("{relative} is readable: {error}"));
        assert!(
            source.contains("enterprise=True") || source.contains("\"enterprise\": True"),
            "{relative} does not select the current Google Gen AI Enterprise client"
        );
        for obsolete in retired {
            assert!(
                !source.contains(obsolete),
                "{relative} publishes retired Vertex AI setup `{obsolete}`"
            );
        }
    }
}

#[test]
fn google_genai_fixture_version_matches_its_example_lock() {
    let repo = repo_root();
    let lock = std::fs::read_to_string(repo.join("examples/python/google-genai/uv.lock"))
        .expect("Google GenAI example lock is readable");
    let package = lock
        .split("[[package]]")
        .find(|package| {
            package
                .lines()
                .any(|line| line == "name = \"google-genai\"")
        })
        .expect("Google GenAI package is locked");
    let version = package
        .lines()
        .find_map(|line| line.strip_prefix("version = \""))
        .and_then(|version| version.strip_suffix('"'))
        .expect("Google GenAI lock entry has a version");

    let manifest =
        std::fs::read_to_string(repo.join("examples/python/google-genai/pyproject.toml"))
            .expect("Google GenAI example manifest is readable");
    assert!(
        manifest.contains(&format!("\"google-genai>={version}\"")),
        "Google GenAI example minimum does not match locked version {version}"
    );

    for relative in [
        "docs/src/content/docs/docs/integrations/providers/google-gemini.mdx",
        "docs/src/content/docs/docs/reference/production-readiness.mdx",
        "server/tests/fixtures/messages/README.md",
    ] {
        let source = std::fs::read_to_string(repo.join(relative))
            .unwrap_or_else(|error| panic!("{relative} is readable: {error}"));
        assert!(
            source.contains(&format!("Google GenAI {version}")),
            "{relative} does not describe the locked Google GenAI {version} fixture"
        );
    }
}

/// `scripts/` is grouped by purpose, and `scripts/README.md` names every group. A script dropped loose at the top,
/// or a group the README does not describe, is how the directory became an undifferentiated pile before.
#[test]
fn scripts_are_grouped_by_purpose() {
    let repo = repo_root();
    let listing = Command::new("git")
        .args(["ls-files", "scripts"])
        .current_dir(repo)
        .output()
        .expect("git is available in a git checkout");
    let files = String::from_utf8_lossy(&listing.stdout).into_owned();
    let loose: Vec<&str> = files
        .lines()
        .filter(|f| f.matches('/').count() == 1 && *f != "scripts/README.md")
        .collect();
    assert!(
        loose.is_empty(),
        "scripts/ holds only purpose directories and its README; move these into one: {loose:?}"
    );
    let readme =
        std::fs::read_to_string(repo.join("scripts/README.md")).expect("scripts/README.md");
    let groups: BTreeSet<&str> = files
        .lines()
        .filter_map(|f| {
            f.strip_prefix("scripts/")?
                .split_once('/')
                .map(|(group, _)| group)
        })
        .collect();
    assert!(groups.len() >= 5, "found only {groups:?} under scripts/");
    for group in groups {
        assert!(
            readme.contains(&format!("`{group}/`")),
            "scripts/README.md does not describe `{group}/`"
        );
    }
}
