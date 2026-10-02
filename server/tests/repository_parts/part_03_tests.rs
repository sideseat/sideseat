
/// Every script that walks up to the repository root actually arrives there.
///
/// Root-walking expressions encode a directory depth that changes when a script moves. The stated level count
/// is compared with the script's current depth.
#[test]
fn every_script_that_locates_the_repository_root_finds_it() {
    let repo = repo_root();
    let listing = Command::new("git")
        .args(["ls-files"])
        .current_dir(repo)
        .output()
        .expect("git is available in a git checkout");
    let tracked: Vec<String> = String::from_utf8_lossy(&listing.stdout)
        .lines()
        .map(str::to_string)
        .collect();

    let mut checked = 0usize;
    let mut wrong: Vec<String> = Vec::new();
    for file in tracked
        .iter()
        // A script is identified by a supported extension or a shebang, which also covers extensionless hooks.
        .filter(|f| {
            [".sh", ".py", ".mjs", ".js", ".ts"]
                .iter()
                .any(|ext| f.ends_with(ext))
                || std::fs::read_to_string(repo.join(f)).is_ok_and(|text| text.starts_with("#!"))
        })
        // Exclude only vendored dependency trees; repository-owned examples remain in scope.
        .filter(|f| !f.contains("/.venv/") && !f.contains("/node_modules/"))
    {
        // The file's own depth: `scripts/bench-http-latency.sh` sits one directory below the root.
        let depth = file.matches('/').count();
        let text = std::fs::read_to_string(repo.join(file)).unwrap_or_default();
        for (number, line) in text.lines().enumerate() {
            // Root variable spelling is matched case-insensitively across supported scripting languages.
            let lower = line.to_ascii_lowercase();
            let assigns_root = lower.contains("root=") || lower.contains("root =");
            if !assigns_root {
                continue;
            }
            // Two idioms, and both state a level count that a move invalidates.
            let stated = if let Some(at) = line.find("parents[") {
                line[at + "parents[".len()..]
                    .split(']')
                    .next()
                    .and_then(|n| n.trim().parse::<usize>().ok())
            } else if line.contains("dirname") {
                Some(line.matches("..").count())
            } else {
                None
            };
            let Some(stated) = stated else { continue };
            if stated == 0 {
                continue;
            }
            checked += 1;
            if stated != depth {
                wrong.push(format!(
                    "{file}:{}: walks up {stated} level(s) from a file {depth} deep, so it lands {} the \
                     repository root",
                    number + 1,
                    if stated > depth { "above" } else { "below" }
                ));
            }
        }
    }

    assert!(
        checked >= 3,
        "only checked {checked} root resolutions - the scan is not finding them"
    );
    assert!(
        wrong.is_empty(),
        "{} script(s) do not resolve the repository root:\n  {}",
        wrong.len(),
        wrong.join("\n  ")
    );
}

/// `CONTRIBUTING.md`'s project structure names every top-level directory, and only real ones.
///
/// Both directions matter: imaginary entries misdirect readers, while omitted directories make the documented
/// organization incomplete. Hidden convention directories are excluded.
#[test]
fn the_documented_project_structure_matches_the_tree() {
    let repo = repo_root();
    let listing = Command::new("git")
        .args(["ls-files"])
        .current_dir(repo)
        .output()
        .expect("git is available in a git checkout");
    let actual: BTreeSet<&str> = String::from_utf8_lossy(&listing.stdout)
        .lines()
        .filter_map(|f| f.split_once('/').map(|(top, _)| top))
        .filter(|top| !top.starts_with('.'))
        .collect::<BTreeSet<_>>()
        .iter()
        .map(|s| Box::leak(s.to_string().into_boxed_str()) as &str)
        .collect();

    let contributing = std::fs::read_to_string(repo.join("CONTRIBUTING.md"))
        .expect("CONTRIBUTING.md is committed");
    let block = contributing
        .split("## Project Structure")
        .nth(1)
        .and_then(|rest| rest.split("```").nth(1))
        .expect("the project structure is a fenced block under its own heading");
    let documented: BTreeSet<&str> = block
        .lines()
        .filter_map(|l| l.split_whitespace().next())
        .filter_map(|first| first.strip_suffix('/'))
        .filter(|name| !name.contains('/'))
        .collect();

    let undocumented: Vec<&&str> = actual
        .iter()
        .filter(|d| !documented.contains(**d))
        .collect();
    let imaginary: Vec<&&str> = documented
        .iter()
        .filter(|d| !actual.contains(**d))
        .collect();
    assert!(
        documented.len() >= 10,
        "parsed only {} directories from the structure block - the parse is wrong, not the document",
        documented.len()
    );
    assert!(
        undocumented.is_empty() && imaginary.is_empty(),
        "CONTRIBUTING.md's project structure disagrees with the tree.\n  in the tree, undocumented: {:?}\n  \
         documented, not in the tree: {:?}",
        undocumented,
        imaginary
    );

    // A line that enumerates any child must enumerate all children; prose-only descriptions claim no inventory.
    let mut inventories: Vec<String> = Vec::new();
    for entry in &documented {
        let Some(line) = block
            .lines()
            .find(|l| l.split_whitespace().next() == Some(&format!("{entry}/")))
        else {
            continue;
        };
        let named: BTreeSet<&str> = line
            .split_whitespace()
            .skip(1)
            .map(|t| t.trim_matches(|c: char| ",;:()`".contains(c)))
            // Path-shaped and one segment deep: a child, not a prose word and not a nested path.
            .filter(|t| t.ends_with('/') || t.contains('.'))
            .map(|t| t.trim_end_matches('/'))
            .filter(|t| !t.is_empty() && !t.contains('/'))
            .collect();
        if named.is_empty() {
            continue;
        }
        let children: BTreeSet<&str> = String::from_utf8_lossy(&listing.stdout)
            .lines()
            .filter_map(|f| f.strip_prefix(&format!("{entry}/")))
            .map(|rest| rest.split('/').next().unwrap_or(rest))
            .collect::<BTreeSet<_>>()
            .iter()
            .map(|s| Box::leak(s.to_string().into_boxed_str()) as &str)
            .collect();
        for child in &named {
            if !children.contains(child) {
                inventories.push(format!("{entry}/ names `{child}`, which is not there"));
            }
        }
        for child in &children {
            if !named.contains(child) {
                inventories.push(format!(
                    "{entry}/ enumerates its children but omits `{child}`"
                ));
            }
        }
    }
    assert!(
        inventories.is_empty(),
        "{} inventory mismatch(es) in CONTRIBUTING.md's project structure:\n  {}",
        inventories.len(),
        inventories.join("\n  ")
    );
}

/// Every citation of a Rust module by directory and filename, anywhere in the repository, resolves to a real
/// file — in prose and in source comments alike.
///
/// Citations resolve root-relative, relative to the citing document, or as a unique repository suffix. The
/// scan is derived from tracked text and includes source comments and non-Rust assets.
#[test]
fn every_module_path_cited_anywhere_resolves() {
    let repo = repo_root();
    let listing = Command::new("git")
        .args(["ls-files"])
        .current_dir(repo)
        .output()
        .expect("git is available in a git checkout");
    let mut tracked: Vec<String> = String::from_utf8_lossy(&listing.stdout)
        .lines()
        .map(str::to_string)
        .collect();
    let untracked = Command::new("git")
        .args(["ls-files", "--others", "--exclude-standard"])
        .current_dir(repo)
        .output()
        .expect("git is available in a git checkout");
    tracked.extend(
        String::from_utf8_lossy(&untracked.stdout)
            .lines()
            .map(str::to_string),
    );

    // Captured OTLP payloads are data, not documentation: whatever paths a framework recorded are its business.
    let citing: Vec<&String> = tracked
        .iter()
        .filter(|f| !f.starts_with("server/tests/fixtures/"))
        // Text detection covers documentation, configuration, scripts, specifications, and extensionless hooks.
        .filter(|f| is_text(&repo.join(f)))
        .collect();

    let resolves = |citing_file: &str, cited: &str| -> bool {
        if cited.starts_with("../") || cited.starts_with("./") {
            let mut parts: Vec<&str> = citing_file.split('/').collect();
            parts.pop();
            for segment in cited.split('/') {
                match segment {
                    "." => {}
                    ".." => {
                        if parts.pop().is_none() {
                            return false;
                        }
                    }
                    other => parts.push(other),
                }
            }
            let resolved = parts.join("/");
            // The ESM allowance applies here too: a relative import of `index` with a `.js` suffix beside an `index.ts` is the same
            // convention, and this branch returned before the fallback below could say so.
            return tracked.contains(&resolved)
                || resolved.strip_suffix(".js").is_some_and(|stem| {
                    [".ts", ".tsx"]
                        .iter()
                        .any(|ext| tracked.contains(&format!("{stem}{ext}")))
                });
        }
        if tracked.iter().any(|f| f == cited) {
            return true;
        }
        if tracked.iter().any(|f| f.ends_with(&format!("/{cited}"))) {
            return true;
        }
        // An ESM import names the *emitted* file: an import naming `index` with a `.js` suffix beside an `index.ts` is the convention, not a
        // stale path, and TypeScript requires it. Resolved against the source it compiles from.
        cited.strip_suffix(".js").is_some_and(|stem| {
            [".ts", ".tsx"].iter().any(|ext| {
                let source = format!("{stem}{ext}");
                tracked.iter().any(|f| f.ends_with(&format!("/{source}")))
            })
        })
    };

    let mut checked = 0usize;
    let mut unresolved: Vec<String> = Vec::new();
    for file in citing {
        let text = std::fs::read_to_string(repo.join(file)).unwrap_or_default();
        // In Rust, only comments describe the layout; a string literal or a module path is not a citation.
        let commentary: Vec<(usize, String)> = if file.ends_with(".rs") {
            rust_commentary(&text)
        } else {
            text.lines()
                .enumerate()
                .map(|(n, l)| (n + 1, l.to_string()))
                .collect()
        };
        for (number, line) in commentary {
            for token in line.split(|c: char| c.is_whitespace() || "`(),;\"'[]<>".contains(c)) {
                // A path with at least one directory and a Rust file at the end. A trailing `:line` is a
                // citation of a position in that file, so it is stripped before resolving.
                let cited = token.trim_end_matches(['.', ':']);
                let cited = cited.split(':').next().unwrap_or(cited);
                // Globs and absolute, home-relative, variable, or elided paths are not repository citations.
                // `!` as well as `@` and `-`: a `.gitignore` negation is a claim about a path, and leaving
                // the marker on made `!data/.gitkeep` unresolvable while the file was right there.
                let cited = cited.trim_start_matches(['@', '-', '!']);
                if !cited.contains('/')
                    || cited.contains('*')
                    // A shell or make variable, an assignment, or an elided path: not a literal claim about
                    // where a file is. `$WORK/sideseat.json` and `sdk/python/.../client.py` are both fine.
                    || cited.contains('$')
                    || cited.contains('=')
                    || cited.contains("...")
                    || cited.starts_with('~')
                    || cited.starts_with('/')
                    || cited.starts_with("./")
                    || cited.contains("node_modules/")
                    || cited.starts_with("http")
                {
                    continue;
                }
                // Require the basename to exist somewhere in the tree before treating the token as an intended
                // repository citation. Deleted or external basenames and bare filenames are outside this
                // working-tree-only check.
                let basename = cited.rsplit('/').next().unwrap_or(cited);
                let held_somewhere = tracked
                    .iter()
                    .any(|f| f.rsplit('/').next() == Some(basename));
                if !held_somewhere {
                    continue;
                }
                checked += 1;
                if !resolves(file, cited) {
                    unresolved.push(format!("{file}:{number}: {cited}"));
                }
            }
        }
    }

    assert!(
        checked > 60,
        "only checked {checked} cited paths - the scan found far fewer citations than this repository carries"
    );
    assert!(
        unresolved.is_empty(),
        "{} cited module path(s) resolve to nothing:\n  {}",
        unresolved.len(),
        unresolved.join("\n  ")
    );
}

/// The storage layer does not reach into the HTTP layer.
///
/// Adapter crates cannot depend on `sideseat-api`; comments are stripped so only source dependencies count.
/// Does this tracked path declare container images this repository does not control?
///
/// Prefix matching covers Dockerfile variants and Compose override files.
fn declares_images(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    path.starts_with(".github/workflows/")
        || path.ends_with("/action.yml")
        || path.ends_with("/action.yaml")
        || name.starts_with("Dockerfile")
        || name.starts_with("docker-compose.")
        || name.starts_with("compose.")
}

/// The image reference on a `FROM` or `image:` line, with flags and stage names stepped over.
///
/// `FROM x AS stage` names a stage *after* the reference and `FROM --platform=... x` puts flags *before* it.
fn image_reference_in(rest: &str) -> &str {
    rest.trim()
        .trim_matches('"')
        .split_whitespace()
        .find(|token| !token.starts_with("--"))
        .unwrap_or_default()
}

/// The two parsing decisions above, on input the tree does not contain.
///
/// Synthetic cases keep uncommon but valid Dockerfile and Compose shapes covered.
#[test]
fn the_image_gate_reads_the_shapes_that_defeated_it() {
    // A flag before the reference, which is ordinary in a multi-arch Dockerfile.
    assert_eq!(
        image_reference_in("--platform=$BUILDPLATFORM debian:bookworm-slim AS builder"),
        "debian:bookworm-slim",
        "a `--platform` flag must not be mistaken for the image"
    );
    assert_eq!(
        image_reference_in("debian:bookworm-slim AS builder"),
        "debian:bookworm-slim",
        "a stage name after the reference is not part of it"
    );
    assert_eq!(
        image_reference_in(" \"postgres:17-alpine\" "),
        "postgres:17-alpine",
        "a quoted Compose image is the reference without its quotes"
    );

    // Compose spellings Compose itself accepts.
    for path in [
        "deploy/local/docker-compose.yml",
        "deploy/local/docker-compose.override.yml",
        "deploy/compose.yaml",
        "deploy/Dockerfile",
        "deploy/Dockerfile.dev",
        ".github/workflows/ci.yml",
        "some/action.yml",
    ] {
        assert!(
            declares_images(path),
            "{path} declares images and must be read"
        );
    }
    for path in ["server/src/app.rs", "docs/compose-notes.md", "README.md"] {
        assert!(!declares_images(path), "{path} declares no images");
    }
}

/// No adapter reaches into a sibling adapter.
///
/// Shared vocabulary belongs in an inward-facing crate; concrete adapters keep only their own rendering.
/// Cross-adapter parity tests are exempt by path because comparing implementations is their purpose.
#[test]
fn no_adapter_imports_a_sibling_adapter() {
    let repo = repo_root();
    let mut adapters: Vec<String> = std::fs::read_dir(repo.join("server/crates"))
        .expect("crates directory is readable")
        .filter_map(Result::ok)
        .filter(|entry| entry.path().join("Cargo.toml").is_file())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter_map(|name| name.strip_prefix("adapter-").map(str::to_string))
        .collect();
    adapters.sort();
    assert!(
        adapters.len() >= 9,
        "found only {} adapter crates: {adapters:?}",
        adapters.len()
    );

    let mut violations: Vec<String> = Vec::new();
    let mut empty_roots: Vec<String> = Vec::new();

    for adapter in &adapters {
        let dir = repo.join(format!("server/crates/adapter-{adapter}/src"));
        assert!(dir.is_dir(), "{} is an adapter source root", dir.display());
        let mut stack = vec![dir];
        let mut checked = 0usize;
        while let Some(current) = stack.pop() {
            for entry in std::fs::read_dir(&current).expect("readable directory") {
                let path = entry.expect("readable entry").path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                    continue;
                }
                let relative = path
                    .strip_prefix(repo)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .replace('\\', "/");
                let text = std::fs::read_to_string(&path).expect("readable file");
                // Commentary names siblings deliberately - explaining why a rendering is per-dialect is the
                // documentation this move exists to make true - so only code counts.
                let mut code = text.clone();
                for (_, comment) in rust_commentary(&text) {
                    if !comment.is_empty() {
                        code = code.replace(&comment, "");
                    }
                }
                checked += 1;
                for sibling in &adapters {
                    if sibling == adapter {
                        continue;
                    }
                    // The module path, **with or without a trailing `::`**. Requiring `::` missed
                    // `use crate::data::duckdb;` - which imports the whole module and then reaches into it - and
                    // `use crate::data::duckdb as db;`, an alias. A trailing character that cannot continue an
                    // identifier is what distinguishes the sibling from a longer name.
                    let needle = format!("crate::data::{sibling}");
                    let reaches = code.match_indices(&needle).any(|(i, _)| {
                        code[i + needle.len()..]
                            .chars()
                            .next()
                            .map(|c| !c.is_alphanumeric() && c != '_')
                            .unwrap_or(true)
                    });
                    if reaches {
                        violations.push(format!("{relative} imports crate::data::{sibling}"));
                    }

                    let package = format!("sideseat_adapter_{}", sibling.replace('-', "_"));
                    if code.contains(&package) {
                        violations.push(format!("{relative} imports {package}"));
                    }
                }
            }
        }
        if checked == 0 {
            empty_roots.push(format!("adapter-{adapter}"));
        }

        let manifest = std::fs::read_to_string(
            repo.join(format!("server/crates/adapter-{adapter}/Cargo.toml")),
        )
        .expect("adapter manifest is readable");
        let dependencies = manifest
            .split("[dependencies]")
            .nth(1)
            .and_then(|rest| rest.split("\n[").next())
            .unwrap_or_default();
        for sibling in &adapters {
            if sibling == adapter {
                continue;
            }
            let package = format!("sideseat-adapter-{sibling}");
            if dependencies
                .lines()
                .any(|line| declares_driver(line, &package))
            {
                violations.push(format!(
                    "server/crates/adapter-{adapter}/Cargo.toml depends on `{package}`"
                ));
            }
        }
    }

    assert!(
        empty_roots.is_empty(),
        "adapter source root(s) contained no Rust files: {}",
        empty_roots.join(", ")
    );
    assert!(
        violations.is_empty(),
        "{} cross-adapter import(s) - a type shared by two adapters belongs to neither, and while one owns it \
         they cannot be separate crates:\n  {}",
        violations.len(),
        violations.join("\n  ")
    );
}

/// The ports crate emits no SQL.
///
/// A port says what a caller may ask for; a statement is how a store answers.
/// SQL rendering belongs to the query layer, not the transport-facing contract.
///
/// Keywords rather than a parser, and the list is deliberately short: these are the words that only appear in a
/// statement. A port may of course contain the *word* "select" in prose, so commentary is stripped first.
#[test]
fn the_ports_crate_emits_no_sql() {
    const SQL_MARKERS: &[&str] = &[
        "SELECT ",
        "INSERT INTO",
        "UPDATE ",
        "DELETE FROM",
        " WHERE ",
        "ORDER BY ",
        "GROUP BY ",
        "LEFT JOIN",
        "CREATE TABLE",
    ];

    let repo = repo_root();
    let mut offenders: Vec<String> = Vec::new();
    let mut checked = 0usize;
    let mut stack = vec![repo.join("server/crates/ports/src")];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("readable directory") {
            let path = entry.expect("readable entry").path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).expect("readable file");
            let mut code = text.clone();
            for (_, comment) in rust_commentary(&text) {
                if !comment.is_empty() {
                    code = code.replace(&comment, "");
                }
            }
            checked += 1;
            for marker in SQL_MARKERS {
                if code.contains(marker) {
                    let shown = path.strip_prefix(repo).unwrap_or(&path).display();
                    offenders.push(format!("{shown} contains `{}`", marker.trim()));
                }
            }
        }
    }

    assert!(
        checked > 5,
        "scanned {checked} files under server/crates/ports/src - the walk is wrong, not the crate"
    );
    assert!(
        offenders.is_empty(),
        "{} SQL fragment(s) in the ports crate - a port describes the question, not the statement:\n  {}",
        offenders.len(),
        offenders.join("\n  ")
    );
}

/// Once an operation enters the typed query registry, both analytical adapters must delegate its
/// statement to that registry and may no longer keep a second SQL spelling.
///
/// The operation list is imported from `sideseat-query-sql`, not copied here. Registering the next
/// migrated operation therefore tightens this gate in the same change.
#[test]
fn migrated_analytics_operations_hold_no_adapter_sql_literal() {
    const ADAPTERS: &[(&str, &str)] = &[
        ("DuckDB", "server/crates/adapter-duckdb/src/repositories"),
        (
            "ClickHouse",
            "server/crates/adapter-clickhouse/src/repositories",
        ),
    ];
    const SQL_MARKERS: &[&str] = &[
        "SELECT ",
        " FROM ",
        " WHERE ",
        "ORDER BY ",
        "GROUP BY ",
        "DELETE FROM",
        "ALTER TABLE",
        "INSERT INTO",
    ];

    assert!(
        !MIGRATED_OPERATIONS.is_empty(),
        "the query-builder registry must contain a real operation"
    );

    for operation in MIGRATED_OPERATIONS {
        for (adapter, repository_dir) in ADAPTERS {
            let relative = format!("{repository_dir}/{}", operation.adapter_repository());
            let source = std::fs::read_to_string(repo_root().join(&relative))
                .unwrap_or_else(|error| panic!("{relative} is readable: {error}"));
            let sync_marker = format!("pub fn {}(", operation.adapter_function());
            let async_marker = format!("pub async fn {}(", operation.adapter_function());
            let start = source
                .find(&sync_marker)
                .or_else(|| source.find(&async_marker))
                .unwrap_or_else(|| {
                    panic!(
                        "{adapter} has a `{}` operation for the builder registry",
                        operation.adapter_function()
                    )
                });
            let rest = &source[start..];
            let end = rest[1..]
                .find("\n/// ")
                .map(|offset| offset + 1)
                .unwrap_or(rest.len());
            let function = &rest[..end];

            assert!(
                function.contains(operation.builder_module()),
                "{relative}'s `{}` does not delegate to the typed `{}` builder",
                operation.name(),
                operation.builder_module(),
            );

            let scan = if operation.scans_entire_repository() {
                source.split("\n#[cfg(test)]").next().unwrap_or(&source)
            } else {
                function
            };
            let code = rust_code_without_comments(scan);
            let upper = code.to_ascii_uppercase();
            let offenders = SQL_MARKERS
                .iter()
                .filter(|marker| upper.contains(**marker))
                .copied()
                .collect::<Vec<_>>();
            assert!(
                offenders.is_empty(),
                "{relative}'s migrated `{}` still owns SQL marker(s): {}",
                operation.name(),
                offenders.join(", ")
            );
        }
    }
}

/// All database adapters share the same version-state machine. SQL and transaction mechanics stay
/// local, but no adapter may reintroduce its own `(current + 1)..=target` loop or too-new policy.
#[test]
fn every_database_adapter_uses_the_shared_migration_runner() {
    let files = [
        "server/crates/adapter-duckdb/src/migrations.rs",
        "server/crates/adapter-sqlite/src/migrations.rs",
        "server/crates/adapter-postgres/src/migrations.rs",
        "server/crates/adapter-clickhouse/src/lib.rs",
    ];

    for relative in files {
        let source = std::fs::read_to_string(repo_root().join(relative))
            .unwrap_or_else(|error| panic!("{relative} is readable: {error}"));
        let production = source.split("\n#[cfg(test)]").next().unwrap_or(&source);
        let mut code = production.to_string();
        for (_, comment) in rust_commentary(production) {
            if !comment.is_empty() {
                code = code.replace(&comment, "");
            }
        }

        assert!(
            code.contains("plan_migrations("),
            "{relative} bypasses sideseat_core::migration::plan_migrations"
        );
        assert!(
            !code.contains("current_version + 1") && !code.contains("(v + 1)..="),
            "{relative} has reintroduced a local migration-version loop"
        );
    }
}

/// Every crate physically placed under `server/crates/` is a workspace member, and vice versa.
///
/// A crate omitted from `members` is invisible to workspace checks, so deriving later architecture gates only
/// from the manifest would let a relocated or newly added crate escape all of them.
#[test]
fn every_server_crate_is_a_workspace_member() {
    let repo = repo_root();
    let root = std::fs::read_to_string(repo.join("Cargo.toml")).expect("workspace manifest");
    let start = root.find("members = [").expect("members list");
    let end = root[start..].find(']').expect("members list ends") + start;
    let declared: BTreeSet<String> = root[start..end]
        .split('"')
        .filter(|member| member.starts_with("server/crates/"))
        .map(str::to_string)
        .collect();

    let actual: BTreeSet<String> = std::fs::read_dir(repo.join("server/crates"))
        .expect("crates directory is readable")
        .filter_map(Result::ok)
        .filter(|entry| entry.path().join("Cargo.toml").is_file())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .map(|name| format!("server/crates/{name}"))
        .collect();

    assert!(
        actual.len() >= 14,
        "found only {} crate manifests under server/crates: {actual:?}",
        actual.len()
    );
    assert_eq!(
        declared, actual,
        "Cargo workspace members and server/crates/*/Cargo.toml must match in both directions"
    );
}

/// Every workspace crate reports the same version, and takes it from one place.
///
/// Product metadata reads `CARGO_PKG_VERSION` from whichever crate owns it, so server crates inherit the single
/// `[workspace.package]` version. Independently released SDKs are excluded below.
#[test]
fn every_workspace_crate_takes_the_one_version() {
    let repo = repo_root();
    let root = std::fs::read_to_string(repo.join("Cargo.toml")).expect("workspace manifest");

    let members: Vec<String> = {
        let start = root.find("members = [").expect("members list");
        let end = root[start..].find(']').expect("members list ends") + start;
        root[start..end]
            .split('"')
            .filter(|t| {
                t.contains('/') || (!t.contains(',') && !t.contains('=') && !t.trim().is_empty())
            })
            .filter(|t| repo.join(t).join("Cargo.toml").is_file())
            .map(str::to_string)
            .collect()
    };
    assert!(
        members.len() >= 3,
        "parsed {} workspace members - the parse is wrong, not the manifest",
        members.len()
    );

    // The SDKs are released on their own cadence - `make sync-version` says so itself, "server + CLI only; SDKs
    // maintained separately" - so their versions are deliberately independent. Exempt by prefix, with the reason,
    // rather than by omitting them from the walk.
    let mut offenders: Vec<String> = Vec::new();
    for member in &members {
        if member.starts_with("sdk/") {
            continue;
        }
        let text = std::fs::read_to_string(repo.join(member).join("Cargo.toml")).expect("manifest");
        // The `[package]` section only: a *dependency* may pin a version, which is not this question. Taken from
        // the header to the next section rather than as "everything before the first `[`" - these manifests open
        // with a comment block, so that form returned the comments and the check could not fail.
        let package = match text.find("[package]") {
            Some(start) => {
                let rest = &text[start + "[package]".len()..];
                let end = rest.find("\n[").unwrap_or(rest.len());
                rest[..end].to_string()
            }
            None => String::new(),
        };
        let spells_own = package
            .lines()
            .any(|l| l.trim_start().starts_with("version = \""));
        if spells_own {
            offenders.push(format!(
                "{member} spells its own version instead of `version.workspace = true`"
            ));
        }
    }

    assert!(
        offenders.is_empty(),
        "{} crate(s) can drift from the workspace version, which decides what `--version`, the banner and the \
         update check report:\n  {}",
        offenders.len(),
        offenders.join("\n  ")
    );
}

#[test]
fn release_stages_only_version_files_and_pushes_atomically() {
    let script =
        std::fs::read_to_string(repo_root().join("scripts/release.sh")).expect("release script");

    for required in [
        "git status --porcelain",
        "make --no-print-directory version-check",
        "git add -- \"${version_files[@]}\"",
        "git diff --quiet",
        "git ls-files --others --exclude-standard",
        "git push --atomic origin",
    ] {
        assert!(
            script.contains(required),
            "release script must contain `{required}`"
        );
    }
    for unsafe_command in ["git add -A", "git push --tags"] {
        assert!(
            !script.contains(unsafe_command),
            "release script must not contain `{unsafe_command}`"
        );
    }

    let bump = script.find("make --no-print-directory bump").expect("bump");
    let verification = script
        .find("make --no-print-directory version-check")
        .expect("post-bump version check");
    let checks: Vec<usize> = script
        .match_indices("make --no-print-directory check")
        .map(|(index, _)| index)
        .collect();
    assert!(
        verification > bump && checks.iter().any(|index| *index > bump),
        "release must verify versions and run checks after the bump"
    );
}

#[test]
fn development_processes_are_scoped_and_environment_is_explicit() {
    let dev = std::fs::read_to_string(repo_root().join("scripts/dev.sh")).expect("dev script");
    assert!(
        !dev.contains("kill 0"),
        "development cleanup must not signal the caller's process group"
    );
    assert!(
        dev.contains("kill -TERM \"$pid\"") && dev.contains("kill -KILL \"$pid\""),
        "development cleanup must target only its tracked child processes"
    );
    assert!(
        dev.contains("child_status=1"),
        "an unexpected clean child exit must still fail the development supervisor"
    );

    let server = std::fs::read_to_string(repo_root().join("scripts/dev-server.sh"))
        .expect("dev-server script");
    assert!(
        server.contains("server_env=(")
            && server.contains("exec env \"${server_env[@]}\"")
            && server.contains("\"SIDESEAT_SECRETS_BACKEND=file\""),
        "server environment must be passed as an env array, not expanded as command words"
    );
}

#[test]
fn maintenance_loops_fail_fast_and_hook_setup_supports_worktrees() {
    let makefile = makefile_sources();
    assert!(
        makefile.contains("update-python-deps: ## Upgrade every Python lockfile")
            && makefile
                .contains("@set -e; for manifest in $$(git ls-files '*pyproject.toml'); do \\")
            && makefile.contains("(cd \"$$project\" && uv lock --upgrade); \\"),
        "dependency updates must stop at the first failed project"
    );

    let hooks_start = makefile.find("setup-hooks:").expect("setup-hooks target");
    let hooks_end = makefile[hooks_start..]
        .find("\n\n")
        .map(|offset| hooks_start + offset)
        .expect("setup-hooks recipe ends");
    let hooks = &makefile[hooks_start..hooks_end];
    assert!(
        hooks.contains("git rev-parse --git-dir")
            && !hooks.contains("[ -d .git ]")
            && !hooks.contains("|| true"),
        "hook setup must accept linked worktrees and report installation failures"
    );
}
