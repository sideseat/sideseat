//! Invariants about the **repository**, not about the server.
//!
//! An integration test rather than a module under `src/`: these are about what the tree contains — which
//! manifests exist, what a committed fixture carries — and the first place a newcomer looks for that is not
//! `server/src/domain/`. The dependabot check lived inside an 8,500-line carrier-rules test file, which is
//! where it was written rather than where it belongs.
//!
//! What deliberately stays in `src/`: the two diagram checks. They read `embedded_sources()` and the rule
//! plans, which are crate-private, so an integration test cannot see them at all.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::Command;

/// The repository root, from this test file's own location.
fn repo_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the crate sits in the repository")
}

/// Dependabot watches **every** manifest in the tree that it can read.
///
/// Three inventories of mine were incomplete in a row - the Cargo/npm/uv entries, then the two standalone
/// tools' lockfiles, then the container images - and each time the file *looked* complete because its own
/// comment claimed one entry per manifest. So the coverage is derived from the tree rather than maintained by
/// hand: a new `package.json`, `pyproject.toml`, `Dockerfile` or Compose file fails this until it is declared.
///
/// The ecosystem is checked, not merely the directory. A Compose file is a **separate** ecosystem from a
/// Dockerfile, and declaring `/deploy/local` under `docker` left four pinned services unwatched while a
/// containment check would have passed - which is why this parses the blocks rather than searching the text.
///
/// Two exclusions, each on a stated ground rather than by omission: the `examples/` suites pin framework
/// versions on purpose, so a bump there changes what a sample demonstrates; and a `package.json` declaring no
/// dependencies has nothing to update.
#[test]
fn dependabot_covers_every_manifest_in_the_tree() {
    let repo = repo_root();
    let config = std::fs::read_to_string(repo.join(".github/dependabot.yml"))
        .expect("dependabot.yml is committed");

    // ecosystem -> the directories it declares. A block runs until the next `package-ecosystem`.
    let mut declared: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut current = String::new();
    for line in config.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("- package-ecosystem:") {
            current = rest.trim().trim_matches('"').to_string();
            declared.entry(current.clone()).or_default();
        } else if let Some(rest) = trimmed.strip_prefix("directory:") {
            declared
                .entry(current.clone())
                .or_default()
                .insert(rest.trim().trim_matches('"').to_string());
        } else if trimmed.starts_with("- \"/") || trimmed == "- \"/\"" {
            declared.entry(current.clone()).or_default().insert(
                trimmed
                    .trim_start_matches("- ")
                    .trim_matches('"')
                    .to_string(),
            );
        }
    }
    assert!(
        declared.len() >= 5,
        "parsed only {} ecosystems from dependabot.yml - the parse is wrong, not the config",
        declared.len()
    );

    let listing = Command::new("git")
        .args(["ls-files"])
        .current_dir(repo)
        .output()
        .expect("git is available in a git checkout");
    let mut gaps: Vec<String> = Vec::new();
    let mut checked = 0usize;
    for file in String::from_utf8_lossy(&listing.stdout).lines() {
        // Deliberately excluded, on the grounds given in the doc comment.
        if file.starts_with("examples/") {
            continue;
        }
        let name = file.rsplit('/').next().unwrap_or(file);
        let ecosystem = match name {
            // A crate needs its own entry only when it has its own lockfile: a workspace member shares the
            // root lock, so declaring it separately would ask Dependabot to resolve a lock that is not there.
            // Filtering on "is it the root manifest" happened to give the same answer for this tree and is the
            // wrong question - a standalone nested crate would have slipped through.
            "Cargo.toml" => {
                let dir = file
                    .rsplit_once('/')
                    .map_or(String::new(), |(p, _)| format!("{p}/"));
                if !repo.join(format!("{dir}Cargo.lock")).exists() {
                    continue;
                }
                "cargo"
            }
            "package.json" => "npm",
            "pyproject.toml" => "uv",
            "Dockerfile" => "docker",
            // Both spellings of both names: Compose accepts `compose.yaml` and `compose.yml`, and a guard that
            // knows one of them is a guard that can be bypassed by naming the file the other way.
            "docker-compose.yml" | "docker-compose.yaml" | "compose.yml" | "compose.yaml" => {
                "docker-compose"
            }
            _ => continue,
        };
        let dir = match file.rsplit_once('/') {
            Some((parent, _)) => format!("/{parent}"),
            None => "/".to_string(),
        };
        // A package with no dependencies has nothing for Dependabot to update.
        if ecosystem == "npm" {
            let body = std::fs::read_to_string(repo.join(file)).unwrap_or_default();
            if ![
                "\"dependencies\"",
                "\"devDependencies\"",
                "\"optionalDependencies\"",
            ]
            .iter()
            .any(|key| body.contains(key))
            {
                continue;
            }
        }
        checked += 1;
        if !declared
            .get(ecosystem)
            .is_some_and(|dirs| dirs.contains(&dir))
        {
            gaps.push(format!("{ecosystem} {dir}  (from {file})"));
        }
    }

    assert!(checked > 6, "only checked {checked} manifests");
    assert!(
        gaps.is_empty(),
        "{} manifest(s) no Dependabot entry covers - add them to .github/dependabot.yml, or exclude them \
         here on a stated ground:\n  {}",
        gaps.len(),
        gaps.join("\n  ")
    );
}
/// No committed fixture carries the capturing developer's account name.
///
/// A sample that reads a file records the absolute path it read, so a capture carries whoever ran it into a
/// public repository: 918 occurrences across 48 fixtures before this existed, naming one maintainer's home
/// directory. `record-otlp.py` substitutes a placeholder at capture time now, and this is what keeps the next
/// capture from quietly reintroducing it — the script is the fix, and a fix nothing checks is a convention.
///
/// The check is "does a fixture contain the *current* user's name", which is the question that can be asked
/// without a list of forbidden names to maintain. It therefore says nothing about a name nobody has run under,
/// and that limit is why the substitution lives in the capture script rather than only here.
#[test]
fn no_fixture_carries_the_capturing_users_name() {
    let user = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_default();
    if user.is_empty() || user == "sideseat" || user.len() < 3 {
        eprintln!("fixtures: no distinctive account name to look for - skipping");
        return;
    }
    let needle = user.as_bytes();
    // **Tracked files only**, which is the property: the concern is what a public repository carries, and a
    // local-only fixture directory is gitignored precisely because it is nobody else's. Scanning the working
    // tree instead reported eight files in `vercel-ai-js/image-gen/`, which `.gitignore` excludes - a failure
    // for something that is not published.
    let repo = repo_root();
    let listing = std::process::Command::new("git")
        .args(["ls-files", "-z", "server/tests/fixtures"])
        .current_dir(repo)
        .output()
        .expect("git is available in a git checkout");
    assert!(listing.status.success(), "git ls-files failed");
    let mut offenders: Vec<String> = Vec::new();
    let mut scanned = 0usize;
    for rel in listing
        .stdout
        .split(|b| *b == 0)
        .filter(|entry| !entry.is_empty())
    {
        let rel = String::from_utf8_lossy(rel).to_string();
        let Ok(bytes) = std::fs::read(repo.join(&rel)) else {
            continue;
        };
        scanned += 1;
        if bytes.windows(needle.len()).any(|w| w == needle) {
            offenders.push(rel);
        }
    }
    assert!(scanned > 100, "only scanned {scanned} fixture files");
    assert!(
        offenders.is_empty(),
        "{} fixture(s) contain the account name `{user}`, which a public repository should not carry - \
         re-capture with scripts/message-fixtures/capture.sh, which substitutes a placeholder:\n  {}",
        offenders.len(),
        offenders.join("\n  ")
    );
}

/// Every module `CLAUDE.md` names in its tree diagram exists.
///
/// The diagram is a map handed to whoever arrives, and it had drifted: it pointed at `core/topic.rs` after
/// pub/sub moved to `data/topics/`, and at `sideml/pipeline.rs` after that file became `normalize.rs`. A map
/// that names a file nobody can open costs more than no map, because it is trusted.
///
/// Checked by **name**, not by position in the diagram: the box-drawing characters make indentation an
/// unreliable guide to nesting, and a resolver built on it produced confident nonsense (`duckdb/pricing/feed/…`)
/// when I tried. Asking "does this filename exist anywhere under `server/src`" is the strongest claim that can
/// be made robustly — it catches a renamed or deleted module, and would miss one merely moved between
/// directories.
#[test]
fn every_module_the_project_map_names_exists() {
    let repo = repo_root();
    let map = std::fs::read_to_string(repo.join("CLAUDE.md")).expect("CLAUDE.md is committed");
    let start = map
        .find("### Server (`server/src/`)")
        .expect("the map has a server section");
    let end = map[start..]
        .find("### Web (`web/src/`)")
        .map_or(map.len(), |at| start + at);
    let block = &map[start..end];

    let listing = Command::new("git")
        .args(["ls-files", "server/src"])
        .current_dir(repo)
        .output()
        .expect("git is available in a git checkout");
    let tracked: Vec<String> = String::from_utf8_lossy(&listing.stdout)
        .lines()
        .map(str::to_string)
        .collect();
    let files: BTreeSet<&str> = tracked
        .iter()
        .map(|f| f.rsplit('/').next().unwrap_or(f))
        .collect();
    let dirs: BTreeSet<&str> = tracked
        .iter()
        .flat_map(|f| f.split('/').skip(2))
        .filter(|part| !part.ends_with(".rs"))
        .collect();

    let mut missing: Vec<String> = Vec::new();
    let mut checked = 0usize;
    for token in block.split_whitespace() {
        let name = token.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '.' && c != '_');
        if name.is_empty() {
            continue;
        }
        if let Some(dir) = token.strip_suffix('/') {
            // Segment by segment: the map writes `api/routes/` as one token, and comparing that against a
            // set of single segments reports a directory that plainly exists.
            for segment in dir.split('/') {
                let segment =
                    segment.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '_');
                if segment.is_empty() {
                    continue;
                }
                checked += 1;
                if !dirs.contains(segment) {
                    missing.push(format!("{segment}/"));
                }
            }
        } else if name.ends_with(".rs") {
            checked += 1;
            if !files.contains(name) {
                missing.push(name.to_string());
            }
        }
    }

    assert!(
        checked > 25,
        "only checked {checked} names - the map section was not parsed"
    );
    assert!(
        missing.is_empty(),
        "CLAUDE.md's project map names {} thing(s) that do not exist under server/src:\n  {}",
        missing.len(),
        missing.join("\n  ")
    );
}

/// Every `dir/file.rs` path `CLAUDE.md` cites in prose resolves to a real file.
///
/// The tree-diagram check above is by **name**, because box-drawing characters make indentation an unreliable
/// guide to nesting — so it passes for a module that merely moved. That is not a hypothetical: the defect this
/// pins was `sideml/pipeline.rs`, and `pipeline.rs` does exist, under `domain/traces/`. The name check could
/// not see it and neither could I until a mutation showed the guard was weaker than its wording.
///
/// A cited path is position-bearing, so it can be checked as a **suffix** of a tracked path without parsing any
/// layout: `sideml/normalize.rs` matches `server/src/domain/sideml/normalize.rs` and `sideml/pipeline.rs`
/// matches nothing.
#[test]
fn every_module_path_the_docs_cite_resolves() {
    let repo = repo_root();
    let listing = Command::new("git")
        .args(["ls-files", "server/src"])
        .current_dir(repo)
        .output()
        .expect("git is available in a git checkout");
    let tracked: Vec<String> = String::from_utf8_lossy(&listing.stdout)
        .lines()
        .map(str::to_string)
        .collect();

    let mut checked = 0usize;
    let mut unresolved: Vec<String> = Vec::new();
    for doc in ["CLAUDE.md", "CONTRIBUTING.md"] {
        let text = std::fs::read_to_string(repo.join(doc)).unwrap_or_default();
        for cited in text.split('`').skip(1).step_by(2) {
            // A path with at least one directory and a Rust file at the end. Anything else is prose.
            if !cited.ends_with(".rs") || !cited.contains('/') || cited.contains(' ') {
                continue;
            }
            // Paths given from the repository root are checked whole; the rest as suffixes.
            let matches = if cited.starts_with("server/") {
                tracked.iter().any(|f| f == cited)
            } else {
                tracked.iter().any(|f| f.ends_with(&format!("/{cited}")))
            };
            checked += 1;
            if !matches {
                unresolved.push(format!("{doc}: {cited}"));
            }
        }
    }

    assert!(checked > 15, "only checked {checked} cited paths");
    assert!(
        unresolved.is_empty(),
        "{} cited module path(s) resolve to nothing:\n  {}",
        unresolved.len(),
        unresolved.join("\n  ")
    );
}
