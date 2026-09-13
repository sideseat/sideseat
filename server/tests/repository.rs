//! Invariants about the **repository**, not about the server.
//!
//! An integration test rather than a module under `src/`: these are about what the tree contains — which
//! manifests exist, what a committed fixture carries — and the first place a newcomer looks for that is not
//! `server/src/domain/`. The dependabot check lived inside an 8,500-line carrier-rules test file, which is
//! where it was written rather than where it belongs.
//!
//! What deliberately stays in `src/`: the two checks over the *architecture* diagrams in
//! `docs/engineering/`. They read `embedded_sources()` and the rule plans, which are crate-private, so an
//! integration test cannot see them at all. The tree-diagram check below needs nothing but the tree.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::Command;

/// The repository root, from this test file's own location.
fn repo_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the crate sits in the repository")
}

/// The comment text of a Rust file, one entry per line that carries any, numbered from 1.
///
/// **One lexical pass**, because two phases cannot agree about which construct encloses which. Blanking string
/// literals first and then looking for comment openers was the previous shape, and it silently lost citations
/// three ways: a comment that put its subject in double quotes had it blanked before the comment was even
/// recognised (the example cannot be written literally here — this check reads its own file); a lifetime
/// (`&'a str`) opened a character literal that never closed, blanking the rest of the line including any
/// trailing comment; and an apostrophe in prose did the same. Handling comments and literals in one state
/// machine is the only form where "inside a comment, a quote is inert" and "inside a string, `/*` is inert" are
/// both true.
///
/// A `'` is a character literal only when a closing one follows within an escape's reach; otherwise it is a
/// lifetime or an apostrophe and is ordinary text. Raw strings carry their hash count, so `r#"…"#` ends where
/// Rust says it does rather than at the first quote.
fn rust_commentary(text: &str) -> Vec<(usize, String)> {
    enum Mode {
        Code,
        Line,
        Block,
        Str,
        Char,
        Raw(usize),
    }

    let chars: Vec<char> = text.chars().collect();
    let mut out: Vec<(usize, String)> = Vec::new();
    let mut buffer = String::new();
    let mut mode = Mode::Code;
    let mut line = 1usize;
    let mut i = 0usize;
    while i < chars.len() {
        let c = chars[i];
        if c == '\n' {
            if !buffer.trim().is_empty() {
                out.push((line, std::mem::take(&mut buffer)));
            }
            buffer.clear();
            if matches!(mode, Mode::Line) {
                mode = Mode::Code;
            }
            line += 1;
            i += 1;
            continue;
        }
        let next = chars.get(i + 1).copied();
        match mode {
            Mode::Code => {
                if c == '/' && next == Some('/') {
                    mode = Mode::Line;
                    i += 2;
                } else if c == '/' && next == Some('*') {
                    mode = Mode::Block;
                    i += 2;
                } else if (c == 'r' || c == 'b') && matches!(next, Some('"') | Some('#')) {
                    let mut hashes = 0usize;
                    while chars.get(i + 1 + hashes) == Some(&'#') {
                        hashes += 1;
                    }
                    if chars.get(i + 1 + hashes) == Some(&'"') {
                        mode = Mode::Raw(hashes);
                        i += 2 + hashes;
                    } else {
                        i += 1;
                    }
                } else if c == '"' {
                    mode = Mode::Str;
                    i += 1;
                } else if c == '\'' {
                    // A character literal closes within four characters; anything longer is a lifetime.
                    let closes = (1..=4).any(|ahead| chars.get(i + ahead) == Some(&'\''));
                    if closes {
                        mode = Mode::Char;
                    }
                    i += 1;
                } else {
                    i += 1;
                }
            }
            Mode::Line => {
                buffer.push(c);
                i += 1;
            }
            Mode::Block => {
                if c == '*' && next == Some('/') {
                    mode = Mode::Code;
                    i += 2;
                } else {
                    buffer.push(c);
                    i += 1;
                }
            }
            Mode::Str | Mode::Char => {
                if c == '\\' {
                    i += 2;
                } else if (matches!(mode, Mode::Str) && c == '"')
                    || (matches!(mode, Mode::Char) && c == '\'')
                {
                    mode = Mode::Code;
                    i += 1;
                } else {
                    i += 1;
                }
            }
            Mode::Raw(hashes) => {
                if c == '"' && (1..=hashes).all(|ahead| chars.get(i + ahead) == Some(&'#')) {
                    mode = Mode::Code;
                    i += 1 + hashes;
                } else {
                    i += 1;
                }
            }
        }
    }
    if !buffer.trim().is_empty() {
        out.push((line, buffer));
    }
    out
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
/// because it is trusted. Both names *and* their parentage are checked, so a directory drawn under the wrong
/// branch is a failure — that is what "the map is right" means, and the first version accepted it.
///
/// The diagrams are **found**, not listed: checking a named pair was the first version, and the day it passed,
/// the public documentation's copy of the same map was stale in the same way. A block qualifies when its stated
/// subject — its own first line, or the nearest heading naming a directory — resolves to a real directory of
/// this repository. Box drawing alone does not qualify it: the integration pages draw **span hierarchies** with
/// the same characters, the ingestion notes draw concepts, and the storage reference draws a runtime data
/// directory that is not in the repository at all. A root that is spelled as a path and resolves to nothing is
/// a failure rather than a skip, which is what catches a renamed root taking the whole check quiet with it.
///
/// Names are resolved as **whole paths**, reconstructed from the diagram's own indentation. A name-only version
/// came first and was too weak in a way its wording concealed: it accepted `topics/` drawn under the wrong
/// branch, and the citation check it deferred to cannot recover a hierarchy from a bare directory name, so
/// between them the parentage went unchecked. Every map here indents in exact four-column steps, so the tree is
/// recoverable — and a prefix that is not a multiple of four is reported rather than guessed at, since that is
/// the only thing that would make the reconstruction unsound.
///
/// A name that resolves to nothing tracked is accepted when **git ignores it**, which is committed information.
/// The JavaScript examples' map documents its `output/` as gitignored, so refusing it would demand the map lie —
/// but the first fix asked the *filesystem*, and that directory does not exist in a clean checkout: the test
/// passed only because this machine had built the samples once, and would have failed in CI on a green tree.
/// A guard whose answer depends on local state is worse than none, because it teaches everyone to disbelieve it.
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
    let mut unresolved_paths: Vec<(String, usize, String, bool)> = Vec::new();
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
            // Extensions the root actually holds, so a `.rs` map and a `.ts` one are each checked on their own
            // vocabulary and a version number in a comment is not mistaken for a filename.
            let extensions: BTreeSet<&str> = under
                .iter()
                .filter_map(|f| f.rsplit_once('.').map(|(_, ext)| ext))
                .collect();

            // One entry per line, at the depth its indentation states.
            let mut branch: Vec<String> = Vec::new();
            for (offset, line) in block.iter().enumerate() {
                let Some(connector) = line.find("├── ").or_else(|| line.find("└── "))
                else {
                    continue;
                };
                let indent = line[..connector].chars().count();
                if !indent.is_multiple_of(4) {
                    missing.push(format!(
                        "{doc}:{}: indented {indent} columns, which is not a whole number of levels - the \
                         parentage cannot be read from it",
                        start + offset + 1
                    ));
                    continue;
                }
                let depth = indent / 4;
                // A jump past the next level is not a deeper entry, it is an unreadable one: nothing states
                // what the skipped level was, and truncating to a shorter branch would resolve the entry as
                // though it sat one level up - a confident answer to a question the diagram did not ask.
                if depth > branch.len() {
                    missing.push(format!(
                        "{doc}:{}: jumps from level {} to level {depth} - no entry states the level in \
                         between, so its parentage is unreadable",
                        start + offset + 1,
                        branch.len()
                    ));
                    continue;
                }
                let entry = line[connector + "├── ".len()..]
                    .split('#')
                    .next()
                    .unwrap_or_default()
                    .trim();
                if entry.is_empty() {
                    continue;
                }
                branch.truncate(depth);

                let is_dir = entry.ends_with('/');
                let name = entry.trim_end_matches('/');
                let named = format!("{root}/{}{name}", {
                    let prefix = branch.join("/");
                    if prefix.is_empty() {
                        String::new()
                    } else {
                        format!("{prefix}/")
                    }
                });
                if is_dir {
                    branch.push(name.to_string());
                }

                // A file is only checkable when the root's own vocabulary says it is one; anything else in a
                // map is prose.
                if !is_dir
                    && !name
                        .rsplit_once('.')
                        .is_some_and(|(_, e)| extensions.contains(e))
                {
                    continue;
                }
                checked += 1;
                let exists = if is_dir {
                    tracked.iter().any(|f| f.starts_with(&format!("{named}/")))
                } else {
                    tracked.iter().any(|f| **f == named)
                };
                if !exists {
                    unresolved_paths.push((doc.clone(), start + offset + 1, named, is_dir));
                }
            }
        }
    }

    // A name nothing tracks may still be one git is told to ignore, which the map may legitimately document.
    // Asked of git rather than of the filesystem: `examples/javascript/output/` exists only on a machine that
    // has run the samples, so a filesystem answer made this test pass here and fail on a clean checkout.
    if !unresolved_paths.is_empty() {
        // A directory is asked about **with its trailing slash**. A directory-only pattern (`output/`) matches a
        // bare path only when git can see that the path *is* a directory - which it cannot for one that does not
        // exist, which is precisely the clean-checkout case this has to answer.
        let candidates = unresolved_paths
            .iter()
            .map(|(_, _, path, is_dir)| {
                if *is_dir {
                    format!("{path}/")
                } else {
                    path.clone()
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        // `-v`, and the source is **verified to be a committed `.gitignore`**. Plain `check-ignore` also
        // consults `.git/info/exclude` and the user's global excludes, so a name missing from the tree could
        // have been accepted here because of one machine's configuration - the same class of defect as asking
        // the filesystem, one step further out.
        let ignored = Command::new("git")
            .args(["check-ignore", "-v", "--stdin"])
            .current_dir(repo)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .and_then(|mut child| {
                use std::io::Write;
                child
                    .stdin
                    .take()
                    .expect("stdin was piped")
                    .write_all(candidates.as_bytes())?;
                child.wait_with_output()
            })
            .map(|out| {
                String::from_utf8_lossy(&out.stdout)
                    .lines()
                    .filter_map(|entry| {
                        // `<source>:<line>:<pattern>\t<pathname>`
                        let (rule, path) = entry.rsplit_once('\t')?;
                        let source = rule.split(':').next()?;
                        tracked
                            .iter()
                            .any(|f| f == source)
                            .then(|| path.to_string())
                    })
                    .collect::<BTreeSet<String>>()
            })
            .unwrap_or_default();

        for (doc, line, path, is_dir) in unresolved_paths {
            let slash = if is_dir { "/" } else { "" };
            let asked = format!("{path}{slash}");
            if ignored.contains(&asked) {
                continue;
            }
            missing.push(format!("{doc}:{line}: {asked}"));
        }
    }

    assert!(diagrams >= 2, "only found {diagrams} tree diagram(s)");
    assert!(
        checked > 50,
        "only checked {checked} names - the diagrams were not parsed"
    );
    assert!(
        missing.is_empty(),
        "{} name(s) in a documentation tree diagram do not exist at the place it draws them:\n  {}",
        missing.len(),
        missing.join("\n  ")
    );
}

/// `CONTRIBUTING.md`'s project structure names every top-level directory, and only real ones.
///
/// It is the first thing a contributor reads, and it is a plain list rather than a tree, so the diagram check
/// does not see it — which is how it came to claim `server/` held "Cargo.toml, src/, tests/, assets/ — nothing
/// else" while `proptest-regressions/` sat there tracked, and then a `build.rs` joined it.
///
/// **Both directions**, because they fail differently and only one of them is visible to a reader. A named
/// directory that does not exist sends someone looking for it; an existing directory nobody named is a part of
/// the repository the introduction denies, which is how a grab-bag starts. Hidden directories are excluded:
/// `.github/` and `.githooks/` are conventions a contributor already knows, and listing them would say nothing.
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
}

/// Every citation of a Rust module by directory and filename, anywhere in the repository, resolves to a real
/// file — in prose and in source comments alike.
///
/// The diagram check above covers a map's own drawing; this one covers every path named in **prose**, which no
/// diagram contains. The two overlap nowhere: a citation is a claim made in a sentence, and there were three
/// live stale ones — the module this pins is `normalize.rs`, whose former name still exists as a *different*
/// file under `domain/traces/`, so any check asking only "does this filename exist" saw a match where the
/// citation was wrong.
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
                if !cited.ends_with(".rs") || !cited.contains('/') {
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
