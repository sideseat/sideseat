#[test]
fn container_tests_share_trapped_cleanup() {
    let script = std::fs::read_to_string(repo_root().join("scripts/test/container-test.sh"))
        .expect("container test helper");
    for required in [
        "trap cleanup EXIT",
        "trap 'exit 130' INT",
        "trap 'exit 143' TERM",
        "local exit_code=$?",
        "if ((exit_code == 0 && cleanup_failed != 0))",
        "cargo test --locked -p sideseat-adapter-topics redis_stream_tests",
    ] {
        assert!(
            script.contains(required),
            "container test helper must contain `{required}`"
        );
    }

    let makefile = makefile_sources();
    for scenario in [
        "clickhouse",
        "clickhouse-replicated",
        "clickhouse-two-shard",
        "postgres",
        "redis",
        "redpanda",
    ] {
        assert!(
            makefile.contains(&format!("./scripts/test/container-test.sh {scenario}")),
            "{scenario} must use the shared container lifecycle"
        );
    }
}

#[test]
fn pre_commit_stays_cheap() {
    let hook = std::fs::read_to_string(repo_root().join("scripts/hooks/pre-commit"))
        .expect("pre-commit hook");
    for required in [
        "make --no-print-directory secret-scan-staged",
        "scripts/check/file-lengths.sh --cached",
    ] {
        assert!(hook.contains(required), "pre-commit must run `{required}`");
    }
    for forbidden in [
        "make test",
        "make lint",
        "make check",
        "nextest",
        "cargo test",
        "clippy",
    ] {
        assert!(
            !hook.contains(forbidden),
            "pre-commit must not run `{forbidden}`: tests and lint belong to make quick and pre-push"
        );
    }
}

/// Unused dependencies fail `make lint`, with a pinned tool, rather than being reported when installed.
#[test]
fn lint_rejects_unused_rust_dependencies() {
    let repo = repo_root();
    let quality = std::fs::read_to_string(repo.join("make/quality.mk")).expect("quality fragment");
    let lint = quality
        .split("\nlint:")
        .nth(1)
        .and_then(|rest| rest.split("\n\n").next())
        .expect("the lint target is declared");
    assert!(
        lint.contains("@cargo machete") && !lint.contains("SKIPPED"),
        "make lint must run cargo machete as a blocking gate"
    );
    let mise = std::fs::read_to_string(repo.join("mise.toml")).expect("mise.toml");
    assert!(
        mise.contains("\"cargo:cargo-machete\" = \"0.9.2\""),
        "mise.toml must pin cargo-machete so every checkout runs the same gate"
    );
}

#[test]
fn unsafe_code_is_confined_to_the_counting_allocator() {
    let repo = repo_root();
    let workspace = std::fs::read_to_string(repo.join("Cargo.toml")).expect("workspace manifest");
    assert!(
        workspace.contains("unsafe_code = \"deny\""),
        "the workspace must deny unsafe code by default"
    );

    let listing = Command::new("git")
        .args(["ls-files", "server", "sdk/rust"])
        .current_dir(repo)
        .output()
        .expect("git is available in a git checkout");
    let override_attribute = ["allow", "(unsafe_code)"].concat();
    let mut overrides = Vec::new();
    for file in String::from_utf8_lossy(&listing.stdout)
        .lines()
        .filter(|file| file.ends_with(".rs"))
    {
        let source = std::fs::read_to_string(repo.join(file)).expect("Rust source is readable");
        if source.contains(&override_attribute) {
            overrides.push(file.to_string());
        }
    }

    assert_eq!(
        overrides,
        ["server/src/runtime/allocation.rs"],
        "local unsafe-code overrides must remain confined to the counting allocator"
    );
}

#[test]
fn http_benchmark_bounds_requests_and_shutdown() {
    let script = std::fs::read_to_string(repo_root().join("scripts/perf/bench-http-latency.sh"))
        .expect("HTTP benchmark script");
    for required in [
        "CURL_CONNECT_TIMEOUT=",
        "CURL_MAX_TIME=",
        "curl --connect-timeout \"$CURL_CONNECT_TIMEOUT\" --max-time \"$CURL_MAX_TIME\"",
        "kill -TERM \"$SERVER_PID\"",
        "kill -KILL \"$SERVER_PID\"",
    ] {
        assert!(
            script.contains(required),
            "HTTP benchmark must contain `{required}`"
        );
    }
}

#[test]
fn stale_cleanup_discovers_every_incremental_directory() {
    let script = std::fs::read_to_string(repo_root().join("scripts/dev/clean-stale.sh"))
        .expect("cleanup script");
    let target_resolver =
        std::fs::read_to_string(repo_root().join("scripts/dev/cargo-target-dir.sh"))
            .expect("Cargo target resolver");
    for required in [
        "target_dir=\"$(bash scripts/dev/cargo-target-dir.sh)\"",
        "find \"$target_dir\" -type d -name incremental",
        "cargo sweep --time 3",
        "removing only units untouched for a day",
    ] {
        assert!(
            script.contains(required),
            "stale cleanup must contain `{required}`"
        );
    }
    // `--installed` fingerprints artifacts against installed toolchains and deleted the active toolchain's
    // builds mid-build when rustup could not fingerprint it.
    assert!(
        !script
            .lines()
            .filter(|line| !line.trim_start().starts_with('#'))
            .any(|line| line.contains("--installed")),
        "stale cleanup must not run `cargo sweep --installed`"
    );
    for required in [
        "cargo metadata --locked --no-deps",
        "metadata.target_directory",
        "refusing symbolic-link Cargo target directory",
        "refusing Cargo target that contains the repository",
        "refusing Cargo target that contains the home directory",
    ] {
        assert!(
            target_resolver.contains(required),
            "Cargo target resolver must contain `{required}`"
        );
    }
    assert!(
        !script.contains("cargo sweep --installed >/dev/null 2>&1 || true")
            && !script.contains("cargo sweep --time 3 >/dev/null 2>&1 || true"),
        "cargo-sweep failures must not be reported as successful cleanup"
    );
}

#[test]
fn make_help_is_generated_from_target_annotations() {
    let makefile = makefile_sources();
    assert!(
        makefile.contains("help: ## Show available commands")
            && makefile.contains("@awk -f make/help.awk $(MAKEFILE_LIST)")
            && !makefile.contains("@echo \"SideSeat Development Commands\"")
            && !makefile.contains("NOTARIZE ?= 1"),
        "Make help and defaults must have one current source"
    );
}

#[test]
fn dependency_report_discovers_every_project_manifest() {
    let repo = repo_root();
    let output = Command::new("bash")
        .arg("scripts/check/deps.sh")
        .env("SIDESEAT_DEPS_CHECK_DRY_RUN", "1")
        .current_dir(repo)
        .output()
        .expect("dependency report dry run");
    assert!(
        output.status.success(),
        "dependency report dry run failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let report = String::from_utf8_lossy(&output.stdout);
    let listing = Command::new("git")
        .args(["ls-files"])
        .current_dir(repo)
        .output()
        .expect("git is available in a git checkout");
    for manifest in String::from_utf8_lossy(&listing.stdout)
        .lines()
        .filter(|path| path.ends_with("package.json") || path.ends_with("pyproject.toml"))
    {
        assert!(
            report.contains(manifest),
            "dependency report omitted {manifest}"
        );
    }
    assert!(
        report.contains("Rust workspace (Cargo.toml)"),
        "dependency report omitted the Rust workspace"
    );

    let script =
        std::fs::read_to_string(repo.join("scripts/check/deps.sh")).expect("dependency report");
    assert!(
        script.contains("command -v cargo-outdated")
            && script.contains("npm_status=0")
            && script.contains("uv tree --project \"$project\" --locked --outdated --depth 1")
            && !script.contains("cargo outdated -R || echo"),
        "dependency failures must not be reported as missing tools or hidden"
    );
}

#[test]
fn frontend_build_script_belongs_to_the_embedding_crate() {
    let repo = repo_root();
    assert!(
        !repo.join("server/build.rs").exists(),
        "the composition root must not own another crate's build inputs"
    );

    let build_script =
        std::fs::read_to_string(repo.join("server/crates/api/build.rs")).expect("API build script");
    let embed = std::fs::read_to_string(repo.join("server/crates/api/src/embedded.rs"))
        .expect("frontend embed");
    assert!(
        build_script.contains("cargo::rerun-if-changed=../../../web/dist")
            && embed.contains("#[folder = \"../../../web/dist\"]"),
        "the crate that embeds web/dist must also own its Cargo invalidation"
    );
}

#[test]
fn update_networking_belongs_to_the_composition_root() {
    let repo = repo_root();
    let core_manifest =
        std::fs::read_to_string(repo.join("server/crates/core/Cargo.toml")).expect("core manifest");
    assert!(
        !core_manifest.contains("reqwest") && !core_manifest.contains("semver"),
        "the innermost crate must not own update-check networking"
    );
    assert!(
        repo.join("server/src/app/update.rs").exists()
            && !repo.join("server/crates/core/src/update.rs").exists(),
        "the executable composition root must own its update check"
    );
}

#[test]
fn core_owns_no_otlp_transport_types() {
    let repo = repo_root();
    let core_manifest =
        std::fs::read_to_string(repo.join("server/crates/core/Cargo.toml")).expect("core manifest");
    assert!(
        !core_manifest.contains("opentelemetry-proto")
            && !repo.join("server/crates/core/src/utils/otlp.rs").exists(),
        "the innermost crate must not depend on generated OTLP transport types"
    );
}

#[test]
fn core_crate_has_a_flat_module_root() {
    let repo = repo_root();
    let nested_root = repo.join("server/crates/core/src/core");
    let crate_root = std::fs::read_to_string(repo.join("server/crates/core/src/lib.rs"))
        .expect("core crate root");
    assert!(
        !nested_root.exists()
            && !crate_root
                .lines()
                .any(|line| line.trim() == "pub mod core;"),
        "the core crate must expose its modules directly from src/"
    );
}

#[test]
fn domain_crate_has_a_flat_module_root() {
    let repo = repo_root();
    let nested_root = repo.join("server/crates/domain/src/domain");
    let crate_root = std::fs::read_to_string(repo.join("server/crates/domain/src/lib.rs"))
        .expect("domain crate root");
    assert!(
        !nested_root.exists()
            && !crate_root
                .lines()
                .any(|line| line.trim() == "pub mod domain;"),
        "the domain crate must expose its modules directly from src/"
    );
}

#[test]
fn composition_root_has_no_legacy_test_facades() {
    let repo = repo_root();
    for path in ["server/src/data", "server/src/domain"] {
        assert!(
            !repo.join(path).exists(),
            "tests must import workspace crates directly instead of restoring {path}"
        );
    }
}

#[test]
fn adapter_crates_do_not_repeat_their_names_under_src() {
    let repo = repo_root();
    for path in [
        "server/crates/adapter-cache/src/cache",
        "server/crates/adapter-secrets/src/secrets",
    ] {
        assert!(
            !repo.join(path).exists(),
            "adapter modules must be exposed directly from src/: {path}"
        );
    }
}

#[test]
fn domain_owns_no_openapi_schema_dependency() {
    let repo = repo_root();
    let domain_manifest = std::fs::read_to_string(repo.join("server/crates/domain/Cargo.toml"))
        .expect("domain manifest");
    assert!(
        !domain_manifest.contains("utoipa"),
        "OpenAPI schemas belong to the API transport crate, not the domain"
    );
}

#[test]
fn domain_owns_no_otlp_transport_types() {
    let repo = repo_root();
    let manifest = std::fs::read_to_string(repo.join("server/crates/domain/Cargo.toml"))
        .expect("domain manifest");
    for dependency in ["opentelemetry-proto", "prost", "tonic"] {
        assert!(
            !manifest.lines().any(|line| {
                let line = line.trim();
                !line.starts_with('#')
                    && line
                        .split_once(['=', ' '])
                        .is_some_and(|(name, _)| name.trim() == dependency)
            }),
            "generated OTLP transport dependency `{dependency}` belongs in sideseat-ingestion"
        );
    }

    let source_root = repo.join("server/crates/domain/src");
    let mut pending = vec![source_root];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory).expect("domain source directory is readable") {
            let path = entry.expect("domain source entry is readable").path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().and_then(|extension| extension.to_str()) == Some("rs") {
                let source = std::fs::read_to_string(&path).expect("domain source is readable");
                assert!(
                    !source.contains("opentelemetry_proto") && !source.contains("prost::"),
                    "{} imports generated OTLP transport types",
                    path.strip_prefix(repo).unwrap_or(&path).display()
                );
            }
        }
    }
}

#[test]
fn pricing_http_is_owned_by_an_adapter() {
    let repo = repo_root();
    let domain_manifest = std::fs::read_to_string(repo.join("server/crates/domain/Cargo.toml"))
        .expect("domain manifest");
    let adapter_manifest =
        std::fs::read_to_string(repo.join("server/crates/adapter-pricing/Cargo.toml"))
            .expect("pricing adapter manifest");
    assert!(
        !domain_manifest.contains("reqwest")
            && adapter_manifest.contains("reqwest")
            && adapter_manifest.contains("sideseat-ports"),
        "pricing HTTP must implement a port in adapter-pricing, outside the domain"
    );
}

#[test]
fn rule_embedding_is_owned_by_the_asset_crate() {
    let repo = repo_root();
    let domain_manifest = std::fs::read_to_string(repo.join("server/crates/domain/Cargo.toml"))
        .expect("domain manifest");
    let assets_manifest =
        std::fs::read_to_string(repo.join("server/crates/rule-assets/Cargo.toml"))
            .expect("rule-assets manifest");
    assert!(
        !domain_manifest.contains("rust-embed")
            && domain_manifest.contains("sideseat-rule-assets")
            && assets_manifest.contains("rust-embed"),
        "rule asset embedding must live in rule-assets, outside the domain"
    );
}

/// Does this manifest line declare `driver`, under its own name or a rename?
///
/// A pure predicate keeps renamed-dependency handling testable without adding a real driver to a layer crate.
fn declares_driver(line: &str, driver: &str) -> bool {
    let trimmed = line.trim();
    // A driver named in a comment is how these manifests explain what they may not reach.
    if trimmed.starts_with('#') {
        return false;
    }
    let key_is_driver = trimmed
        .split_once(['=', ' '])
        .map(|(key, _)| key.trim() == driver)
        .unwrap_or(false);
    // Cargo dependencies may rename the package key while retaining the driver's package name.
    let renamed_to_driver =
        trimmed.contains("package") && trimmed.contains(&format!("\"{driver}\""));
    key_is_driver || renamed_to_driver
}

/// The driver predicate covers direct, renamed, commented, and prefix-adjacent declarations.
#[test]
fn the_driver_gate_reads_a_renamed_dependency() {
    assert!(declares_driver("duckdb = { workspace = true }", "duckdb"));
    assert!(
        declares_driver("db = { package = \"duckdb\", workspace = true }", "duckdb"),
        "a renamed driver is still the driver, and `use db::…` reaches it"
    );
    assert!(
        declares_driver("  sqlx = { workspace = true }  ", "sqlx"),
        "indentation is not a defence"
    );
    assert!(
        !declares_driver(
            "# no `duckdb` here: this crate may not name a driver",
            "duckdb"
        ),
        "a comment explaining the rule must not trip it"
    );
    assert!(
        !declares_driver("duckdb-adjacent = { workspace = true }", "duckdb"),
        "a different crate whose name starts the same is not the driver"
    );
    assert!(!declares_driver("serde = { workspace = true }", "duckdb"));
}

/// No inward layer crate names a driver, which is what makes the layer boundary a compiler check.
///
/// The point of splitting the workspace is that a forbidden dependency **does not compile** - there is no list
/// to maintain, and no macro or re-export that can defeat it. That property rests on one thing: the manifest not
/// naming the driver. So the manifest is what is checked.
///
/// It is not redundant with the compiler. The compiler refuses `use duckdb::…` in a crate that does not depend on
/// `duckdb`; it says nothing about someone *adding* the dependency, which is a one-line edit in a file nobody
/// diffs carefully. This test is the difference between "the boundary holds today" and "the boundary is a rule".
///
/// The crate list is derived from the workspace rather than named here, so a new layer crate is covered the
/// moment it exists, and the count is asserted so the scan cannot pass by finding nothing.
#[test]
fn no_layer_crate_depends_on_a_driver() {
    // The drivers a layer crate must not reach: the two analytics stores, the two transactional stores, the
    // object store, the queue, the cache, and the two transports. `sqlx` covers SQLite and PostgreSQL both.
    const DRIVERS: &[&str] = &[
        "duckdb",
        "clickhouse",
        "sqlx",
        "aws-sdk-s3",
        "aws-sdk-secretsmanager",
        "rdkafka",
        "redis",
        "axum",
        "tonic",
        "moka",
    ];

    let repo = repo_root();

    // Resolve workspace members from the manifest so the gate follows the build graph.
    let root = std::fs::read_to_string(repo.join("Cargo.toml")).expect("workspace manifest");
    let start = root.find("members = [").expect("members list");
    let end = root[start..].find(']').expect("members list ends") + start;
    let members: Vec<String> = root[start..end]
        .split('"')
        .filter(|t| repo.join(t).join("Cargo.toml").is_file())
        .map(str::to_string)
        .collect();
    let layer_crates: Vec<&String> = members
        .iter()
        // Adapter crates own drivers; every other crate under `server/crates/` is an inward-facing layer.
        .filter(|m| m.starts_with("server/crates/") && !m.starts_with("server/crates/adapter-"))
        .collect();

    let mut checked = 0usize;
    let mut violations: Vec<String> = Vec::new();

    for member in &layer_crates {
        let text = std::fs::read_to_string(repo.join(member).join("Cargo.toml"))
            .expect("manifest is readable");
        checked += 1;

        for line in text.lines() {
            let trimmed = line.trim();
            for driver in DRIVERS {
                // The API is the transport layer, so Axum and Tonic are its own tools rather than an
                // outward dependency. Storage, cache and queue drivers remain forbidden there.
                if member.as_str() == "server/crates/api" && matches!(*driver, "axum" | "tonic") {
                    continue;
                }
                if declares_driver(trimmed, driver) {
                    violations.push(format!("{member} declares `{driver}`"));
                }
            }
        }
    }

    assert!(
        checked >= 4,
        "scanned {checked} layer crate manifests - the members parse is wrong, not the workspace"
    );
    assert!(
        violations.is_empty(),
        "{} layer crate dependency(ies) on a driver - the boundary these crates exist to enforce is gone, and \
         a forbidden import in them would now compile:\n  {}",
        violations.len(),
        violations.join("\n  ")
    );
}

/// The cargo-hakari crate that pins one feature set per third-party dependency. Every member depends on it and it
/// holds no code, so it is no layer and says nothing about the direction of dependencies.
const WORKSPACE_HACK: &str = "sideseat-workspace-hack";

#[test]
fn the_api_crate_names_only_inward_workspace_crates() {
    let manifest = std::fs::read_to_string(repo_root().join("server/crates/api/Cargo.toml"))
        .expect("API manifest");
    let dependencies = manifest
        .split("[dependencies]")
        .nth(1)
        .and_then(|rest| rest.split("[dev-dependencies]").next())
        .expect("API dependencies section");
    let workspace_dependencies: BTreeSet<&str> = dependencies
        .lines()
        .filter_map(|line| line.split_once('='))
        // A dotted key (`name.workspace = true`) names the same dependency as `name = ...`.
        .map(|(name, _)| name.trim().split('.').next().unwrap_or_default())
        .filter(|name| name.starts_with("sideseat-") && *name != WORKSPACE_HACK)
        .collect();
    assert_eq!(
        workspace_dependencies,
        BTreeSet::from([
            "sideseat-core",
            "sideseat-domain",
            "sideseat-ingestion",
            "sideseat-messaging",
            "sideseat-ports",
        ]),
        "the API transport may depend only on inward-facing SideSeat crates"
    );
}

#[test]
fn messaging_stays_transport_neutral() {
    let manifest = std::fs::read_to_string(repo_root().join("server/crates/messaging/Cargo.toml"))
        .expect("messaging manifest");
    let dependencies = manifest
        .split("[dependencies]")
        .nth(1)
        .expect("messaging dependencies section");
    let workspace_dependencies: BTreeSet<&str> = dependencies
        .lines()
        .filter_map(|line| line.split_once('='))
        // A dotted key (`name.workspace = true`) names the same dependency as `name = ...`.
        .map(|(name, _)| name.trim().split('.').next().unwrap_or_default())
        .filter(|name| name.starts_with("sideseat-") && *name != WORKSPACE_HACK)
        .collect();

    assert_eq!(
        workspace_dependencies,
        BTreeSet::from(["sideseat-ports"]),
        "typed messaging may depend only on the transport-neutral queue port"
    );
    assert!(
        !manifest.contains("opentelemetry"),
        "OTLP message policy belongs to ingestion, not generic messaging"
    );
}

/// Production wiring that behavioural tests cannot see, because they call the underlying method directly.
///
/// These calls schedule side effects around a live `AppState` or database migration, while behavioural tests
/// exercise only the underlying operations. A structural assertion therefore verifies that production reaches
/// each operation. Commentary is stripped so prose cannot satisfy the check.
#[test]
fn every_detector_is_actually_started_in_production() {
    let repo = repo_root();
    for (file, call, why) in [
        (
            "server/src/app/background_tasks.rs",
            "start_consistency_check_task",
            "nothing would run the cross-partition consistency check, so the cross-month duplicate residual \
             would never be reported despite being documented as detected",
        ),
        (
            "server/crates/adapter-duckdb/src/lib.rs",
            "reconcile_trace_survivors",
            "retention would go back to the trace-wide file cleanup, reclaiming the files of spans that are \
             still live - and both behavioural tests call the reconciliation directly, so neither would notice",
        ),
        (
            "server/crates/adapter-duckdb/src/lib.rs",
            "record_retention_cleanup",
            "retention would delete spans without recording that their cleanup is owed, so a crash before the \
             cleanup orphans their files and favourites with nothing able to rediscover them",
        ),
        (
            "server/crates/adapter-duckdb/src/lib.rs",
            "traces_without_spans",
            "a favourited trace with one expired span would lose its favourite while still being visible",
        ),
    ] {
        let text = std::fs::read_to_string(repo.join(file))
            .unwrap_or_else(|e| panic!("{file} is readable: {e}"));
        // Remove commentary so only executable references satisfy the wiring check.
        let mut code = text.clone();
        for (_, comment) in rust_commentary(&text) {
            if !comment.is_empty() {
                code = code.replace(&comment, "");
            }
        }
        assert!(
            code.contains(call),
            "{file} no longer calls `{call}`, so {why}"
        );
    }
}

#[test]
fn the_storage_layer_does_not_import_the_http_layer() {
    let repo = repo_root();
    let mut offenders: Vec<String> = Vec::new();
    let mut roots = Vec::new();
    for entry in std::fs::read_dir(repo.join("server/crates")).expect("read crates dir") {
        let path = entry.expect("crate entry").path();
        if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("adapter-"))
        {
            roots.push(path.join("src"));
        }
    }
    assert!(
        !roots.is_empty(),
        "no adapter crate source roots discovered"
    );
    for root in &roots {
        assert!(
            root.join("lib.rs").is_file(),
            "adapter source root has no lib.rs: {}",
            root.display()
        );
    }

    let mut stack = roots;
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("read data dir") {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).expect("read source");

            // Blank out commentary so only code is matched.
            let mut code = text.clone();
            for (_, comment) in rust_commentary(&text) {
                if !comment.is_empty() {
                    code = code.replace(&comment, "");
                }
            }

            let relative = path
                .strip_prefix(repo_root())
                .unwrap_or(&path)
                .display()
                .to_string();
            for (offset, line) in code.lines().enumerate() {
                // `crate::api` in any position: a `use`, a fully-qualified call, a type in a signature.
                if line.contains("crate::api") || line.contains("sideseat_server") {
                    offenders.push(format!("{relative}:{}: {}", offset + 1, line.trim()));
                }
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "the storage layer reaches into the HTTP layer in {} place(s), which the crate split will refuse \
         to compile:\n  {}",
        offenders.len(),
        offenders.join("\n  ")
    );
}

fn raw_project_method_scope(source: &str) -> bool {
    source.contains("project_id: &str") || source.contains("project_ids: &[String]")
}

fn raw_project_query_scope(source: &str) -> bool {
    source.contains("pub project_id: String")
}

#[test]
fn every_tenant_scoped_port_uses_project_id() {
    let repo = repo_root();
    let mut method_sources = vec![
        repo.join("server/crates/ports/src/blobs.rs"),
        repo.join("server/crates/ports/src/registrations.rs"),
        repo.join("server/crates/ports/src/traits.rs"),
    ];
    let traits_dir = repo.join("server/crates/ports/src/traits");
    let mut trait_modules: Vec<_> = std::fs::read_dir(&traits_dir)
        .unwrap_or_else(|e| panic!("{} is readable: {e}", traits_dir.display()))
        .map(|entry| entry.expect("trait module directory entry").path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "rs"))
        .collect();
    trait_modules.sort();
    method_sources.extend(trait_modules);
    let query_sources = [
        "server/crates/ports/src/types/analytics.rs",
        "server/crates/ports/src/types/messages.rs",
        "server/crates/ports/src/types/stats.rs",
    ];

    let mut typed_scopes = 0usize;
    let mut offenders = Vec::new();
    for path in method_sources {
        let source = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("{} is readable: {e}", path.display()));
        typed_scopes += source.matches("project_id: &ProjectId").count();
        typed_scopes += source.matches("project_id: ProjectId").count();
        if raw_project_method_scope(&source) {
            offenders.push(
                path.strip_prefix(repo)
                    .unwrap_or(&path)
                    .display()
                    .to_string(),
            );
        }
    }
    for file in query_sources {
        let source = std::fs::read_to_string(repo.join(file))
            .unwrap_or_else(|e| panic!("{file} is readable: {e}"));
        typed_scopes += source.matches("pub project_id: ProjectId").count();
        if raw_project_query_scope(&source) {
            offenders.push(file.to_string());
        }
    }

    assert!(
        typed_scopes >= 70,
        "only found {typed_scopes} typed tenant scopes; the scan is no longer covering the port surface"
    );
    assert!(
        offenders.is_empty(),
        "raw project ids reopened the tenant boundary in: {}",
        offenders.join(", ")
    );
}

#[test]
fn the_project_id_gate_rejects_each_raw_shape() {
    for source in [
        "async fn get(&self, project_id: &str);",
        "async fn count(&self, project_ids: &[String]);",
    ] {
        assert!(
            raw_project_method_scope(source),
            "the tenant-scope gate missed a raw signature: {source}"
        );
    }
    assert!(raw_project_query_scope(
        "struct Query { pub project_id: String }"
    ));
    assert!(!raw_project_method_scope(
        "async fn get(&self, project_id: &ProjectId);"
    ));
    assert!(!raw_project_query_scope(
        "struct Query { pub project_id: ProjectId }"
    ));
}
