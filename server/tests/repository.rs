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
        .filter(|f| {
            let name = f.rsplit('/').next().unwrap_or(f);
            f.starts_with(".github/workflows/")
                || f.ends_with("/action.yml")
                || f.ends_with("/action.yaml")
                // `Dockerfile.dev` and the like carry base images too, so this is a prefix rather than the
                // exact name the Dependabot inventory needs.
                || name.starts_with("Dockerfile")
                || matches!(
                    name,
                    "docker-compose.yml" | "docker-compose.yaml" | "compose.yml" | "compose.yaml"
                )
        })
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
                // `FROM x AS stage` names a stage after the reference.
                let reference = rest
                    .trim()
                    .trim_matches('"')
                    .split_whitespace()
                    .next()
                    .unwrap_or_default();
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
    let tracked: Vec<String> = String::from_utf8_lossy(&listing.stdout)
        .lines()
        .map(str::to_string)
        .collect();

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
/// * `data/duckdb/repositories/query.rs` imported `crate::api::routes::otel::filters` - a module that is
///   eight lines of `pub use crate::data::filters::…`. So the analytics adapter reached *through* the HTTP
///   routing layer to borrow types the data layer already owned.
/// * `data/duckdb/filters/{types,parser}.rs` returned `ApiError` from filter parsing and validation, which
///   made the adapter manufacture HTTP responses. They return `FilterError` now and `api::types` converts
///   at the boundary, so the routes still just use `?`.
/// * `data/types/analytics.rs` imported `OrderBy`, a column plus a direction, from `api::types` - while
///   three of its own DTOs carried it as a field. The type and its SQL moved to `data::types::order`;
///   parsing a `?order_by=` parameter, which is where the 400 belongs, stayed in `api`.
///
/// Comments are stripped before matching, so prose about the API layer is not a violation - the same reason
/// the framework sweeps tokenise rather than grep.
#[test]
fn the_storage_layer_does_not_import_the_http_layer() {
    let data = repo_root().join("server/src/data");
    let mut offenders: Vec<String> = Vec::new();
    let mut scanned = 0usize;

    let mut stack = vec![data.clone()];
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
                if line.contains("crate::api") {
                    offenders.push(format!("{relative}:{}: {}", offset + 1, line.trim()));
                }
            }
        }
    }

    assert!(
        scanned > 50,
        "only scanned {scanned} files under server/src/data - the walk is not reaching the tree"
    );
    assert!(
        offenders.is_empty(),
        "the storage layer reaches into the HTTP layer in {} place(s), which the crate split will refuse \
         to compile:\n  {}",
        offenders.len(),
        offenders.join("\n  ")
    );
}
