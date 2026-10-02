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

/// The root Makefile followed by every fragment it includes, in include order.
fn makefile_sources() -> String {
    let root = std::fs::read_to_string(repo_root().join("Makefile")).expect("Makefile");
    let mut combined = root.clone();
    for line in root.lines() {
        if let Some(fragment) = line.strip_prefix("include ") {
            let path = repo_root().join(fragment.trim());
            combined.push('\n');
            combined.push_str(&std::fs::read_to_string(&path).unwrap_or_else(|error| {
                panic!("{} is included but unreadable: {error}", path.display())
            }));
        }
    }
    combined
}

#[test]
fn authorization_membership_checks_are_not_cached() {
    let source = std::fs::read_to_string(repo_root().join("server/crates/api/src/auth/context.rs"))
        .expect("auth context is readable");

    assert!(
        source.contains(".get_membership(org_id, user_id)"),
        "authorization must read current membership"
    );
    assert!(
        !source.contains("CacheKey::user_org_member")
            && !source.contains("CACHE_TTL_USER_ORG_MEMBER"),
        "membership revocation must take effect on the next request"
    );
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
/// Comments and literals are handled in one lexical pass so quotes inside comments and comment markers inside
/// strings remain inert.
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
/// mutable by whoever owns that repository, so only a full commit identifies reviewed code.
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
/// new build or deployment file cannot be added outside the rule.
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
                // A local action is maintained as part of this repository.
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
/// Captured payloads can include absolute paths from the machine that produced them. The sweep reports Unix,
/// macOS, and Windows home-directory shapes unless the account segment is one of [`HOME_PLACEHOLDERS`]. It
/// reads every tracked file and decompresses gzip archives, where captured paths are common and difficult to
/// notice. Archive counts ensure decoder failures cannot silently reduce coverage.
///
/// The check intentionally targets path shapes rather than arbitrary account-name occurrences. Names in
/// author fields, hostnames, or other non-path data are outside its scope.
///
/// A prefix must sit at an **absolute-path boundary**: `@/pages/home/create-project-dialog` is a module import,
/// not a home directory, and reporting it would teach a reader to disbelieve the finding.
#[test]
fn no_tracked_file_carries_the_capturing_users_name() {
    use std::io::Read;

    // A JSON fixture may carry Windows separators literally or escaped, so both byte forms are checked.
    let prefixes: [&[u8]; 5] = [
        b"/Users/",
        b"/home/",
        br"C:\Users\",
        br"C:\\Users\\",
        b"C:/Users/",
    ];
    // Only tracked files can publish a captured account path; ignored local fixtures are outside this property.
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
            // Gzip files may contain concatenated members, so every member must be scanned.
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
/// A block qualifies when its first line or nearest heading names a repository directory. Conceptual trees,
/// span hierarchies, and runtime data directories are excluded. A repository-shaped root that resolves to
/// nothing is an error rather than a skipped diagram.
///
/// Entries resolve as whole paths reconstructed from exact four-column indentation, so both names and
/// parentage are checked. Invalid indentation is reported rather than guessed.
///
/// A name that resolves to nothing tracked is accepted when **git ignores it**, which is committed information.
/// This permits diagrams to document generated directories without making the result depend on local build
/// artifacts.
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
