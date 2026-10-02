
#[test]
fn every_tree_diagram_names_things_that_exist() {
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

    let mut missing: Vec<String> = Vec::new();
    let mut unresolved_paths: Vec<(String, usize, String, bool)> = Vec::new();
    let mut diagrams = 0usize;
    let mut uncovered_diagrams: Vec<String> = Vec::new();
    // Markdown, and unlike the extension allowlists this file has had to remove, this one was **measured**:
    // six Rust files draw box-drawing diagrams in doc comments and not one of them is a directory tree of this
    // repository - a call-dispatch sketch, three boxed tables, a data-flow arrow, and the *runtime* storage
    // layout, which the qualification below excludes from Markdown too for exactly the same reason. A detector
    // for "a tree diagram somewhere this cannot read" was written and reverted: matching box drawing alone, it
    // accused all six, which is the false accusation the qualification exists to prevent. Widening the reader
    // instead would need a `///` prefix and a docstring's own indent to survive the four-column
    // reconstruction, and there is nothing yet to read.
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
                // A multi-segment path is a claim about this repository, so failing to resolve is an error - it
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

            let (entries, errors) = parse_tree_block(&root, block, &extensions);
            for (offset, error) in errors {
                missing.push(format!("{doc}:{}: {error}", start + offset + 1));
            }
            if entries.is_empty() {
                uncovered_diagrams.push(format!("{doc}:{}", open + 1));
            }
            for entry in entries {
                let exists = if entry.is_dir {
                    tracked
                        .iter()
                        .any(|file| file.starts_with(&format!("{}/", entry.path)))
                } else {
                    tracked.iter().any(|file| **file == entry.path)
                };
                if !exists {
                    unresolved_paths.push((
                        doc.clone(),
                        start + entry.offset + 1,
                        entry.path,
                        entry.is_dir,
                    ));
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
        // `-v` identifies the matching ignore source; only committed `.gitignore` files may justify a
        // generated path, never machine-local excludes.
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
        uncovered_diagrams.is_empty(),
        "{} qualified tree diagram(s) yielded no checkable path:\n  {}",
        uncovered_diagrams.len(),
        uncovered_diagrams.join("\n  ")
    );
    assert!(
        missing.is_empty(),
        "{} name(s) in a documentation tree diagram do not exist at the place it draws them:\n  {}",
        missing.len(),
        missing.join("\n  ")
    );
}

/// Each package's `engines` and its lockfile's copy of it agree.
///
/// npm writes the root manifest's `engines` into `package-lock.json` and does not refresh it until an install
/// runs. `make node-floor` derives supported Node versions from the lockfiles, so the recorded constraint must
/// match the manifest.
///
/// The remedy is `npm install --package-lock-only` in that package.
#[test]
fn every_lockfile_carries_its_manifests_engines() {
    let repo = repo_root();
    let listing = Command::new("git")
        .args(["ls-files"])
        .current_dir(repo)
        .output()
        .expect("git is available in a git checkout");

    let tracked: BTreeSet<String> = String::from_utf8_lossy(&listing.stdout)
        .lines()
        .map(str::to_string)
        .collect();

    let engines_of = |value: &serde_json::Value| -> Option<String> {
        value
            .get("engines")?
            .get("node")?
            .as_str()
            .map(str::to_string)
    };

    let mut checked = 0usize;
    let mut disagree: Vec<String> = Vec::new();
    for manifest in tracked
        .iter()
        .filter(|f| f.ends_with("package.json"))
        .filter(|f| !f.contains("/node_modules/"))
    {
        let dir = manifest.trim_end_matches("package.json");
        let lock_path = format!("{dir}package-lock.json");
        let Ok(lock_text) = std::fs::read_to_string(repo.join(&lock_path)) else {
            continue;
        };
        let lock: serde_json::Value = serde_json::from_str(&lock_text).expect("a lockfile is JSON");
        let packages = lock
            .get("packages")
            .and_then(serde_json::Value::as_object)
            .expect("a lockfile has a packages map");

        // Which entries describe a package **in this repository**, and where its manifest is. Two spellings
        // reach one package and only one of them carries the metadata: npm files a `file:` dependency's
        // manifest under its *path* (`packages["thing"]` or `packages["../../sdk/js"]`) and leaves
        // `node_modules/<name>` holding only `resolved` and `link`. Deriving the path from the
        // `node_modules/<name>` key is therefore wrong whenever the directory is not named after the package -
        // which is the ordinary case for `file:./thing`. So the link entry is *followed* through its `resolved`
        // value, which is the only thing that states the path.
        let mut local: BTreeMap<String, String> = BTreeMap::new();
        for (key, entry) in packages {
            if key.is_empty() {
                local.insert(String::new(), manifest.to_string());
                continue;
            }
            let resolved = entry.get("resolved").and_then(serde_json::Value::as_str);
            let path = match resolved {
                // A link: the path is the value, and the metadata lives under that same path.
                Some(value) if !value.starts_with("http") => {
                    Some(value.trim_start_matches("file:"))
                }
                // A metadata entry for a local package: the key *is* the path.
                _ if key.starts_with("../") || key.starts_with("./") || !key.contains('/') => {
                    (!key.starts_with("node_modules")).then_some(key.as_str())
                }
                _ => None,
            };
            let Some(path) = path else { continue };
            // A path that climbs above the repository root is a machine-local dependency.
            let Some(joined) = join_relative(dir, path) else {
                disagree.push(format!(
                    "{lock_path} resolves `{key}` to `{path}`, which climbs above the repository root - a \
                     local dependency outside the tree is one only this machine has"
                ));
                continue;
            };
            let candidate = format!("{joined}/package.json");
            // **Tracked**, not merely present: an untracked manifest is the same machine-specific dependency
            // by another route, and it is what a fresh clone would not have.
            if tracked.contains(&candidate) {
                // Keyed by the path, so the link and its metadata entry collapse to one comparison.
                local.insert(path.to_string(), candidate);
            } else if resolved.is_some_and(|v| !v.starts_with("http")) {
                let why = if repo.join(&candidate).exists() {
                    "exists but is not tracked"
                } else {
                    "does not exist"
                };
                disagree.push(format!(
                    "{lock_path} resolves `{key}` to `{path}`, but {candidate} {why}"
                ));
            }
        }

        for (path, source) in local {
            let declared: Option<String> = std::fs::read_to_string(repo.join(&source))
                .ok()
                .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
                .and_then(|value| engines_of(&value));
            // The entry that carries the metadata is keyed by the path, empty for the root.
            let recorded = packages.get(&path).and_then(engines_of);
            checked += 1;
            if declared != recorded {
                let entry = if path.is_empty() {
                    String::new()
                } else {
                    format!(" (entry `{path}`)")
                };
                disagree.push(format!(
                    "{source} says {declared:?} while {lock_path}{entry} says {recorded:?} - run \
                     `npm install --package-lock-only` in {dir}"
                ));
            }
        }
    }

    assert!(checked >= 3, "only compared {checked} manifest/lock pairs");
    assert!(
        disagree.is_empty(),
        "{} package(s) whose lockfile disagrees with their manifest:\n  {}",
        disagree.len(),
        disagree.join("\n  ")
    );
}

/// Every command that resolves dependencies passes `--locked`.
///
/// A lockfile identifies the reviewed dependency graph. Commands that consume that graph must not silently
/// rewrite it.
///
/// **A line that could be pasted and run**, which is the distinction that makes this checkable: a command at the
/// start of a line (after a make recipe's `@`, a `(cd … &&` prefix, or an `echo` that prints instructions) or
/// inside a fenced block is something a reader or a shell executes. A command named mid-sentence - "lint with
/// `cargo clippy`" - is a reference, and locking prose would be noise rather than rigour.
///
/// The exceptions are commands whose **purpose** is to write the lockfile, and they are named rather than
/// pattern-matched: `uv lock`, `uv add`, `cargo update`, and the installers (`cargo install`, `npm install`),
/// which resolve something other than this workspace.
#[test]
fn every_resolving_command_is_locked() {
    let repo = repo_root();
    let listing = Command::new("git")
        .args(["ls-files"])
        .current_dir(repo)
        .output()
        .expect("git is available in a git checkout");

    const RESOLVING: [&str; 12] = [
        "cargo build",
        "cargo test",
        "cargo clippy",
        "cargo check",
        "cargo run",
        "cargo zigbuild",
        "cargo tarpaulin",
        "cargo fetch",
        // Metadata resolves the workspace graph and can update its lockfile.
        "cargo metadata",
        "uv sync",
        "uv run",
        "uv export",
    ];
    let mut unlocked: Vec<String> = Vec::new();
    let mut checked = 0usize;
    for file in String::from_utf8_lossy(&listing.stdout)
        .lines()
        // Every tracked text file is eligible; binary payloads are excluded by `is_text`.
        .filter(|f| is_text(&repo.join(f)))
    {
        let text = std::fs::read_to_string(repo.join(file)).unwrap_or_default();
        // Rust contributes pasteable commands from commentary, not command names constructed as code.
        let rust = file.ends_with(".rs");
        let commentary: Vec<(usize, String)>;
        let numbered: Vec<(usize, &str)> = if rust {
            commentary = rust_commentary(&text);
            commentary
                .iter()
                // Strip doc markers before detecting Markdown fences.
                .map(|(number, line)| {
                    (number - 1, line.trim_start_matches(['/', '!']).trim_start())
                })
                .collect()
        } else {
            text.lines().enumerate().collect()
        };
        let mut fenced = false;
        for (number, line) in numbered {
            if line.trim_start().starts_with("```") {
                fenced = !fenced;
                continue;
            }
            // A shell comment containing only a command is pasteable; an inline command reference is not.
            let bare = line.trim().trim_start_matches(['@', '\t']).trim_start();
            let line = match bare.strip_prefix('#') {
                Some(comment) if !fenced => {
                    let comment = comment.trim_start_matches('#').trim();
                    if RESOLVING.iter().any(|c| comment.starts_with(c)) {
                        comment
                    } else {
                        continue;
                    }
                }
                _ => line,
            };
            // What a shell would see: the recipe marker, a subshell, and an `echo` of instructions all leave a
            // command at the front of what remains.
            let mut runnable = line.trim();
            for prefix in ['@', '-', '(', '\t'] {
                runnable = runnable.trim_start_matches(prefix).trim_start();
            }
            // A whole backticked line is pasteable, while a backticked name inside prose is only a reference.
            let mut whole_line_command = false;
            if let Some(inner) = runnable
                .strip_prefix('`')
                .and_then(|rest| rest.strip_suffix('`'))
                .filter(|inner| !inner.contains('`'))
            {
                runnable = inner.trim();
                whole_line_command = true;
            }
            // Rust commentary counts commands only in fences or whole backticked lines; Markdown also permits
            // a command at the start of a line.
            if rust && !fenced && !whole_line_command {
                continue;
            }
            // Remove leading environment assignments before identifying the command.
            while let Some((head, rest)) = runnable.split_once(' ') {
                let assignment = head.split_once('=').is_some_and(|(name, _)| {
                    !name.is_empty()
                        && name
                            .chars()
                            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
                });
                if !assignment {
                    break;
                }
                runnable = rest.trim_start();
            }
            for lead in [
                "cd ",
                "echo \"",
                "echo '",
                "$$_secrets_env ",
                "timeout 900 ",
                "if ",
            ] {
                if let Some(rest) = runnable.strip_prefix(lead) {
                    runnable = rest.trim_start_matches(|c: char| c != ' ').trim_start();
                    let _ = rest;
                }
            }
            // Judge every shell segment independently after the same normalization.
            let candidate = runnable;
            for segment in candidate
                .split("&&")
                .flat_map(|part| part.split(';'))
                .flat_map(|part| part.split("||"))
                .map(str::trim)
            {
                // Watch runners carry the delegated Cargo command inside an argument.
                let delegated =
                    segment.starts_with("cargo watch") || segment.starts_with("watchexec");
                let Some(command) = RESOLVING.iter().find(|c| {
                    segment.starts_with(**c)
                        || (delegated
                            && c.starts_with("cargo ")
                            && segment.contains(c.strip_prefix("cargo ").unwrap_or(c)))
                }) else {
                    continue;
                };
                checked += 1;
                if !segment.contains("--locked") {
                    unlocked.push(format!(
                        "{file}:{}: `{command}` without `--locked`",
                        number + 1
                    ));
                }
            }
        }
    }

    assert!(
        checked > 40,
        "only {checked} resolving commands found - the scan is wrong, not the tree"
    );
    assert!(
        unlocked.is_empty(),
        "{} command(s) can rewrite a lockfile, which makes every `--locked` check downstream a statement \
         about one machine:\n  {}",
        unlocked.len(),
        unlocked.join("\n  ")
    );
}

/// A sample suite that declares a collision-free alias is invoked by it everywhere.
///
/// Framework packages may install a CLI with the suite's short name. Suites that declare a
/// `telemetry-<name>` alias must use it consistently so the sample entry point wins.
#[test]
fn every_aliased_sample_suite_is_invoked_by_its_alias() {
    let repo = repo_root();
    let listing = Command::new("git")
        .args(["ls-files", "examples/python"])
        .current_dir(repo)
        .output()
        .expect("git is available in a git checkout");

    let mut aliased: Vec<String> = Vec::new();
    let mut manifests = 0usize;
    for manifest in String::from_utf8_lossy(&listing.stdout)
        .lines()
        .filter(|f| f.ends_with("pyproject.toml"))
    {
        let Some(suite) = manifest.split('/').nth(2) else {
            continue;
        };
        manifests += 1;
        let text = std::fs::read_to_string(repo.join(manifest)).unwrap_or_default();
        let Some(scripts) = text.split("[project.scripts]").nth(1) else {
            continue;
        };
        let scripts = scripts.split("\n[").next().unwrap_or(scripts);
        let declares = |name: &str| {
            scripts
                .lines()
                .any(|l| l.trim().starts_with(&format!("{name} = ")))
        };
        if declares(suite) && declares(&format!("telemetry-{suite}")) {
            aliased.push(suite.to_string());
        }
    }
    // Suites on the scenario harness run `sample`, which no framework installs, so they declare no
    // alias and the aliased set shrinks as suites move. What must not shrink is the scan itself.
    assert!(
        manifests >= 10,
        "read only {manifests} suite manifest(s) - the scan is wrong, not the tree"
    );

    // Discover callers across tracked text instead of maintaining a filename list.
    let all = Command::new("git")
        .args(["ls-files"])
        .current_dir(repo)
        .output()
        .expect("git is available in a git checkout");
    let mut colliding: Vec<String> = Vec::new();
    for caller in String::from_utf8_lossy(&all.stdout)
        .lines()
        // Text detection also covers extensionless scripts and package-manager command fields.
        .filter(|f| !f.starts_with("server/tests/fixtures/"))
        // Rust commentary documents these patterns but does not invoke sample suites.
        .filter(|f| !f.ends_with(".rs"))
        .filter(|f| is_text(&repo.join(f)))
    {
        let text = std::fs::read_to_string(repo.join(caller)).unwrap_or_default();
        for (number, line) in text.lines().enumerate() {
            for suite in &aliased {
                // The two shapes these callers use, both naming the suite and then the entry point.
                // Both the absolute and the `--directory`-relative spelling, and the `run_py` helper: a
                // README inside `examples/python` writes `--directory strands strands`, with no path prefix.
                for pattern in [
                    format!("examples/python/{suite} {suite}"),
                    format!("--directory {suite} {suite}"),
                    format!("run_py {suite} {suite}"),
                ] {
                    if line.contains(&pattern) {
                        colliding.push(format!(
                            "{caller}:{}: invokes `{suite}` where the framework's own CLI wins - use \
                             `telemetry-{suite}`",
                            number + 1
                        ));
                    }
                }
            }
        }
    }
    assert!(
        colliding.is_empty(),
        "{} invocation(s) name a colliding entry point:\n  {}",
        colliding.len(),
        colliding.join("\n  ")
    );
}

/// Every uv project requires the same resolver, and CI installs exactly that one.
///
/// `required-version` is what makes the pin real: uv refuses to run when it does not match, which no Makefile
/// check can do for a `uv` invoked directly.
///
/// Each project's `[tool.uv]` replaces rather than merges the `uv.toml` above it, so every project must restate
/// the same requirement.
#[test]
fn every_uv_project_requires_the_same_resolver() {
    let repo = repo_root();
    let listing = Command::new("git")
        .args(["ls-files"])
        .current_dir(repo)
        .output()
        .expect("git is available in a git checkout");

    let mut declared: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut misplaced: Vec<String> = Vec::new();
    for file in String::from_utf8_lossy(&listing.stdout)
        .lines()
        // Only the root `uv.toml` supplies the repository-wide default.
        .filter(|f| f.ends_with("pyproject.toml") || *f == "uv.toml")
    {
        let text = std::fs::read_to_string(repo.join(file)).unwrap_or_default();
        // uv reads `required-version` from `[tool.uv]` in manifests and from the top level of `uv.toml`.
        let wanted = if file.ends_with("uv.toml") {
            ""
        } else {
            "tool.uv"
        };
        let mut section = String::new();
        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with('#') {
                continue;
            }
            if let Some(name) = trimmed.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
                section = name.trim_matches('"').to_string();
                continue;
            }
            if let Some(value) = trimmed.strip_prefix("required-version") {
                if section != wanted {
                    misplaced.push(format!(
                        "{file}: under `[{section}]`, where uv does not read it"
                    ));
                    continue;
                }
                let value = value
                    .trim_start_matches([' ', '='])
                    .trim()
                    .trim_matches('"');
                declared
                    .entry(value.to_string())
                    .or_default()
                    .push(file.to_string());
            }
        }
    }

    assert!(
        repo.join("uv.toml").exists(),
        "there is no root `uv.toml`: the projects that do not restate `required-version` inherit the pin from \
         it, so without it they accept any resolver"
    );
    assert!(
        misplaced.is_empty(),
        "{} `required-version` declaration(s) in a section uv does not read - it takes them from `[tool.uv]` in \
         a manifest and from the top level of a `uv.toml`:\n  {}",
        misplaced.len(),
        misplaced.join("\n  ")
    );

    // The floor is derived: every manifest that configures uv must declare it, and so must the root `uv.toml`.
    // `>= 10` was a number I chose, which cannot notice a project appearing or disappearing.
    let configuring = String::from_utf8_lossy(&listing.stdout)
        .lines()
        .filter(|f| f.ends_with("pyproject.toml"))
        .filter(|f| {
            std::fs::read_to_string(repo.join(f))
                .unwrap_or_default()
                .contains("[tool.uv")
        })
        .count();
    assert!(
        declared.values().map(Vec::len).sum::<usize>() > configuring,
        "{} `required-version` declarations for {configuring} uv-configuring project(s) plus the root file - \
         a project with its own `[tool.uv]` and no declaration is one a differently-versioned uv can write \
         lockfiles for: {declared:?}",
        declared.values().map(Vec::len).sum::<usize>()
    );
    assert!(
        declared.len() == 1,
        "the resolver is required at {} different versions:\n  {}",
        declared.len(),
        declared
            .iter()
            .map(|(value, files)| format!("`{value}` in {}", files.join(", ")))
            .collect::<Vec<_>>()
            .join("\n  ")
    );

    // And every project that configures uv at all must say it, since its own section replaces the root file.
    let mut silent: Vec<String> = Vec::new();
    for file in String::from_utf8_lossy(&listing.stdout)
        .lines()
        .filter(|f| f.ends_with("pyproject.toml"))
    {
        let text = std::fs::read_to_string(repo.join(file)).unwrap_or_default();
        // Require a declaration on a non-comment line, not merely the phrase in prose.
        let declares = text.lines().any(|line| {
            let trimmed = line.trim();
            !trimmed.starts_with('#') && trimmed.starts_with("required-version")
        });
        if text.contains("[tool.uv") && !declares {
            silent.push(file.to_string());
        }
    }
    assert!(
        silent.is_empty(),
        "{} project(s) configure uv without requiring a version, so the root `uv.toml` does not reach them:\n  {}",
        silent.len(),
        silent.join("\n  ")
    );

    // CI installs the version the tree requires, or the pin holds in one place and not the other.
    let required = declared
        .keys()
        .next()
        .expect("one value")
        .trim_start_matches("==")
        .to_string();
    let workflow = std::fs::read_to_string(repo.join(".github/workflows/ci.yml"))
        .expect("the workflow is committed");
    let pins = workflow.matches(&format!("version: '{required}'")).count();
    let setups = workflow.matches("astral-sh/setup-uv@").count();
    assert_eq!(
        pins, setups,
        "{setups} job(s) install uv and {pins} pin `{required}` - a job without the version input installs \
         whatever is newest, which is the resolver writing this repository's lockfiles in CI"
    );
}

/// The repository's Node requirement is stated identically everywhere it is stated.
///
/// The repository-wide range may appear in tooling and contributor documentation, but every occurrence must
/// carry one value. Independently installable examples may declare a looser package-specific range.
#[test]
fn the_node_requirement_is_stated_once() {
    let repo = repo_root();
    let listing = Command::new("git")
        .args(["ls-files"])
        .current_dir(repo)
        .output()
        .expect("git is available in a git checkout");
    let mut stated: BTreeMap<String, Vec<String>> = BTreeMap::new();
    // Discover statements from tracked text rather than maintaining a filename list.
    for file in String::from_utf8_lossy(&listing.stdout)
        .lines()
        .filter(|f| !f.starts_with("server/tests/fixtures/"))
        .filter(|f| !f.ends_with("package-lock.json"))
        .filter(|f| is_text(&repo.join(f)))
    {
        let text = std::fs::read_to_string(repo.join(file)).unwrap_or_default();
        for (number, line) in text.lines().enumerate() {
            // The exact SemVer shape used for the disjoint supported majors:
            // `^22.22.0 || ^24.0.0 || >=26.0.0`.
            let words: Vec<&str> = line.split_whitespace().collect();
            for range in words.windows(5) {
                let punctuation = |c: char| matches!(c, ',' | ';' | ')' | '.' | '"' | '\'');
                let first = range[0].trim_end_matches(punctuation);
                let second = range[2].trim_end_matches(punctuation);
                let third = range[4].trim_end_matches(punctuation);
                let is_version = |word: &str, prefix: &str| {
                    word.strip_prefix(prefix).is_some_and(|version| {
                        let parts: Vec<&str> = version.split('.').collect();
                        parts.len() == 3
                            && parts.iter().all(|part| {
                                !part.is_empty() && part.chars().all(|c| c.is_ascii_digit())
                            })
                    })
                };
                if range[1] != "||"
                    || range[3] != "||"
                    || !is_version(first, "^")
                    || !is_version(second, "^")
                    || !is_version(third, ">=")
                {
                    continue;
                }
                stated
                    .entry(format!("{first} || {second} || {third}"))
                    .or_default()
                    .push(format!("{file}:{}", number + 1));
            }
        }
    }

    assert!(
        stated.values().map(Vec::len).sum::<usize>() >= 4,
        "found the Node requirement in {} place(s), expected at least four - the scan is wrong, not the \
         documents: {stated:?}",
        stated.values().map(Vec::len).sum::<usize>()
    );
    assert!(
        stated.len() == 1,
        "the Node requirement is stated {} different ways:\n  {}",
        stated.len(),
        stated
            .iter()
            .map(|(value, places)| format!("`{value}` at {}", places.join(", ")))
            .collect::<Vec<_>>()
            .join("\n  ")
    );
}

/// Every relative `$schema` reference in a tracked JSON file resolves.
///
/// Editors treat a declared schema as authoritative, so a relative reference must resolve from the JSON file
/// that declares it.
#[test]
fn every_relative_schema_reference_resolves() {
    let repo = repo_root();
    let listing = Command::new("git")
        .args(["ls-files", "*.json"])
        .current_dir(repo)
        .output()
        .expect("git is available in a git checkout");

    let mut checked = 0usize;
    let mut broken: Vec<String> = Vec::new();
    for file in String::from_utf8_lossy(&listing.stdout)
        .lines()
        .filter(|f| !f.contains("/node_modules/") && !f.starts_with("server/tests/fixtures/"))
    {
        let Ok(text) = std::fs::read_to_string(repo.join(file)) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        let Some(reference) = value.get("$schema").and_then(serde_json::Value::as_str) else {
            continue;
        };
        if reference.starts_with("http") {
            continue;
        }
        checked += 1;
        let dir = file.rsplit_once('/').map_or("", |(parent, _)| parent);
        match join_relative(dir, reference) {
            Some(target) if repo.join(&target).exists() => {}
            Some(target) => broken.push(format!(
                "{file}: `{reference}` resolves to {target}, which is absent"
            )),
            None => broken.push(format!(
                "{file}: `{reference}` climbs above the repository root"
            )),
        }
    }

    assert!(checked >= 3, "only checked {checked} schema references");
    assert!(
        broken.is_empty(),
        "{} schema reference(s) resolve to nothing:\n  {}",
        broken.len(),
        broken.join("\n  ")
    );
}
