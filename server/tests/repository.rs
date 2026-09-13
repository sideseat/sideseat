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

/// Every tree diagram in the repository's documentation names things that exist.
///
/// A diagram is a map handed to whoever arrives, and both maps had drifted: each still named a `topic.rs`
/// under `core/` after pub/sub moved to `data/topics/`, and `CLAUDE.md` named a `pipeline.rs` under `sideml/`
/// after that file became `normalize.rs`. A map that names a file nobody can open costs more than no map,
/// because it is trusted.
///
/// The diagrams are **found**, not listed: checking a named pair was the first version, and the day it passed,
/// the public documentation's copy of the same map was stale in the same way. A block qualifies when its stated
/// subject — its own first line, or the nearest heading naming a directory — resolves to a real directory of
/// this repository. Box drawing alone does not qualify it: the integration pages draw **span hierarchies** with
/// the same characters, the ingestion notes draw concepts, and the storage reference draws a runtime data
/// directory that is not in the repository at all. A root that is spelled as a path and resolves to nothing is
/// a failure rather than a skip, which is what catches a renamed root taking the whole check quiet with it.
///
/// Directory names are resolved on **disk**, not against `git ls-files`: the JavaScript examples' map documents
/// its `output/` as gitignored, and a check that refused it would be demanding the map lie. Filenames stay
/// tracked-only — an untracked source file has no business in a map.
///
/// Checked by **name**, not by position in the diagram: the box-drawing characters make indentation an
/// unreliable guide to nesting, and a resolver built on it produced confident nonsense (`duckdb/pricing/feed/…`)
/// when I tried. Asking "does this filename exist anywhere under this root" is the strongest claim that can be
/// made robustly — it catches a renamed or deleted module, and misses one merely moved between directories,
/// which is what [`every_module_path_cited_anywhere_resolves`] is for.
#[test]
fn every_tree_diagram_names_things_that_exist() {
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

    let mut missing: Vec<String> = Vec::new();
    let mut checked = 0usize;
    let mut diagrams = 0usize;
    for doc in tracked
        .iter()
        .filter(|f| f.ends_with(".md") || f.ends_with(".mdx"))
        .filter(|f| !f.starts_with("server/tests/fixtures/"))
    {
        let text = std::fs::read_to_string(repo.join(doc)).unwrap_or_default();
        let lines: Vec<&str> = text.lines().collect();
        let mut at = 0usize;
        while at < lines.len() {
            if !lines[at].trim_start().starts_with("```") {
                at += 1;
                continue;
            }
            let open = at;
            at += 1;
            let start = at;
            while at < lines.len() && !lines[at].trim_start().starts_with("```") {
                at += 1;
            }
            let block = &lines[start..at.min(lines.len())];
            at += 1;
            if !block.iter().any(|l| l.contains("├──") || l.contains("└──")) {
                continue;
            }

            // The subject is stated either as the block's own first line or by the nearest heading above it.
            let stated_on_first_line = block
                .first()
                .map(|l| l.trim())
                .filter(|l| l.ends_with('/') && !l.contains(' ') && !l.contains("──"))
                .map(|l| l.trim_end_matches('/').to_string());
            let root = stated_on_first_line.clone().or_else(|| {
                lines[..open].iter().rev().take(6).find_map(|l| {
                    l.split('`')
                        .find(|t| t.ends_with('/') && t.contains('/') && !t.contains(' '))
                        .map(|t| t.trim_end_matches('/').to_string())
                })
            });
            let Some(root) = root else {
                continue;
            };

            let under: Vec<&String> = tracked
                .iter()
                .filter(|f| f.starts_with(&format!("{root}/")))
                .collect();
            if under.is_empty() {
                // A multi-segment path is a claim about this repository, so failing to resolve is a defect - it
                // is how a renamed root is caught. A bare name is more likely another kind of tree entirely.
                if root.contains('/') {
                    missing.push(format!("{doc}:{}: root `{root}/` does not exist", open + 1));
                }
                continue;
            }
            diagrams += 1;
            let depth = root.split('/').count();
            let files: BTreeSet<&str> = under
                .iter()
                .map(|f| f.rsplit('/').next().unwrap_or(f))
                .collect();
            let mut dirs: BTreeSet<String> = under
                .iter()
                .flat_map(|f| f.split('/').skip(depth))
                .filter(|part| !part.contains('.'))
                .map(str::to_string)
                .collect();
            // Plus directories that exist but are not tracked, which a map may legitimately document.
            let mut frontier = vec![repo.join(&root)];
            while let Some(dir) = frontier.pop() {
                let Ok(entries) = std::fs::read_dir(&dir) else {
                    continue;
                };
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if !entry.file_type().is_ok_and(|t| t.is_dir())
                        || name.starts_with('.')
                        || matches!(name.as_str(), "node_modules" | "target" | "dist" | "build")
                    {
                        continue;
                    }
                    frontier.push(entry.path());
                    dirs.insert(name);
                }
            }
            // Extensions the root actually holds, so a `.rs` map and a `.ts` one are each checked on their own
            // vocabulary and a version number in a comment is not mistaken for a filename.
            let extensions: BTreeSet<&str> = files
                .iter()
                .filter_map(|f| f.rsplit_once('.').map(|(_, ext)| ext))
                .collect();

            for line in block
                .iter()
                .skip(usize::from(stated_on_first_line.is_some()))
            {
                // Everything after `#` is the diagram's own commentary.
                let content = line.split('#').next().unwrap_or(line);
                for token in content.split_whitespace() {
                    let token = token.trim_matches(|c: char| {
                        !c.is_ascii_alphanumeric() && c != '.' && c != '_' && c != '/' && c != '-'
                    });
                    if token.is_empty() {
                        continue;
                    }
                    if let Some(dir) = token.strip_suffix('/') {
                        // Segment by segment: a map writes `api/routes/` as one token, and comparing that
                        // against a set of single segments reports a directory that plainly exists.
                        for segment in dir.split('/') {
                            if segment.is_empty() {
                                continue;
                            }
                            checked += 1;
                            if !dirs.contains(segment) {
                                missing.push(format!("{doc}: {root}/…/{segment}/"));
                            }
                        }
                    } else if token
                        .rsplit_once('.')
                        .is_some_and(|(_, ext)| extensions.contains(ext))
                    {
                        checked += 1;
                        if !files.contains(token) {
                            missing.push(format!("{doc}: {root}/…/{token}"));
                        }
                    }
                }
            }
        }
    }

    assert!(diagrams >= 2, "only found {diagrams} tree diagram(s)");
    assert!(
        checked > 50,
        "only checked {checked} names - the diagrams were not parsed"
    );
    assert!(
        missing.is_empty(),
        "{} name(s) in a documentation tree diagram do not exist:\n  {}",
        missing.len(),
        missing.join("\n  ")
    );
}

/// Every citation of a Rust module by directory and filename, anywhere in the repository, resolves to a real
/// file — in prose and in source comments alike.
///
/// The tree-diagram check above is by **name**, because box-drawing characters make indentation an unreliable
/// guide to nesting — so it passes for a module that merely moved. That is not a hypothetical: the module this
/// pins is `normalize.rs`, whose former name still exists as a *different* file under `domain/traces/`, so the
/// name check saw a match where the citation was wrong. It took a mutation to show the guard was weaker than
/// its wording.
///
/// A cited path is position-bearing, so it can be checked without parsing any layout: root-anchored whole,
/// doc-relative against the citing file's directory, otherwise as a **suffix** of a tracked path — where
/// `sideml/normalize.rs` matches and the stale spelling matches nothing.
///
/// The documents are **derived from the tree**, not listed here, and that is the point rather than tidiness.
/// The first version of this test named two files; the same stale path was live in the *public* architecture
/// page and in a source comment, and a hand-kept inventory is exactly the shape that confirms only what its
/// author already knew. Comments are included because a comment pointing at a moved module misleads the same
/// way — one did. This file is scanned too: a check that must exempt itself is a hole, so the counter-example
/// above is phrased not to be a citation.
#[test]
fn every_module_path_cited_anywhere_resolves() {
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

    // Captured OTLP payloads are data, not documentation: whatever paths a framework recorded are its business.
    let citing: Vec<&String> = tracked
        .iter()
        .filter(|f| !f.starts_with("server/tests/fixtures/"))
        .filter(|f| f.ends_with(".md") || f.ends_with(".mdx") || f.ends_with(".rs"))
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
            return tracked.contains(&parts.join("/"));
        }
        if tracked.iter().any(|f| f == cited) {
            return true;
        }
        tracked.iter().any(|f| f.ends_with(&format!("/{cited}")))
    };

    let mut checked = 0usize;
    let mut unresolved: Vec<String> = Vec::new();
    for file in citing {
        let text = std::fs::read_to_string(repo.join(file)).unwrap_or_default();
        let is_rust = file.ends_with(".rs");
        for (number, line) in text.lines().enumerate() {
            // In Rust, only comments describe the layout; a string literal or a module path is not a citation.
            if is_rust {
                let code = line.trim_start();
                if !(code.starts_with("//") || code.starts_with('*')) {
                    continue;
                }
            }
            for token in line.split(|c: char| c.is_whitespace() || "`(),;\"'[]<>".contains(c)) {
                // A path with at least one directory and a Rust file at the end. A trailing `:line` is a
                // citation of a position in that file, so it is stripped before resolving.
                let cited = token.trim_end_matches(['.', ':']);
                let cited = cited.split(':').next().unwrap_or(cited);
                if !cited.ends_with(".rs") || !cited.contains('/') {
                    continue;
                }
                checked += 1;
                if !resolves(file, cited) {
                    unresolved.push(format!("{file}:{}: {cited}", number + 1));
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
