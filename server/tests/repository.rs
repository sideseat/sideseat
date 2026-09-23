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

use sideseat_query_sql::analytics::MIGRATED_OPERATIONS;

/// The repository root, from this test file's own location.
fn repo_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the crate sits in the repository")
}

/// Whether a tracked file is text, so a scan over "every file" can mean it.
///
/// A NUL byte is the test git itself uses. Cheaper than an extension list and, unlike one, it cannot omit the
/// extensionless hooks or the next kind of script somebody adds.
fn is_text(path: &Path) -> bool {
    std::fs::read(path).is_ok_and(|bytes| !bytes.contains(&0))
}

/// `base` (a directory, with or without a trailing slash) joined with a `relative` path, `..` segments applied.
fn join_relative(base: &str, relative: &str) -> Option<String> {
    let mut parts: Vec<&str> = base
        .trim_end_matches('/')
        .split('/')
        .filter(|p| !p.is_empty())
        .collect();
    for segment in relative.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            other => parts.push(other),
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
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
                } else if c == 'r' || (c == 'b' && next == Some('r')) {
                    // Raw only for `r"…"` and `br"…"`. A **byte** string (`b"…"`) is an ordinary escaped
                    // literal, and treating it as raw desynchronised the scanner on real code:
                    // `b"\"hello-"` closed at the escaped quote, after which the rest of the file was read
                    // in the wrong mode and the next comment was invisible.
                    let at = i + usize::from(c == 'b');
                    let mut hashes = 0usize;
                    while chars.get(at + 1 + hashes) == Some(&'#') {
                        hashes += 1;
                    }
                    if chars.get(at + 1 + hashes) == Some(&'"') {
                        mode = Mode::Raw(hashes);
                        i = at + 2 + hashes;
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
                    // A string continuation is a backslash *followed by the newline*, so skipping the escaped
                    // character in one step skipped the line count with it: every `"…\` in the file shifted
                    // every later reported line up by one, and this file's own scanners cited lines seven
                    // above the ones they had read. The finding was right and its address was not, which is
                    // the failure mode a line number exists to prevent.
                    if chars.get(i + 1) == Some(&'\n') {
                        line += 1;
                    }
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

/// Rust source with comments blanked and string/character literals preserved.
///
/// Removing comments by globally replacing their text is order-dependent: a short earlier comment can erase
/// part of a later one before the later full-line replacement runs, leaving prose that looks like SQL. This
/// lexical pass preserves byte positions with spaces and therefore cannot manufacture code from comments.
fn rust_code_without_comments(text: &str) -> String {
    #[derive(Clone, Copy)]
    enum Mode {
        Code,
        Line,
        Block(usize),
        Str,
        Char,
        Raw(usize),
    }

    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut mode = Mode::Code;
    let mut i = 0usize;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        match mode {
            Mode::Code => {
                if c == '/' && next == Some('/') {
                    out.push_str("  ");
                    mode = Mode::Line;
                    i += 2;
                } else if c == '/' && next == Some('*') {
                    out.push_str("  ");
                    mode = Mode::Block(1);
                    i += 2;
                } else if c == 'r' || (c == 'b' && next == Some('r')) {
                    let at = i + usize::from(c == 'b');
                    let mut hashes = 0usize;
                    while chars.get(at + 1 + hashes) == Some(&'#') {
                        hashes += 1;
                    }
                    if chars.get(at + 1 + hashes) == Some(&'"') {
                        for value in &chars[i..=at + 1 + hashes] {
                            out.push(*value);
                        }
                        mode = Mode::Raw(hashes);
                        i = at + 2 + hashes;
                    } else {
                        out.push(c);
                        i += 1;
                    }
                } else {
                    out.push(c);
                    if c == '"' {
                        mode = Mode::Str;
                    } else if c == '\'' {
                        let closes = (1..=4).any(|ahead| chars.get(i + ahead) == Some(&'\''));
                        if closes {
                            mode = Mode::Char;
                        }
                    }
                    i += 1;
                }
            }
            Mode::Line => {
                if c == '\n' {
                    out.push('\n');
                    mode = Mode::Code;
                } else {
                    out.push(' ');
                }
                i += 1;
            }
            Mode::Block(depth) => {
                if c == '/' && next == Some('*') {
                    out.push_str("  ");
                    mode = Mode::Block(depth + 1);
                    i += 2;
                } else if c == '*' && next == Some('/') {
                    out.push_str("  ");
                    mode = if depth == 1 {
                        Mode::Code
                    } else {
                        Mode::Block(depth - 1)
                    };
                    i += 2;
                } else {
                    out.push(if c == '\n' { '\n' } else { ' ' });
                    i += 1;
                }
            }
            Mode::Str | Mode::Char => {
                out.push(c);
                if c == '\\' {
                    if let Some(escaped) = chars.get(i + 1) {
                        out.push(*escaped);
                    }
                    i += 2;
                } else {
                    if (matches!(mode, Mode::Str) && c == '"')
                        || (matches!(mode, Mode::Char) && c == '\'')
                    {
                        mode = Mode::Code;
                    }
                    i += 1;
                }
            }
            Mode::Raw(hashes) => {
                out.push(c);
                if c == '"' && (1..=hashes).all(|ahead| chars.get(i + ahead) == Some(&'#')) {
                    for ahead in 1..=hashes {
                        out.push(chars[i + ahead]);
                    }
                    mode = Mode::Code;
                    i += 1 + hashes;
                } else {
                    i += 1;
                }
            }
        }
    }
    out
}

/// Every third-party action is pinned to a commit, and every container image to an explicit tag.
///
/// A workflow's `uses:` is remote code executed with this repository's token. A tag or a branch there is
/// mutable by whoever owns that repository, so `@v6` means "whatever they publish next" — the shape every
/// supply-chain compromise of GitHub Actions has taken. Everything here *was* already pinned to a full SHA, and
/// that is exactly why this exists: nothing enforced it, so the next step written as `@v6` would have passed
/// every gate, and "we pin our actions" was a convention rather than a property. The same argument this file
/// makes about lockfiles.
///
/// The version is expected in a trailing comment, because a bare SHA is unreadable and Dependabot writes and
/// updates that comment itself — an unreadable pin is one nobody dares bump.
///
/// Images take a **tag** rather than a digest, and that is the deliberate weaker rule: a service container's tag
/// is chosen so an upstream release cannot fail an unrelated pull request, which `latest` (or no tag at all)
/// defeats outright. Requiring digests would mean a digest bump per patch release of PostgreSQL for a container
/// that holds a test database for ninety seconds.
///
/// Derived from the tree: every tracked workflow, action definition, Compose file **and Dockerfile**, so a
/// second workflow cannot be added outside the rule — one already exists (`docs.yml`, which deploys the public
/// site).
///
/// **The Dockerfile selection is the part that was missing, and the `FROM` branch below was unreachable
/// without it.** The comment beside that branch claimed a Dockerfile's base image is checked "in the other
/// spelling", while the file filter named only workflows, action definitions and paths containing
/// `docker-compose` — so `deploy/Dockerfile`, the one image that actually reaches a user, was never read and
/// `FROM debian:latest` would have passed. A gate that sees less than it claims is this file's own recurring
/// defect; both Compose spellings are matched for the same reason the Dependabot inventory matches both.
#[test]
fn every_action_is_pinned_to_a_commit_and_every_image_to_a_tag() {
    let repo = repo_root();
    let listing = Command::new("git")
        .args(["ls-files"])
        .current_dir(repo)
        .output()
        .expect("git is available in a git checkout");

    let mut unpinned: Vec<String> = Vec::new();
    let mut actions = 0usize;
    let mut images = 0usize;
    // Asserted directly rather than inferred from the image total: a count floor is satisfied by the Compose
    // file alone, so dropping the Dockerfile selection again would leave every assertion here passing.
    let mut dockerfiles = 0usize;
    for file in String::from_utf8_lossy(&listing.stdout)
        .lines()
        .filter(|f| declares_images(f))
    {
        if file
            .rsplit('/')
            .next()
            .unwrap_or(file)
            .starts_with("Dockerfile")
        {
            dockerfiles += 1;
        }
        let text = std::fs::read_to_string(repo.join(file)).unwrap_or_default();
        for (number, line) in text.lines().enumerate() {
            let trimmed = line.trim().trim_start_matches("- ").trim();
            if let Some(rest) = trimmed.strip_prefix("uses:") {
                let reference = rest.trim();
                // A local action is this repository's own code, reviewed with it.
                if reference.starts_with('.') {
                    continue;
                }
                actions += 1;
                let (_, version) = match reference.split_once('@') {
                    Some(pair) => pair,
                    None => {
                        unpinned.push(format!(
                            "{file}:{}: `{reference}` names no version",
                            number + 1
                        ));
                        continue;
                    }
                };
                let sha = version.split_whitespace().next().unwrap_or(version);
                if sha.len() != 40 || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
                    unpinned.push(format!(
                        "{file}:{}: `{reference}` is not pinned to a 40-character commit sha",
                        number + 1
                    ));
                } else if !version.contains('#') {
                    unpinned.push(format!(
                        "{file}:{}: `{reference}` is pinned but says nothing about which version that is",
                        number + 1
                    ));
                }
            } else if let Some(rest) = trimmed
                .strip_prefix("image:")
                // A Dockerfile's base image is the same claim in the other spelling, and it is the one that
                // reaches a user: the published image is built from it, where a service container holds a test
                // database for ninety seconds.
                .or_else(|| trimmed.strip_prefix("FROM "))
            {
                let reference = image_reference_in(rest);
                // A Compose file may build rather than pull, and interpolate its own tag; a later Dockerfile
                // stage may refer to an earlier one by the name it gave it, which is internal to this build.
                if reference.is_empty()
                    || reference.contains('$')
                    || (!reference.contains('/') && !reference.contains(':'))
                {
                    continue;
                }
                images += 1;
                // A tag, and not a moving one. The registry host may carry a port, so the tag is looked for
                // after the last `/`.
                let last = reference.rsplit('/').next().unwrap_or(reference);
                match last.split_once(':') {
                    Some((_, tag)) if tag != "latest" && !tag.is_empty() => {}
                    _ => unpinned.push(format!(
                        "{file}:{}: image `{reference}` has no explicit tag, or names `latest`",
                        number + 1
                    )),
                }
            }
        }
    }

    assert!(actions > 20, "only found {actions} action references");
    assert!(images > 4, "only found {images} image references");
    assert!(
        dockerfiles > 0,
        "no Dockerfile was read - the `FROM` branch above is unreachable, so a base image on `latest` passes"
    );
    eprintln!("IMAGES={images}");
    assert!(
        unpinned.is_empty(),
        "{} unpinned reference(s) - remote code or an image this repository does not control the contents \
         of:\n  {}",
        unpinned.len(),
        unpinned.join("\n  ")
    );
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
/// The placeholders that stand for a home directory in tracked content, each named with what writes it.
///
/// A list, and a short one, because the alternative is worse in both directions: no list means the sweep cannot
/// distinguish a scrubbed archive from an unscrubbed one, and a *derived* answer would have to decide whether an
/// arbitrary name is a real account, which nothing in a repository can know.
const HOME_PLACEHOLDERS: [&str; 3] = [
    // What `scripts/message-fixtures/capture.sh` substitutes.
    "sideseat",
    // What the replay archives under `tools/otel-replay/fixtures/` were scrubbed to.
    "test-user",
    // The generic in documentation and doc comments (`expand_path("~") -> /home/user`).
    "user",
];

/// No **tracked file** carries the capturing developer's account name — compressed archives included.
///
/// A sample that reads a file records the absolute path it read, so a capture carries whoever ran it into a
/// public repository: 918 occurrences across 48 fixtures before this existed, naming one maintainer's home
/// directory. `record-otlp.py` substitutes a placeholder at capture time now, and this is what keeps the next
/// capture from quietly reintroducing it — the script is the fix, and a fix nothing checks is a convention.
///
/// The question is asked of the **shape of a home directory**, not of one account name. Matching `$USER` was
/// the first form, and it is vacuous exactly where it matters most: in CI the account is `runner`, so the guard
/// ran and could not have seen a contributor's own home directory in a fixture they captured. Every
/// `…/Users/<name>/` and `…/home/<name>/` in a tracked file is reported unless the name is one of
/// [`HOME_PLACEHOLDERS`] — which needs no list of forbidden names and does not depend on who runs it. Windows
/// profile paths are matched in both slash spellings, since a capture can come from there.
///
/// **The account name itself is deliberately not searched for**, and that was measured rather than reasoned
/// about. It used to be, as a supplement — "a name can reach a file by something other than a path: an author
/// field, a hostname, a bucket name" — and once the scope became the whole repository it made the invariant
/// unusable: with `USER=runner`, the account every GitHub Ubuntu job runs as, it reports **79 tracked files**,
/// so `check-server` could never pass. Narrowing it does not rescue it either. `/runner/` occurs as a path
/// segment in three lockfiles, and inside the captured payloads alone `user` occurs 7,094 times, `build` 1,479
/// and `test` 826. A check that depends on the maintainer's account name being an unusual word is a check that
/// fires for the wrong people, and its own note above already said the form was vacuous in CI. The residual is
/// stated: an account name reaching a file somewhere other than a path — an author field, a hostname — is not
/// caught here. What is caught is the shape that a capture actually records, which is an absolute path.
///
/// **Scope was the defect, twice over.** Restricted to `server/tests/fixtures`, this passed while
/// `tools/otel-replay/fixtures/traces-crewai.jsonl.gz` carried 940 occurrences of a maintainer's home directory
/// and a doc comment in `api/routes/agui/` cited a plan file in the same home directory. Five of the six replay
/// archives *had* been scrubbed by hand, which is exactly what a guard that cannot see them produces: the work
/// was done and one file was missed, with nothing to say so. So the sweep reads every tracked file, and a
/// compressed one is **decompressed** rather than skipped — a `.gz` is where a capture's paths are most likely
/// to be and least likely to be noticed. Compressed archives are counted against the number tracked, so a
/// decoder that silently fails cannot leave the sweep quietly reading five files instead of six.
///
/// A prefix must sit at an **absolute-path boundary**: `@/pages/home/create-project-dialog` is a module import,
/// not a home directory, and reporting it would teach a reader to disbelieve the finding.
#[test]
fn no_tracked_file_carries_the_capturing_users_name() {
    use std::io::Read;

    // The prefixes a home directory is spelled with, on every platform a capture can come from - and in both
    // encodings, because a fixture is JSON: a Windows path arrives with its separators doubled, so the literal
    // single-backslash form never matches the bytes on disk. The first version had only that form, and the
    // mutation that "verified" it used forward slashes, so the case it was written for was untested.
    let prefixes: [&[u8]; 5] = [
        b"/Users/",
        b"/home/",
        br"C:\Users\",
        br"C:\\Users\\",
        b"C:/Users/",
    ];
    // **Tracked files only**, which is the property: the concern is what a public repository carries, and a
    // local-only fixture directory is gitignored precisely because it is nobody else's. Scanning the working
    // tree instead reported eight files in `vercel-ai-js/image-gen/`, which `.gitignore` excludes - a failure
    // for something that is not published.
    let repo = repo_root();
    let listing = std::process::Command::new("git")
        .args(["ls-files", "-z"])
        .current_dir(repo)
        .output()
        .expect("git is available in a git checkout");
    assert!(listing.status.success(), "git ls-files failed");

    let mut offenders: Vec<String> = Vec::new();
    let mut scanned = 0usize;
    let mut archives = 0usize;
    let mut tracked_archives = 0usize;

    // Chunked, with an overlap, because the archives decompress to 346 MB and holding one whole is 129 MB of
    // it. The overlap is what keeps a match that straddles a chunk boundary visible; without it the sweep's
    // blind spot would depend on the buffer size, which is the worst kind.
    const CHUNK: usize = 1 << 20;
    let overlap = prefixes.iter().map(|p| p.len()).max().unwrap_or(0) + 64;

    for rel in listing
        .stdout
        .split(|b| *b == 0)
        .filter(|entry| !entry.is_empty())
    {
        let rel = String::from_utf8_lossy(rel).to_string();
        let path = repo.join(&rel);
        let compressed = rel.ends_with(".gz");
        if compressed {
            tracked_archives += 1;
        }
        let Ok(file) = std::fs::File::open(&path) else {
            continue;
        };
        let mut reader: Box<dyn Read> = if compressed {
            // `MultiGzDecoder`, not `GzDecoder`: a gzip file may hold **several concatenated members** (which
            // is how `cat a.gz b.gz` works, and what any appending producer writes), and the single-member
            // decoder stops at the first one - so a second member carrying a home directory was invisible
            // while the archive counter still recorded the file as read. Silent truncation of the input is
            // the same defect as skipping the file, one layer down.
            Box::new(flate2::read::MultiGzDecoder::new(file))
        } else {
            Box::new(file)
        };
        scanned += 1;
        if compressed {
            archives += 1;
        }

        let mut buffer: Vec<u8> = Vec::with_capacity(CHUNK + overlap);
        let mut done = false;
        let mut found_here = false;
        while !done && !found_here {
            let filled = buffer.len();
            buffer.resize(filled + CHUNK, 0);
            let mut read = 0usize;
            while read < CHUNK {
                match reader.read(&mut buffer[filled + read..]) {
                    Ok(0) => {
                        done = true;
                        break;
                    }
                    Ok(n) => read += n,
                    // A truncated or corrupt archive is a finding of its own: the sweep must not report a
                    // clean answer for content it could not read.
                    Err(error) => {
                        offenders.push(format!("{rel}: could not be read ({error})"));
                        done = true;
                        break;
                    }
                }
            }
            buffer.truncate(filled + read);

            for prefix in prefixes {
                for found in memchr::memmem::find_iter(&buffer, prefix) {
                    // A home directory is named by an *absolute* path. `pages/home/…` is a module path, and
                    // the byte before the match is what tells them apart.
                    if found > 0
                        && matches!(buffer[found - 1],
                            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.')
                    {
                        continue;
                    }
                    let after = found + prefix.len();
                    // What an account name is *made of*, rather than a list of the separators that end one:
                    // as an exclusion list this captured the trailing punctuation of prose (`/home/user`).`
                    // stopped at neither the backtick nor the paren), so the placeholder comparison failed on
                    // a name that was the placeholder. Non-ASCII is included because the truncation marker
                    // below is `…` and because an account name may itself be non-ASCII.
                    let name: Vec<u8> = buffer[after..]
                        .iter()
                        .copied()
                        .take_while(|b| {
                            b.is_ascii_alphanumeric()
                                || matches!(b, b'-' | b'_' | b'.')
                                || *b >= 0x80
                        })
                        .collect();
                    if name.is_empty() || name.len() > 64 {
                        continue;
                    }
                    let name = String::from_utf8_lossy(&name).to_string();
                    // A preview truncates, so a golden holds the placeholder cut mid-name (`…/si…[960
                    // chars]`). Anything up to the ellipsis that is still a prefix of a placeholder is
                    // consistent with it, and nothing distinguishes it from one - the residual is a real
                    // account whose name is itself a prefix of a placeholder *and* truncated at that point.
                    let (name, truncated) = match name.split_once('…') {
                        Some((head, _)) => (head.to_string(), true),
                        None => (name, false),
                    };
                    let stands_for_a_home = HOME_PLACEHOLDERS.contains(&name.as_str())
                        || (truncated
                            && HOME_PLACEHOLDERS
                                .iter()
                                .any(|placeholder| placeholder.starts_with(&name)));
                    if !stands_for_a_home {
                        offenders.push(format!("{rel}: {}{name}", String::from_utf8_lossy(prefix)));
                        found_here = true;
                        break;
                    }
                }
            }

            // Keep the tail, so a name split across two reads is still whole in the next pass.
            if !done {
                let keep = buffer.len().saturating_sub(overlap);
                buffer.drain(..keep);
            }
        }
    }
    offenders.sort();
    offenders.dedup();
    assert!(scanned > 1_000, "only scanned {scanned} tracked files");
    assert_eq!(
        archives, tracked_archives,
        "{tracked_archives} compressed archive(s) are tracked and {archives} were read - a decoder that \
         fails silently leaves the sweep blind to exactly the files a capture's paths hide in"
    );
    assert!(
        offenders.is_empty(),
        "{} tracked file(s) name a home directory a public repository should not carry - re-capture with \
         scripts/message-fixtures/capture.sh, which substitutes `{}`:\n  {}",
        offenders.len(),
        HOME_PLACEHOLDERS[0],
        offenders.join("\n  ")
    );
}

/// A Rust source file cannot be hidden by a broad ignore rule.
///
/// Cargo can compile an ignored module from a developer's working tree even though the file will be absent
/// from a clean checkout. This is especially easy with ordinary domain names such as `logs/`, which also
/// occur in generic Node ignore templates.
#[test]
fn no_rust_source_file_is_ignored() {
    let output = Command::new("git")
        .args([
            "ls-files",
            "--others",
            "--ignored",
            "--exclude-standard",
            "server",
        ])
        .current_dir(repo_root())
        .output()
        .expect("git is available in a git checkout");
    assert!(
        output.status.success(),
        "git could not enumerate ignored server files: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let ignored: Vec<&str> = stdout
        .lines()
        .filter(|path| path.ends_with(".rs"))
        .collect();
    assert!(
        ignored.is_empty(),
        "ignored Rust source file(s) compile locally but disappear from a clean checkout:\n  {}",
        ignored.join("\n  ")
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
#[derive(Debug, Eq, PartialEq)]
struct DiagramEntry {
    offset: usize,
    path: String,
    is_dir: bool,
}

fn parse_tree_block(
    root: &str,
    block: &[&str],
    extensions: &BTreeSet<&str>,
) -> (Vec<DiagramEntry>, Vec<(usize, String)>) {
    let mut entries = Vec::new();
    let mut errors = Vec::new();
    let mut branch: Vec<String> = Vec::new();

    for (offset, line) in block.iter().enumerate() {
        let Some(connector) = line.find("├── ").or_else(|| line.find("└── ")) else {
            continue;
        };
        let indent = line[..connector].chars().count();
        if !indent.is_multiple_of(4) {
            errors.push((
                offset,
                format!(
                    "indented {indent} columns, which is not a whole number of levels - the parentage \
                     cannot be read from it"
                ),
            ));
            continue;
        }
        let depth = indent / 4;
        if depth > branch.len() {
            errors.push((
                offset,
                format!(
                    "jumps from level {} to level {depth} - no entry states the level in between, so its \
                     parentage is unreadable",
                    branch.len()
                ),
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
        let path = format!("{root}/{}{name}", {
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

        if !is_dir
            && !name
                .rsplit_once('.')
                .is_some_and(|(_, extension)| extensions.contains(extension))
        {
            continue;
        }
        entries.push(DiagramEntry {
            offset,
            path,
            is_dir,
        });
    }

    (entries, errors)
}

#[test]
fn the_tree_diagram_parser_preserves_parentage_and_rejects_ambiguous_indent() {
    let extensions = BTreeSet::from(["rs", "toml"]);
    let block = [
        "├── src/",
        "│   ├── lib.rs",
        "│   └── nested/",
        "│       └── mod.rs",
        "└── Cargo.toml",
    ];
    let (entries, errors) = parse_tree_block("server/crates/example", &block, &extensions);
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(
        entries
            .iter()
            .map(|entry| entry.path.as_str())
            .collect::<Vec<_>>(),
        [
            "server/crates/example/src",
            "server/crates/example/src/lib.rs",
            "server/crates/example/src/nested",
            "server/crates/example/src/nested/mod.rs",
            "server/crates/example/Cargo.toml",
        ]
    );

    let (_, errors) = parse_tree_block(
        "server/crates/example",
        &["  └── bad.rs", "        └── jump.rs"],
        &extensions,
    );
    assert_eq!(errors.len(), 2);
    assert!(errors[0].1.contains("not a whole number of levels"));
    assert!(errors[1].1.contains("jumps from level"));
}

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
/// runs, so editing one leaves the other stating the previous requirement. That is not cosmetic here:
/// `make node-floor` derives the supported Node versions from the **lockfiles**, so a stale copy means the
/// derivation silently omits the package's own declared constraint - which is exactly what happened when
/// `examples/javascript` was corrected from `>=20.0.0` and only the manifest was touched.
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
            // A path that climbs past the repository root names something outside this tree, and a lockfile
            // depending on a directory beside the checkout is a dependency nobody else can resolve. That was a
            // silent `continue`, so it passed.
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
/// Four separate reviews found one of these at a time - the audit steps, the Makefile's eleven uv calls, Cargo's
/// nine, then `make setup`, `dev-server` and the benchmark - because each fix was an instance and the rule lived
/// nowhere. This is the rule: a lockfile is a statement about what was reviewed, and a command that silently
/// rewrites it makes every later `--locked` check a statement about a machine instead.
///
/// **A line that could be pasted and run**, which is the distinction that makes this checkable: a command at the
/// start of a line (after a make recipe's `@`, a `(cd … &&` prefix, or an `echo` that prints instructions) or
/// inside a fenced block is something a reader or a shell executes. A command named mid-sentence - "lint with
/// `cargo clippy`" - is a reference, and locking prose would be noise rather than rigour.
///
/// The exceptions are commands whose **purpose** is to write the lockfile, and they are named rather than
/// pattern-matched: `uv lock`, `uv add`, `cargo update`, and the installers (`cargo install`, `npm install`),
/// which resolve something other than this workspace.
///
/// `cargo fetch` was on that list and does not belong there: it resolves **this** workspace and writes
/// `Cargo.lock` when the manifest has moved, so `make setup` — whose whole purpose is to reproduce the locked
/// tree, and which argues exactly that three lines below about `npm ci` — could repair a stale lockfile before
/// any later `--locked` gate looked at it. `cargo tarpaulin` was missing outright, which is the hand-maintained
/// inventory this file exists to remove, in the invariant that removes it.
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
        // Resolves and writes the lockfile like any other read of the graph - the MSRV note in the root
        // manifest documents one, and it was invisible while `#` comments were skipped wholesale.
        "cargo metadata",
        "uv sync",
        "uv run",
        "uv export",
    ];
    // No exception list. There was one - `uv lock`, `uv add`, `cargo update`, `cargo install`, `npm install` -
    // and emptying it changed no answer, because a segment is only examined when it *starts with* a resolving
    // command and none of those is one. So it excused nothing while telling the next reader that exceptions
    // were handled here. The deliberate writers are excluded by construction instead: they are absent from
    // `RESOLVING`, which is the list that decides what is examined.

    let mut unlocked: Vec<String> = Vec::new();
    let mut checked = 0usize;
    for file in String::from_utf8_lossy(&listing.stdout)
        .lines()
        // **Every tracked text file**, with no directory excluded either: the allowlist omitted the
        // extensionless git hooks, `.py`, `.mjs`, `.js` and `package.json` scripts, and the exclusion of
        // `server/tests/fixtures/` - written for captured payloads, which are data - also hid the fixtures
        // README, whose two golden-regeneration commands were unlocked. A payload is excluded by being
        // binary, which `is_text` already decides.
        .filter(|f| is_text(&repo.join(f)))
    {
        let text = std::fs::read_to_string(repo.join(file)).unwrap_or_default();
        // Rust is read through its **commentary**, not skipped and not read whole. Skipped, it hid two
        // pasteable commands in doc comments - the golden-regeneration line and a benchmark invocation, both
        // inside ```bash fences that a reader copies. Read whole, every entry of this scanner's own tables
        // would be a finding. A command *constructed* in code (`Command::new("cargo").args([…])`) is a third
        // shape, and one this would not have matched in any case.
        let rust = file.ends_with(".rs");
        let commentary: Vec<(usize, String)>;
        let numbered: Vec<(usize, &str)> = if rust {
            commentary = rust_commentary(&text);
            commentary
                .iter()
                // The doc marker is part of the comment body once `//` is consumed, so `///` arrives as `/`
                // and `//!` as `!`. Left on, the fence line `/// ```bash` does not start with a fence and
                // every command inside one was invisible.
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
            // A `#` comment that is *nothing but* a command is a command - the root manifest documents how to
            // re-measure the MSRV with one, and skipping every commented line hid it. A comment that quotes a
            // command inside a sentence is still a reference, which is what the `starts_with` distinguishes:
            // the same rule the fenced and backticked forms use, applied to the third kind of commentary.
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
            // A whole line that is one backticked command is a command. Mid-sentence, a backticked name is a
            // reference and locking it would be noise - but a comment line consisting of nothing else is how
            // every "run this to regenerate" line in this crate is written, and two of them were unlocked.
            let mut whole_line_command = false;
            if let Some(inner) = runnable
                .strip_prefix('`')
                .and_then(|rest| rest.strip_suffix('`'))
                .filter(|inner| !inner.contains('`'))
            {
                runnable = inner.trim();
                whole_line_command = true;
            }
            // In Rust commentary, prose **wraps**, so a continuation line can begin with anything - including
            // the tail of a quoted command, which is how this file's own explanation of the `&&` splitting
            // reported itself three times. A comment line is a command only where it says so: inside a fenced
            // block, or as a line that is nothing but one backticked command. Markdown keeps the looser rule,
            // where a command at the start of a line is what a reader copies.
            if rust && !fenced && !whole_line_command {
                continue;
            }
            // Leading environment assignments: `UPDATE_GOLDENS=1 cargo test …` is the documented way to
            // regenerate the goldens, and testing what the segment *starts with* saw the assignment instead of
            // the command - the same blindness as the `cargo watch` argument form.
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
            // Each segment a shell would run, judged **on its own**: taking the first resolving segment and
            // then testing the whole *line* let `cargo test --locked && cargo build` pass and
            // `cargo fetch && cargo build` be exempt entirely.
            // One normalisation for both, because the fenced branch used the *raw* line and so discarded every
            // prefix strip above it: the documented `UPDATE_GOLDENS=1 cargo test …` sits inside a ```bash
            // fence, and inside a fence the environment assignment was never removed, so the command behind it
            // was never seen. A fence makes a line more pasteable, not less.
            let candidate = runnable;
            for segment in candidate
                .split("&&")
                .flat_map(|part| part.split(';'))
                .flat_map(|part| part.split("||"))
                .map(str::trim)
            {
                // `cargo watch -x "run -- …"` and `watchexec -- "… cargo run …"` carry the command inside an
                // argument, so for those the segment is searched from any position - both `dev-server`
                // branches were unlocked and invisible for exactly that reason.
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
/// Five suites declare both `<name>` and `telemetry-<name>`, and the alias exists for a reason `run-all.sh`
/// states: the framework's own package installs a CLI of that name and **wins**, so `uv run --directory
/// examples/python/crewai crewai` runs CrewAI's CLI rather than the sample. Only `run-all.sh` used the alias;
/// `capture.sh` and the README named the colliding form, which means the fixture-capture path for that suite was
/// invoking the wrong program.
///
/// The rule is uniform rather than per-suite, deliberately: `langgraph` did not collide *today* only because
/// `langgraph-cli` is not in that tree, and the alias costs nothing.
#[test]
fn every_aliased_sample_suite_is_invoked_by_its_alias() {
    let repo = repo_root();
    let listing = Command::new("git")
        .args(["ls-files", "examples/python"])
        .current_dir(repo)
        .output()
        .expect("git is available in a git checkout");

    let mut aliased: Vec<String> = Vec::new();
    for manifest in String::from_utf8_lossy(&listing.stdout)
        .lines()
        .filter(|f| f.ends_with("pyproject.toml"))
    {
        let Some(suite) = manifest.split('/').nth(2) else {
            continue;
        };
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
    assert!(
        aliased.len() >= 3,
        "found only {} aliased suite(s) - the scan is wrong, not the tree",
        aliased.len()
    );

    // The callers are **discovered**, not listed: three filenames were hardcoded, so a new documentation page,
    // Make target or script could invoke the colliding CLI while this stayed green - and one already did
    // (`examples/python/README.md`, twenty-five times). Every tracked text file is a candidate.
    let all = Command::new("git")
        .args(["ls-files"])
        .current_dir(repo)
        .output()
        .expect("git is available in a git checkout");
    let mut colliding: Vec<String> = Vec::new();
    for caller in String::from_utf8_lossy(&all.stdout)
        .lines()
        // **Every tracked text file**, not an extension allowlist: the allowlist omitted the extensionless git
        // hooks, `.py`, `.mjs`, `.js` and `package.json` scripts - the hand-maintained inventory these
        // invariants exist to remove, reintroduced inside one of them.
        .filter(|f| !f.starts_with("server/tests/fixtures/"))
        // Rust is excluded for a reason of its own, not the one the sibling scanner has (that comment was
        // copied here and did not fit): **this file documents the pattern it looks for**, so reading Rust
        // commentary would report its own explanation. The residual, stated: a Rust file that genuinely
        // invoked a sample suite would be missed - and nothing in Rust runs them, since the capture path is
        // `scripts/message-fixtures/capture.sh`.
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
/// check can do for a `uv` invoked directly - and `make update-python-deps` is the one command that *writes*
/// lockfiles, on a contributor's machine. Pinning `setup-uv`'s version input covered CI alone.
///
/// It has to be restated per project, and that is uv's rule rather than a choice: a project's own `[tool.uv]`
/// **replaces** the `uv.toml` found above it instead of merging, so fourteen projects were exactly the ones a
/// differently-versioned uv could still write lockfiles for. Verified by mutating one and watching uv refuse.
/// This test is what keeps the fifteen declarations one value.
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
        // The **root** `uv.toml`, by path: any tracked file of that name satisfied the count and section
        // checks, so moving it under `config/` unchanged would have removed the pin from every project that
        // inherits it while every assertion still passed.
        .filter(|f| f.ends_with("pyproject.toml") || *f == "uv.toml")
    {
        let text = std::fs::read_to_string(repo.join(file)).unwrap_or_default();
        // The **section** matters, not only the line: uv reads `required-version` from `[tool.uv]` in a manifest
        // and from the top level of a `uv.toml`, and ignores it silently anywhere else. The mutation test covered
        // deletion and not misplacement, so a declaration moved into an unrelated table satisfied every
        // assertion here while doing nothing at all.
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
        // The declaration, not the *word*: `contains` over the whole file counts a mention in a comment, so
        // deleting the real line from a manifest whose comment explains it still passed - the exact defect this
        // half is for. Asked of non-comment lines, like the value scan above.
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
/// It appears in four places — the Makefile header, `make help`, the `setup` prerequisite check and
/// `CONTRIBUTING.md` — and each time it changed, one of them was missed: `make help` said "20+" for a whole
/// review cycle after the others were corrected, and a *fifth* place (the JavaScript samples) said "20+" for
/// two. A prerequisite that is wrong in one place is worse than one that is absent, because the reader who
/// finds it stops looking.
///
/// Scoped to the repository-wide floor: `examples/javascript` states its own, looser range on purpose, since
/// that suite is installable on its own. What is pinned is that the repository's number has one value.
#[test]
fn the_node_requirement_is_stated_once() {
    let repo = repo_root();
    let listing = Command::new("git")
        .args(["ls-files"])
        .current_dir(repo)
        .output()
        .expect("git is available in a git checkout");
    let mut stated: BTreeMap<String, Vec<String>> = BTreeMap::new();
    // **Derived from the tree**, not the two files this started with. `examples/javascript/README.md` states
    // the repository-wide floor as well - a fifth place, invisible to a two-name list, free to drift.
    for file in String::from_utf8_lossy(&listing.stdout)
        .lines()
        .filter(|f| !f.starts_with("server/tests/fixtures/"))
        .filter(|f| is_text(&repo.join(f)))
    {
        let text = std::fs::read_to_string(repo.join(file)).unwrap_or_default();
        for (number, line) in text.lines().enumerate() {
            // The shape the floor is written in: `22.22+ or 24+`.
            for (at, c) in line.char_indices() {
                if !c.is_ascii_digit() || (at > 0 && !line[..at].ends_with([' ', '(', '>'])) {
                    continue;
                }
                let rest = &line[at..];
                let end = match rest.find(|c: char| !c.is_ascii_digit() && c != '.' && c != '+') {
                    Some(end) => end,
                    None => continue,
                };
                let (first, tail) = (&rest[..end], &rest[end..]);
                if !first.ends_with('+') || !first.contains('.') {
                    continue;
                }
                let Some(second) = tail.strip_prefix(" or ") else {
                    continue;
                };
                let second_end = second
                    .find(|c: char| !c.is_ascii_digit() && c != '.' && c != '+')
                    .unwrap_or(second.len());
                // A sentence's full stop is not part of the version. `.` has to be *inside* the scan, because
                // the floor is written `22.22+`, so it is trimmed afterwards - and until it was, a statement
                // ending in a full stop was invisible: the second version came out as `22+.`, failed the `+`
                // test, and a whole contradicting claim went unseen. (Phrased without the two-version form on
                // purpose: this file is scanned too, and an example of the shape would *be* a statement.)
                let second = second[..second_end].trim_end_matches('.');
                if !second.ends_with('+') {
                    continue;
                }
                stated
                    .entry(format!("{first} or {second}"))
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
/// The **sixth** instance of the relative-depth class, and the first that no earlier guard could see: nothing
/// executes a `$schema`, so `deploy/local/sideseat.json` pointed at `../../../config/…` from two levels down
/// and every check stayed green while an editor silently validated against nothing. A configuration file whose
/// schema does not load is worse than one with no schema, because the absence of complaints reads as approval.
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

/// Every script that walks up to the repository root actually arrives there.
///
/// This is the **fifth** instance of one class: a file moved to a purpose-named directory keeps counting the
/// levels its old location had, and nothing fails at the moment of the move. The first four were caught by
/// running the thing — an embedded asset folder, two `include_str!` paths, thirteen package manifests, the
/// fixture scripts. The fifth was not: `scripts/bench-http-latency.sh` came from `misc/bench/`, kept `../..`, and
/// resolved the root to the *parent of the repository* — so `make bench-http`, which is the latency gate,
/// failed before building anything, and it stayed that way because a benchmark is not part of `make check`.
///
/// The claim is evaluated, not read: the level count in the expression is compared against the file's own
/// depth in the tree. That is exactly the fact a move changes, and the only one a reviewer reliably misses.
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
        // A **script**, asked of the file rather than of its name: a shebang or a script extension. The
        // extension allowlist omitted `.githooks/pre-commit` and `.githooks/pre-push`, which are shell scripts
        // with no extension at all - the same blindness that hid two unlocked commands in `every_resolving_
        // command_is_locked`, still live in the third scanner after the other two were fixed. Rust is not a
        // script and its tests legitimately write `let root = …`, so nothing here matches it.
        .filter(|f| {
            [".sh", ".py", ".mjs", ".js", ".ts"]
                .iter()
                .any(|ext| f.ends_with(ext))
                || std::fs::read_to_string(repo.join(f)).is_ok_and(|text| text.starts_with("#!"))
        })
        // Only the vendored trees, not `examples/` wholesale: that blanket exclusion hid `examples/run-all.sh`,
        // which resolves the root exactly as the benchmark did. The sample suites themselves resolve their own
        // content and `.env` directories, not the root, and are named accordingly - so they are not skipped,
        // they simply do not match.
        .filter(|f| !f.contains("/.venv/") && !f.contains("/node_modules/"))
    {
        // The file's own depth: `scripts/bench-http-latency.sh` sits one directory below the root.
        let depth = file.matches('/').count();
        let text = std::fs::read_to_string(repo.join(file)).unwrap_or_default();
        for (number, line) in text.lines().enumerate() {
            // Case-insensitively, and in every scripting language here. Restricted to shell and Python with
            // an uppercase name, this could not see `scripts/node-floor.mjs` - a file added in the same
            // commit as the guard, whose `const root = join(dirname(…), "..")` is the identical claim.
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

    // A line that names any child of its directory has to name **all** of them, because that is what the
    // reader takes from an enumeration. Comparing top-level names alone left the very claim that motivated
    // this test unprotected: `server/`'s line inventories its children, and adding a seventh would have
    // passed. Lines that describe a directory in prose enumerate nothing and so claim nothing.
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
/// The diagram check above covers a map's own drawing; this one covers every path named in **prose**, which no
/// diagram contains. The two overlap nowhere: a citation is a claim made in a sentence, and there were three
/// live stale ones — the module this pins is `normalize.rs`, whose former name still exists as a *different*
/// file under `domain/traces/`, so any check asking only "does this filename exist" saw a match where the
/// citation was wrong.
///
/// A cited path is position-bearing, so it can be checked without parsing any layout: root-anchored whole,
/// doc-relative against the citing file's directory, otherwise as a **suffix** of a tracked path — where
/// `sideml/normalize.rs` matches and the stale spelling matches nothing. **Assets as well as modules**: the
/// rules regrouping moved 43 JSON files, and a check that watched only `.rs` had nothing to say about the
/// nineteen citations left naming the flat path.
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
        // **Every tracked text file.** An extension allowlist came first and made the test's name ("anywhere")
        // false twice over: it started at Markdown and Rust, grew to eleven extensions plus `Makefile` as each
        // omission was found - `sdk/python`'s `protocol.py`, the TLA+ specifications, the Makefile's script
        // paths - and still omitted `.json`, `.astro`, `.yaml` and the extensionless hooks. That is the
        // hand-maintained inventory these invariants exist to remove, inside one of them.
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
                // A path with at least one directory, and not a glob or a path outside the tree: a glob names
                // a set rather than a file, and `~/.sideseat/sideseat.json`, `./sideseat.json` and
                // `/path/to/service-account.json` are runtime and example paths that the first version of this
                // extension reported.
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
                // **The question is whether this repository has that file somewhere else.** Requiring the
                // citation's first segment to name a directory that exists was the previous rule and it had the
                // defect exactly backwards: a citation left pointing at a *removed* directory - `protocol/`,
                // `benchmarks/` - was skipped as "not a repository path", which is the one case that matters.
                // A basename the tree does not hold at all is skipped, and that is a **stated limit** rather
                // than an oversight: it covers a deleted file, an external path, and - the case worth naming -
                // a citation of a file that was *renamed*, whose old basename is gone by definition. Catching
                // that needs history, not the working tree. What is left covering it: a script path in a make
                // recipe fails when the recipe runs, and `every_script_that_locates_the_repository_root_finds_it`
                // checks the scripts themselves. A basename the tree holds at another path is always a defect.
                //
                // A **bare filename** with no directory is also skipped, and that too was measured rather than
                // assumed: two stale ones had reached the fixtures README (a renamed capture script and a
                // renamed review script), so resolving them looked worthwhile - but sweeping every
                // `<name>.sh`/`<name>.py` token in the tree reports `astral.sh` (a domain), the halves of a
                // wheel filename split at `py2.py3`, and every generic `app.py` in prose. The rule would
                // accuse more than it caught, and a check a reader learns to disbelieve protects nothing.
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
/// This is the dependency the crate split turns into a compiler error: once `data` is
/// `sideseat-adapter-*` and cannot name `sideseat-api` in its manifest, a violation stops compiling. Until
/// then it is a test, because the compiler cannot see a layer that is only a directory.
///
/// Three real violations existed when this was written, and each was a different shape:
///
/// * `server/crates/adapter-duckdb/src/repositories/query.rs` imported `crate::api::routes::otel::filters` - a module that is
///   eight lines of `pub use sideseat_ports::filters::…`. So the analytics adapter reached *through* the HTTP
///   routing layer to borrow types the data layer already owned.
/// * `data/duckdb/filters/{types,parser}.rs` returned `ApiError` from filter parsing and validation, which
///   made the adapter manufacture HTTP responses. They return `FilterError` now and `api::types` converts
///   at the boundary, so the routes still just use `?`.
/// * `server/crates/ports/src/types/analytics.rs` imported `OrderBy`, a column plus a direction, from `api::types` - while
///   three of its own DTOs carried it as a field. The type and its SQL moved to `data::types::order`;
///   parsing a `?order_by=` parameter, which is where the 400 belongs, stayed in `api`.
///
/// Comments are stripped before matching, so prose about the API layer is not a violation - the same reason
/// the framework sweeps tokenise rather than grep.
/// Does this tracked path declare container images this repository does not control?
///
/// Prefixes rather than exact names, and both prefixes were defects. `Dockerfile.dev` carries a base image.
/// Compose reads `docker-compose.override.yml` as well as the four canonical spellings, and narrowing this to
/// exact basenames was a **regression** on the `contains("docker-compose")` it replaced - an override file
/// being precisely where a `latest` gets added.
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
/// Taking the first token blind read `--platform=$BUILDPLATFORM` as the image, saw the `$`, and skipped the
/// line - so `FROM --platform=$BUILDPLATFORM debian:latest` passed the one gate that reads the published
/// image.
fn image_reference_in(rest: &str) -> &str {
    rest.trim()
        .trim_matches('"')
        .split_whitespace()
        .find(|token| !token.starts_with("--"))
        .unwrap_or_default()
}

/// The two parsing decisions above, on input the tree does not contain.
///
/// Both fixes were verified by hand-editing `deploy/Dockerfile` and staging an override file, which is not an
/// enduring gate: with no tracked file carrying either shape, each fix could be reverted and the suite would
/// stay green. These cases are the shapes themselves, so the parser is held to them permanently.
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
/// The plan names this defect precisely: "`Filter` lives in one adapter and the other imports it". It did - the
/// filter vocabulary, its operators and the column allowlists sat inside `data::duckdb::filters` while the
/// ClickHouse adapter imported them, so a shared type was owned by one implementation and the two could never be
/// separate crates. The vocabulary now lives in `data::filters` and each adapter keeps only its own rendering.
///
/// **Parity tests are exempt, and that is the point of them.** A test whose whole purpose is to require two
/// backends to return identical rows must see both. The exemption is by path, so it cannot quietly cover
/// production code.
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
/// A port says what a caller may ask for; a statement is how a store answers. Both were mixed in: `Filter` had
/// three `to_sql` methods, `OrderBy` rendered `column DIRECTION`, and `DisplayNameDialect` was a hand-rolled
/// per-dialect switch living in the DTOs - beside a real `SqlDialect` seam in the data layer that had no
/// consumers at all. It is the same shape as `DataError` naming four drivers: the abstraction carrying the
/// implementation.
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
/// `banner.rs` and `update.rs` read `env!("CARGO_PKG_VERSION")`, so which crate they live in decides which
/// version the product *claims*. Moving them into `sideseat-core` therefore made `--version`, the banner and the
/// update check report **core's** version - and `make sync-version` edited `server/Cargo.toml` alone, so the next
/// release would have printed the previous version and offered the running build to itself as an upgrade.
///
/// The fix is one version in `[workspace.package]`. This is the guard: a crate that spells its own version, or a
/// workspace that spells a different one, fails here rather than at a release.
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
    let makefile = std::fs::read_to_string(repo_root().join("Makefile")).expect("Makefile");
    assert!(
        makefile.contains("update-python-deps: ## Upgrade every Python lockfile")
            && makefile
                .contains("@set -e; for manifest in $$(git ls-files '*pyproject.toml'); do \\")
            && makefile.contains("(cd \"$$project\" && uv lock --upgrade); \\"),
        "dependency updates must stop at the first failed project"
    );

    let hooks_start = makefile.find("setup-hooks:").expect("setup-hooks target");
    let hooks_end = makefile[hooks_start..]
        .find("# Development")
        .map(|offset| hooks_start + offset)
        .expect("development section");
    let hooks = &makefile[hooks_start..hooks_end];
    assert!(
        hooks.contains("git rev-parse --git-dir")
            && !hooks.contains("[ -d .git ]")
            && !hooks.contains("|| true"),
        "hook setup must accept linked worktrees and report installation failures"
    );
}

#[test]
fn fixture_capture_only_stops_its_own_recorder() {
    let script = std::fs::read_to_string(repo_root().join("scripts/message-fixtures/capture.sh"))
        .expect("fixture capture script");

    assert!(
        !script.contains("pkill"),
        "fixture capture must not terminate recorders by global process matching"
    );
    for required in [
        "RECORDER_PID_FILE=",
        "printf '%s\\n' \"$recorder_pid\" >\"$RECORDER_PID_FILE\"",
        "read -r pid <\"$RECORDER_PID_FILE\"",
        "kill -TERM \"$pid\"",
    ] {
        assert!(
            script.contains(required),
            "fixture capture must contain `{required}`"
        );
    }
}

#[test]
fn container_tests_share_trapped_cleanup() {
    let script = std::fs::read_to_string(repo_root().join("scripts/container-test.sh"))
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

    let makefile = std::fs::read_to_string(repo_root().join("Makefile")).expect("Makefile");
    for scenario in [
        "clickhouse",
        "clickhouse-replicated",
        "clickhouse-two-shard",
        "postgres",
        "redis",
        "redpanda",
    ] {
        assert!(
            makefile.contains(&format!("./scripts/container-test.sh {scenario}")),
            "{scenario} must use the shared container lifecycle"
        );
    }
}

#[test]
fn pre_commit_routes_root_rust_changes_to_the_workspace_suite() {
    let hook =
        std::fs::read_to_string(repo_root().join(".githooks/pre-commit")).expect("pre-commit hook");
    for root_input in [
        "'Cargo.toml'",
        "'Cargo.lock'",
        "'deny.toml'",
        "'rustfmt.toml'",
        "'clippy.toml'",
        "'rust-toolchain.toml'",
        "'.cargo/'",
    ] {
        assert!(
            hook.contains(root_input),
            "pre-commit Rust detection must include {root_input}"
        );
    }
    assert!(
        hook.contains("staged 'sdk/rust/'") && hook.contains("make test-rust"),
        "Rust SDK and root configuration changes must run workspace tests"
    );
}

#[test]
fn ci_rejects_unused_rust_dependencies() {
    let workflow =
        std::fs::read_to_string(repo_root().join(".github/workflows/ci.yml")).expect("CI workflow");
    assert!(
        workflow.contains("cargo-machete@0.9.2")
            && workflow.contains("run: cargo machete")
            && !workflow.contains("continue-on-error: true\n        run: cargo machete"),
        "CI must install a pinned cargo-machete and run it as a blocking gate"
    );
}

#[test]
fn http_benchmark_bounds_requests_and_shutdown() {
    let script = std::fs::read_to_string(repo_root().join("scripts/bench-http-latency.sh"))
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
    let script = std::fs::read_to_string(repo_root().join("scripts/clean-stale.sh"))
        .expect("cleanup script");
    for required in [
        "cargo metadata --locked --no-deps",
        "metadata.target_directory",
        "find \"$target_dir\" -type d -name incremental",
        "cargo sweep --installed",
        "cargo sweep --time 3",
    ] {
        assert!(
            script.contains(required),
            "stale cleanup must contain `{required}`"
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
    let makefile = std::fs::read_to_string(repo_root().join("Makefile")).expect("Makefile");
    assert!(
        makefile.contains("help: ## Show available commands")
            && makefile.contains("@awk -f scripts/make-help.awk $(MAKEFILE_LIST)")
            && !makefile.contains("@echo \"SideSeat Development Commands\"")
            && !makefile.contains("NOTARIZE ?= 1"),
        "Make help and defaults must have one current source"
    );
}

#[test]
fn dependency_report_discovers_every_project_manifest() {
    let repo = repo_root();
    let output = Command::new("bash")
        .arg("scripts/deps-check.sh")
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
        std::fs::read_to_string(repo.join("scripts/deps-check.sh")).expect("dependency report");
    assert!(
        script.contains("command -v cargo-outdated")
            && script.contains("npm_status=0")
            && script.contains("uv tree --project \"$project\" --locked --outdated --depth 1")
            && !script.contains("cargo outdated -R || echo"),
        "dependency failures must not be reported as missing tools or hidden"
    );
}

/// Does this manifest line declare `driver`, under its own name or a rename?
///
/// Extracted so it can be tested on input the workspace does not contain. A live mutation is not available:
/// adding an unresolvable dependency to a manifest fails the *build*, so the test never runs and proves nothing
/// about its own logic.
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
    // `db = { package = "duckdb" }` followed by `use db::Connection` linked the driver under another name, and a
    // key-only check saw nothing.
    let renamed_to_driver =
        trimmed.contains("package") && trimmed.contains(&format!("\"{driver}\""));
    key_is_driver || renamed_to_driver
}

/// The predicate above, on the forms that defeated its first version.
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

    // **Workspace members, resolved from the manifest** - not the children of `server/crates/`. Reading the directory
    // was a claim the test could not keep: a layer crate placed anywhere else, or one removed from the members
    // list, was silently unscanned, and `checked > 0` could not tell the difference.
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
        // Adapter crates are the one place a driver belongs. Every other crate under `server/crates/` is an
        // inward-facing layer and must stay unable to import one. The prefix is part of the workspace's
        // target graph (`sideseat-adapter-*`), so a newly extracted adapter is classified immediately.
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
        .map(|(name, _)| name.trim())
        .filter(|name| name.starts_with("sideseat-"))
        .collect();
    assert_eq!(
        workspace_dependencies,
        BTreeSet::from(["sideseat-core", "sideseat-domain", "sideseat-ports"]),
        "the API transport may depend only on inward-facing SideSeat crates"
    );
}

/// Production wiring that behavioural tests cannot see, because they call the underlying method directly.
///
/// Two production call sites, each the *only* one, and each invisible to the behavioural tests because those
/// call the underlying method directly:
///
/// - `start_consistency_check_task` in `app.rs` is what runs the cross-partition check. Every consistency test
///   calls `check_partition_consistency()` itself, so deleting the scheduling left the suite green and the
///   cross-month residual permanently unreported - which is worse than not having the detector, because the
///   commit message says it is reported.
/// - `report_unidentified_metric_rows` in the ClickHouse migration path is what tells an operator that
///   pre-identity metric rows exist. The parity test invokes the method directly, so deleting the call left an
///   upgrade silently exposed.
///
/// Structural because there is nothing else available: both are `tokio::spawn`-and-forget side effects on a
/// path that needs a live ClickHouse and a full `AppState`, and asserting on log output is asserting on a
/// string. What can be checked is that the call exists, which is exactly the property that was missing.
///
/// Commentary is stripped first, so a mention of either name in a doc comment does not satisfy it - the same
/// discipline `the_storage_layer_does_not_import_the_http_layer` uses, and for the same reason: a gate that
/// accepts prose is a gate that passes while seeing less than it claims.
#[test]
fn every_detector_is_actually_started_in_production() {
    let repo = repo_root();
    for (file, call, why) in [
        (
            "server/src/app.rs",
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
        (
            "server/crates/adapter-clickhouse/src/lib.rs",
            "report_unidentified_metric_rows",
            "an upgrade would not report pre-identity metric rows, so an operator would have no way to learn \
             that a released row and its correction are both being served",
        ),
    ] {
        let text = std::fs::read_to_string(repo.join(file))
            .unwrap_or_else(|e| panic!("{file} is readable: {e}"));
        // `rust_commentary` returns the *comments*, so the code is what is left once they are removed - the
        // same shape `the_storage_layer_does_not_import_the_http_layer` uses.
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
    let mut scanned = 0usize;
    let mut roots = vec![repo.join("server/src/data")];
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
            scanned += 1;

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
        scanned > 100,
        "only scanned {scanned} storage and adapter files - the walk is not reaching the tree"
    );
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
    let method_sources = [
        "server/crates/ports/src/blobs.rs",
        "server/crates/ports/src/registrations.rs",
        "server/crates/ports/src/traits.rs",
    ];
    let query_sources = [
        "server/crates/ports/src/types/analytics.rs",
        "server/crates/ports/src/types/messages.rs",
        "server/crates/ports/src/types/stats.rs",
    ];

    let mut typed_scopes = 0usize;
    let mut offenders = Vec::new();
    for file in method_sources {
        let source = std::fs::read_to_string(repo.join(file))
            .unwrap_or_else(|e| panic!("{file} is readable: {e}"));
        typed_scopes += source.matches("project_id: &ProjectId").count();
        typed_scopes += source.matches("project_id: ProjectId").count();
        if raw_project_method_scope(&source) {
            offenders.push(file);
        }
    }
    for file in query_sources {
        let source = std::fs::read_to_string(repo.join(file))
            .unwrap_or_else(|e| panic!("{file} is readable: {e}"));
        typed_scopes += source.matches("pub project_id: ProjectId").count();
        if raw_project_query_scope(&source) {
            offenders.push(file);
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
            "the tenant-scope gate missed its regression fixture: {source}"
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
        "server/crates/domain/src/domain/pricing/mod.rs",
        "server/crates/domain/src/rate_limit.rs",
    ] {
        let source = std::fs::read_to_string(repo.join(file))
            .unwrap_or_else(|e| panic!("{file} is readable: {e}"));
        assert!(
            !source.contains("Utc::now"),
            "{file} bypasses the injected Clock"
        );
    }

    let secrets = repo.join("server/crates/adapter-secrets/src/secrets");
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

    for signal in sideseat_domain::signals::REGISTERED_SIGNAL_NAMES {
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
        let start = grpc
            .find(&format!("impl {service}"))
            .unwrap_or_else(|| panic!("registered signal {signal} has no {service}"));
        let service_source = &grpc[start..];
        let export_start = service_source
            .find("#[tonic::async_trait]")
            .unwrap_or_else(|| panic!("{service} has no transport implementation"));
        let export_source = &service_source[export_start..];
        let end = export_source
            .find("\n/// gRPC ")
            .unwrap_or(export_source.len());
        assert!(
            export_source[..end].contains("export_signal("),
            "registered signal {signal} bypasses the shared lifecycle on gRPC"
        );
    }
}
