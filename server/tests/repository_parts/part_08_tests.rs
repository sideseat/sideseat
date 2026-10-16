/// The one account a home directory may name in tracked content: `/home/user`, which the capture's path
/// normalisation writes for every home and checkout (`examples/python/harness/harness/paths.py`) and documentation
/// uses for `~`. A placeholder account under `/Users/` (the old `sideseat`, `test-user`) is still a home path, of
/// a machine layout no reader needs, and is reported.
const HOME_PLACEHOLDER: &str = "user";

/// The home paths tracked files name on purpose: the file, exactly the homes it may name - each as the prefix the
/// sweep reports it under and the account - and why. Exact homes rather than whole files, so a real account path
/// added to an exhibit is still reported; split in two, so this list does not name them itself.
/// A home as the sweep reports it: its prefix and its account.
type ExhibitedHome = (&'static str, &'static str);

const HOME_PATH_EXHIBITS: [(&str, &[ExhibitedHome], &str); 5] = [
    (
        "examples/python/harness/harness/paths.py",
        &[("/Users/", "sideseat"), ("/Users/", "test-user")],
        "the committed capture layouts the path normalisation rewrites, which it must name to find",
    ),
    (
        "examples/python/harness/tests/test_paths.py",
        &[
            ("/Users/", "sideseat"),
            ("/Users/", "test-user"),
            ("/Users/", "Jdoe1"),
            ("/Users/", "someone"),
            ("/Users/", "user"),
            ("/Users/", "al"),
            ("/Users/", "alice"),
            ("/Users/", "bob"),
            ("/Users/", "José"),
            ("/Users/", "alice😀"),
            ("/Users/", "😀"),
            ("/home/", "a.b"),
            ("/home/", "ab"),
            ("/home/", "username"),
            (":\\Users\\", "alice"),
            (":\\Users\\", "Alice Smith"),
            (":\\Users\\", "bob"),
            (":\\Users\\", "李"),
        ],
        "the home and checkout paths the path normalisation's tests feed in",
    ),
    (
        "examples/python/harness/tests/test_scrub.py",
        &[("/Users/", "Jdoe1"), ("/Users/", "sides")],
        "foreign accounts the account scrub's tests feed in, and what it cuts them to",
    ),
    (
        "examples/python/harness/tests/test_truth.py",
        &[("/Users/", "someone"), ("/home/", "a.b")],
        "home paths the truth's anonymisation tests feed in",
    ),
    (
        "examples/python/harness/tests/test_truth_requests.py",
        &[("/Users/", "someone")],
        "a home path a transcript test feeds in",
    ),
];

/// How deep JSON strings and base64 may nest inside one file before the sweep refuses it rather than read less:
/// each layer is shorter than the one holding it, and no recording nests anywhere near this.
const MAX_BASE64_DEPTH: usize = 32;

/// The running account's home, when it is not the placeholder, as its path components: a home outside `/Users/`
/// and `/home/` has no shape the generic search below sees.
fn own_home() -> Option<Vec<String>> {
    let home = std::env::var("HOME").ok()?;
    let home = home.trim_end_matches('/');
    (home.starts_with('/') && home != format!("/home/{HOME_PLACEHOLDER}") && home.len() > 1).then(
        || {
            home.split('/')
                .filter(|part| !part.is_empty())
                .map(str::to_string)
                .collect()
        },
    )
}

fn is_name_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.') || b >= 0x80
}

/// The width of the name character at the start of `rest`, if it is one: a word character, `.`, `-`, or a
/// non-ASCII character but whitespace and the ellipsis a preview truncates with - as the normalisation reads it.
/// Plain text: JSON escapes are read by [`decoded_layers`], which hands this each string decoded.
fn name_char(rest: &[u8]) -> Option<usize> {
    let first = *rest.first()?;
    if first.is_ascii() {
        return is_name_byte(first).then_some(1);
    }
    let width = match first {
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF7 => 4,
        _ => 1,
    };
    let c = std::str::from_utf8(rest.get(..width)?)
        .ok()
        .and_then(|text| text.chars().next());
    match c {
        Some(c) if c.is_whitespace() || c == '…' => None,
        _ => Some(width),
    }
}

/// The length of the account name at the start of `rest`. A Windows account may hold single spaces where the
/// path goes on past it (`Alice Smith\x`).
fn name_length(rest: &[u8], windows: Option<u8>) -> usize {
    let word = |from: usize| -> usize {
        let mut at = from;
        while let Some(width) = name_char(&rest[at..]) {
            at += width;
        }
        at
    };
    let first = word(0);
    let Some(separator) = windows else {
        return first;
    };
    let mut end = first;
    let mut at = first;
    while first > 0 && rest.get(at) == Some(&b' ') {
        let next = word(at + 1);
        if next == at + 1 {
            break;
        }
        at = next;
        if rest.get(at) == Some(&separator) {
            end = at;
        }
    }
    end
}

/// Whether a path starting at `at` starts an absolute path, as the normalisation decides it: no word character,
/// `.`, `-`, `+` or `/` before it (`pages/home/x` is a module path, and a base64 run holds `+` and `/`). A
/// protobuf payload has no such boundary - a string's length prefix is a byte like any other, and one that reads
/// as a letter would hide the path behind it - so there every occurrence counts.
fn starts_a_path(layer: &[u8], at: usize, binary: bool) -> bool {
    binary || at == 0 || (!matches!(layer[at - 1], b'+' | b'/') && word_ends_before(layer, at))
}

/// Whether no word continues up to `at`: the start, an ASCII byte that is no name character, or a non-ASCII
/// character that is not a letter, digit or underscore - as the normalisation's `\w` decides.
fn word_ends_before(layer: &[u8], at: usize) -> bool {
    if at == 0 {
        return true;
    }
    let before = layer[at - 1];
    if before.is_ascii() {
        return !is_name_byte(before);
    }
    let start = (at.saturating_sub(4)..at)
        .find(|&i| (layer[i] & 0xC0) != 0x80)
        .unwrap_or(at - 1);
    let previous = std::str::from_utf8(&layer[start..at])
        .ok()
        .and_then(|text| text.chars().last());
    !previous.is_some_and(|c| c.is_alphanumeric() || c == '_')
}

/// Every home path of one decoded layer whose account is not the placeholder: `/Users/<name>`, `/home/<name>`,
/// and a Windows profile `<drive>:\Users\<name>` (or with `/`). Plain text only: what JSON escapes is read by
/// [`decoded_layers`], which hands this every string it decodes.
fn foreign_homes(layer: &[u8], own: Option<&[String]>, binary: bool, out: &mut BTreeSet<String>) {
    if let Some(own) = own {
        own_home_in(layer, own, binary, out);
    }
    for word in [&b"Users"[..], &b"home"[..]] {
        for found in memchr::memmem::find_iter(layer, word) {
            let Some(&separator) = found.checked_sub(1).map(|at| &layer[at]) else {
                continue;
            };
            let after = found + word.len();
            if layer.get(after) != Some(&separator) {
                continue;
            }
            let drive = word == b"Users"
                && found >= 3
                && layer[found - 2] == b':'
                && layer[found - 3].is_ascii_alphabetic()
                && matches!(separator, b'\\' | b'/')
                && starts_a_path(layer, found - 3, binary);
            if !(drive || separator == b'/' && starts_a_path(layer, found - 1, binary)) {
                continue;
            }
            // A `/Users/` after a drive is the rest of a Windows profile; after `bin:` (a `PATH` list) it is not.
            if !drive
                && word == b"Users"
                && found >= 3
                && layer[found - 2] == b':'
                && layer[found - 3].is_ascii_alphabetic()
                && word_ends_before(layer, found - 3)
            {
                continue;
            }
            let rest = &layer[after + 1..];
            let length = name_length(rest, drive.then_some(separator));
            if length == 0 {
                continue;
            }
            let head = String::from_utf8_lossy(&rest[..length.min(64)]).to_string();
            let shown = if length > 64 {
                format!("{head}…")
            } else {
                head.clone()
            };
            // A preview truncates, so a golden may hold the placeholder cut mid-name (`/home/us…`).
            let truncated = rest[length..].starts_with("…".as_bytes());
            let placeholder = head == HOME_PLACEHOLDER
                || (truncated && HOME_PLACEHOLDER.starts_with(head.as_str()));
            if placeholder && (drive || word == b"home") {
                continue;
            }
            out.insert(if drive {
                format!(":\\Users\\{shown}")
            } else {
                format!("/{}/{shown}", String::from_utf8_lossy(word))
            });
        }
    }
}

/// The running account's own home, component by component.
fn own_home_in(layer: &[u8], own: &[String], binary: bool, out: &mut BTreeSet<String>) {
    let joined = format!("/{}", own.join("/"));
    for found in memchr::memmem::find_iter(layer, joined.as_bytes()) {
        let end = found + joined.len();
        if starts_a_path(layer, found, binary) && name_char(&layer[end..]).is_none() {
            out.insert(joined.clone());
        }
    }
}

/// The end of the JSON string token opening at `open` (a `"`), exclusive, if it closes on the same line.
fn string_end(raw: &[u8], open: usize) -> Option<usize> {
    let mut at = open + 1;
    while let Some(&b) = raw.get(at) {
        match b {
            b'"' => return Some(at + 1),
            // An escaped line break is no JSON escape: the token ends unread, as the normalisation's does.
            b'\\' if matches!(raw.get(at + 1), Some(b'\n' | b'\r')) => return None,
            b'\\' => at += 2,
            b'\n' | b'\r' => return None,
            _ => at += 1,
        }
    }
    None
}

/// A layer read as the normalisation reads it (`harness.paths._text`): the text between JSON string tokens as
/// plain text, every token decoded - its escapes read for what they mean, the documents nested in it read the
/// same way - and a token that is all base64 decoded too; a token that is not JSON (a quoted Windows path in
/// prose) as plain text. `visit` sees plain text only, so no escape is ever read twice or in the wrong layer.
/// Deeper than [`MAX_BASE64_DEPTH`] is reported rather than skipped.
fn decoded_layers(raw: &[u8], depth: usize, visit: &mut dyn FnMut(&[u8]), too_deep: &mut bool) {
    use base64::Engine as _;
    let mut at = 0;
    let mut plain_from = 0;
    while let Some(open) = memchr::memchr(b'"', &raw[at..]).map(|found| at + found) {
        let Some(end) = string_end(raw, open) else {
            at = open + 1;
            continue;
        };
        visit(&raw[plain_from..open]);
        plain_from = end;
        at = end;
        let inner = &raw[open + 1..end - 1];
        let value = if memchr::memchr(b'\\', inner).is_some() {
            serde_json::from_slice::<String>(&raw[open..end])
                .ok()
                .map(String::into_bytes)
        } else {
            Some(inner.to_vec())
        };
        let Some(value) = value else {
            visit(&raw[open..end]);
            continue;
        };
        if depth == 0 {
            *too_deep = true;
            continue;
        }
        let base64_like = value.len() >= 8
            && value
                .iter()
                .rev()
                .skip_while(|b| **b == b'=')
                .all(|b| b.is_ascii_alphanumeric() || *b == b'+' || *b == b'/');
        if base64_like && let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(&value)
        {
            decoded_layers(&decoded, depth - 1, visit, too_deep);
        }
        decoded_layers(&value, depth - 1, visit, too_deep);
    }
    visit(&raw[plain_from..]);
}

/// The length-delimited fields of one protobuf message, `None` where `raw` is not one message.
fn protobuf_fields(raw: &[u8]) -> Option<Vec<std::ops::Range<usize>>> {
    fn varint(raw: &[u8], at: &mut usize) -> Option<u64> {
        let mut value = 0u64;
        for shift in (0..64).step_by(7) {
            let byte = *raw.get(*at)?;
            *at += 1;
            value |= u64::from(byte & 0x7F) << shift;
            if byte & 0x80 == 0 {
                return Some(value);
            }
        }
        None
    }
    let mut fields = Vec::new();
    let mut at = 0;
    while at < raw.len() {
        let key = varint(raw, &mut at)?;
        if key >> 3 == 0 {
            return None;
        }
        match key & 7 {
            0 => {
                varint(raw, &mut at)?;
            }
            1 => at += 8,
            5 => at += 4,
            2 => {
                let length = usize::try_from(varint(raw, &mut at)?).ok()?;
                let end = at.checked_add(length).filter(|end| *end <= raw.len())?;
                fields.push(at..end);
                at = end;
            }
            _ => return None,
        }
    }
    (at == raw.len()).then_some(fields)
}

/// Every length-delimited field of a protobuf message, at any depth, handed to `visit`: a field that parses as a
/// message is descended into as well, since the wire format does not say which a string is. Each message is
/// parsed once. Whether `raw` itself parses; one that does not visits nothing.
fn protobuf_strings(
    raw: &[u8],
    depth: usize,
    visit: &mut dyn FnMut(&[u8]),
    too_deep: &mut bool,
) -> bool {
    let Some(fields) = protobuf_fields(raw) else {
        return false;
    };
    for range in fields {
        let field = &raw[range];
        // A string is UTF-8; a nested message usually is not, and read as text its length bytes could pair
        // into quotes the string never had.
        if std::str::from_utf8(field).is_ok() {
            visit(field);
        }
        // Each nested message is shorter than the one holding it, so the descent ends; a nesting deeper than
        // any export holds is reported rather than left unread.
        if depth < MAX_PROTOBUF_DEPTH {
            protobuf_strings(field, depth + 1, visit, too_deep);
        } else if protobuf_fields(field).is_some() {
            *too_deep = true;
        }
    }
    true
}

/// How deep the sweep follows nested protobuf messages: OTLP nests a value a few levels below its export, and an
/// array value one level per array.
const MAX_PROTOBUF_DEPTH: usize = 256;

/// The longest archive line the sweep holds: one export, which the server refuses far below this.
const MAX_ARCHIVE_LINE: usize = 256 << 20;

/// Every newline-terminated line of a stream, read in bounded chunks into one buffer; a line longer than
/// [`MAX_ARCHIVE_LINE`] is an error rather than an allocation the archive decides.
fn for_each_line(
    mut reader: impl std::io::Read,
    mut visit: impl FnMut(&[u8]),
) -> std::io::Result<()> {
    let mut chunk = vec![0u8; 1 << 20];
    let mut line: Vec<u8> = Vec::new();
    loop {
        let read = reader.read(&mut chunk)?;
        if read == 0 {
            if !line.is_empty() {
                visit(&line);
            }
            return Ok(());
        }
        let mut start = 0;
        while start < read {
            let end = memchr::memchr(b'\n', &chunk[start..read]).map(|at| start + at);
            let piece = &chunk[start..end.unwrap_or(read)];
            if line.len() + piece.len() > MAX_ARCHIVE_LINE {
                return Err(std::io::Error::other(format!(
                    "a line longer than {MAX_ARCHIVE_LINE} bytes"
                )));
            }
            line.extend_from_slice(piece);
            match end {
                Some(end) => {
                    visit(&line);
                    line.clear();
                    start = end + 1;
                }
                None => break,
            }
        }
    }
}

/// The stored bytes of a file, scanned in fixed chunks that overlap by more than any home path the scan reports,
/// so a path across a chunk boundary is seen whole and a path inside the overlap is reported once (a set).
fn scan_stored(
    path: &std::path::Path,
    own: Option<&[String]>,
    found: &mut BTreeSet<String>,
) -> std::io::Result<()> {
    use std::io::Read;
    const CHUNK: usize = 1 << 20;
    const OVERLAP: usize = 4096;
    let mut file = std::fs::File::open(path)?;
    let mut window = vec![0u8; OVERLAP + CHUNK];
    let mut kept = 0;
    loop {
        let read = file.read(&mut window[kept..])?;
        if read == 0 {
            if kept > 0 {
                foreign_homes(&window[..kept], own, true, found);
            }
            return Ok(());
        }
        let filled = kept + read;
        foreign_homes(&window[..filled], own, true, found);
        kept = filled.min(OVERLAP);
        window.copy_within(filled - kept..filled, 0);
    }
}

/// No **tracked file** names a home directory other than the placeholder - in any decoded layer, compressed
/// archives included.
///
/// Captured payloads hold absolute paths from the machine that recorded them: tracebacks name the checkout, an
/// interpreter names the account's home. The capture normalises them (`harness.paths`), and committed recordings
/// were normalised by the same function (`python -m harness normalise`). The sweep reads every tracked file, the
/// base64 bodies of transcripts and cassettes, and every line of the gzip replay archives with their headers, and
/// reports the running account's `$HOME` and any home path whose account is not [`HOME_PLACEHOLDER`] under
/// `/home/`, beyond the exact homes [`HOME_PATH_EXHIBITS`] names. A tracked file that cannot be read fails the
/// sweep; one deleted from the working tree is not there to publish anything.
#[test]
fn no_tracked_file_names_a_home_directory() {
    let repo = repo_root();
    let listing = Command::new("git")
        .args(["ls-files", "-z"])
        .current_dir(repo)
        .output()
        .expect("git is available in a git checkout");
    assert!(listing.status.success(), "git ls-files failed");
    let exhibits: BTreeMap<&str, &[ExhibitedHome]> = HOME_PATH_EXHIBITS
        .iter()
        .map(|(file, homes, _)| (*file, *homes))
        .collect();
    let own = own_home();
    let mut offenders: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let (mut scanned, mut archives, mut tracked_archives) = (0usize, 0usize, 0usize);
    let mut exhibited = BTreeSet::new();
    for rel in listing.stdout.split(|b| *b == 0).filter(|e| !e.is_empty()) {
        let rel = String::from_utf8_lossy(rel).to_string();
        let path = repo.join(&rel);
        let archive = rel.ends_with(".gz");
        tracked_archives += usize::from(archive);
        let mut found = BTreeSet::new();
        let mut too_deep = false;
        let binary = rel.ends_with(".pb");
        let opened = if archive {
            std::fs::File::open(&path).map(|_| Vec::new())
        } else {
            std::fs::read(&path)
        };
        match opened {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                found.insert(format!("could not be read ({error})"));
            }
            Ok(_) if archive => {
                // The compressed bytes as stored, for what a member header holds (a file name, a comment), then
                // line by line decompressed: both streamed, since an archive is tens of megabytes compressed and
                // hundreds decompressed, and every line is one export.
                if let Err(error) = scan_stored(&path, own.as_deref(), &mut found) {
                    found.insert(format!("could not be read ({error})"));
                }
                let reader = std::io::BufReader::new(flate2::read::MultiGzDecoder::new(
                    std::io::BufReader::new(std::fs::File::open(&path).expect("opened above")),
                ));
                let read_all = for_each_line(reader, |line| {
                    decoded_layers(
                        line,
                        MAX_BASE64_DEPTH,
                        &mut |layer| foreign_homes(layer, own.as_deref(), false, &mut found),
                        &mut too_deep,
                    );
                });
                if let Err(error) = &read_all {
                    found.insert(format!("could not be read ({error})"));
                }
                archives += usize::from(read_all.is_ok());
            }
            // A protobuf export is read field by field, each string as text with the normalisation's own
            // boundaries; one that does not parse is searched whole with none.
            Ok(raw) if binary && protobuf_fields(&raw).is_some() => {
                let mut nested_too_deep = false;
                protobuf_strings(
                    &raw,
                    0,
                    &mut |string| {
                        decoded_layers(
                            string,
                            MAX_BASE64_DEPTH,
                            &mut |layer| foreign_homes(layer, own.as_deref(), false, &mut found),
                            &mut too_deep,
                        );
                    },
                    &mut nested_too_deep,
                );
                too_deep |= nested_too_deep;
            }
            Ok(raw) => decoded_layers(
                &raw,
                MAX_BASE64_DEPTH,
                &mut |layer| foreign_homes(layer, own.as_deref(), binary, &mut found),
                &mut too_deep,
            ),
        }
        scanned += 1;
        if too_deep {
            found.insert(format!(
                "base64 nested deeper than {MAX_BASE64_DEPTH} layers"
            ));
        }
        if let Some(allowed) = exhibits.get(rel.as_str()) {
            exhibited.insert(rel.clone());
            let named: BTreeSet<String> = found.iter().cloned().collect();
            let allowed: Vec<String> = allowed
                .iter()
                .map(|(prefix, name)| format!("{prefix}{name}"))
                .collect();
            found.retain(|home| !allowed.contains(home));
            let unused: Vec<&String> = allowed
                .iter()
                .filter(|home| !named.contains(*home))
                .collect();
            if !unused.is_empty() {
                found.insert(format!(
                    "exhibits homes it no longer names, which the list must drop: {unused:?}"
                ));
            }
        }
        if !found.is_empty() {
            offenders.insert(rel, found);
        }
    }
    assert!(scanned > 1_000, "only scanned {scanned} tracked files");
    assert_eq!(
        archives, tracked_archives,
        "{tracked_archives} compressed archive(s) are tracked and {archives} were read whole"
    );
    let missing: Vec<&&str> = exhibits
        .keys()
        .filter(|f| !exhibited.contains(**f))
        .collect();
    assert!(
        missing.is_empty(),
        "exhibits that are no longer tracked, which the list must drop: {missing:?}"
    );
    assert!(
        offenders.is_empty(),
        "{} tracked file(s) name a home directory - normalise them (`uv run --locked --directory \
         examples/python/harness python -m harness normalise`), which writes /home/{HOME_PLACEHOLDER}:\n  {}",
        offenders.len(),
        offenders
            .iter()
            .map(|(file, homes)| format!(
                "{file}: {}",
                homes.iter().cloned().collect::<Vec<_>>().join(", ")
            ))
            .collect::<Vec<_>>()
            .join("\n  ")
    );
}

/// The sweep's matcher on every spelling the normalisation writes or reads, the paths built at run time so this
/// file names none of them.
#[test]
fn the_home_sweep_sees_every_spelling() {
    let users = ["Us", "ers"].concat();
    let found = |text: String, binary: bool| {
        let mut out = BTreeSet::new();
        let mut deep = false;
        decoded_layers(
            text.as_bytes(),
            MAX_BASE64_DEPTH,
            &mut |layer| foreign_homes(layer, None, binary, &mut out),
            &mut deep,
        );
        out
    };
    let named = |name: &str| BTreeSet::from([format!("/{users}/{name}")]);
    let windows = |name: &str| BTreeSet::from([format!(":\\{users}\\{name}")]);
    assert_eq!(
        found(format!("\"/{users}/alice/x\""), false),
        named("alice")
    );
    // Every escape JSON may write, at any depth, read for what it means.
    assert_eq!(
        found(format!(r#""\/{users}\/alice\/x""#), false),
        named("alice")
    );
    assert_eq!(
        found(format!(r#""/{users}\/alice/x""#), false),
        named("alice")
    );
    let home = ["ho", "me"].concat();
    assert!(
        found(format!(r#""/{home}/user\u006eame/x""#), false)
            .contains(&format!("/{home}/username"))
    );
    assert!(found(format!(r#""/{users}/Jos\u00e9/x""#), false).contains(&format!("/{users}/José")));
    assert_eq!(
        found(
            format!(r#""\"{{\\\"p\\\": \\\"/{users}/bob/x\\\"}}\"""#),
            false
        ),
        named("bob")
    );
    assert_eq!(
        found(format!(r#""/{home}/user\u002fx""#), false),
        BTreeSet::new()
    );
    assert_eq!(
        found(format!(r#""page\u0073/{home}/create""#), false),
        BTreeSet::new()
    );
    assert_eq!(
        found(format!(r#""/{home}/us\u2026""#), false),
        BTreeSet::new()
    );
    assert_eq!(
        found(format!(r#""\n/{users}/alice/x""#), false),
        named("alice")
    );
    // Windows profiles, as text and in JSON.
    assert_eq!(
        found(format!(r"C:\{users}\Alice Smith\x"), false),
        windows("Alice Smith")
    );
    assert_eq!(
        found(format!(r#""C:\\{users}\\\u674e\\x""#), false),
        windows("李")
    );
    assert_eq!(
        found(format!(r"C:\{users}\user\x and D:/{users}/user/y"), false),
        BTreeSet::new()
    );
    assert_eq!(
        found(format!("x\u{a0}C:\\{users}\\alice\\x"), false),
        windows("alice")
    );
    assert_eq!(
        found(format!("pages/{users}/alice"), false),
        BTreeSet::new()
    );
    assert_eq!(
        found(format!("/usr/bin:/{users}/bob/y"), false),
        named("bob")
    );
    // A literal backslash-u in a decoded string is text, as the normalisation reads it.
    assert_eq!(
        found(format!(r#""/{home}/user\\u006e/x""#), false),
        BTreeSet::new()
    );
    assert_eq!(
        found(format!("C:/{home}/alice/x"), false),
        BTreeSet::from([format!("/{home}/alice")])
    );
    // A protobuf message read field by field: a module path in a string keeps its boundary.
    let string = format!("pages/{home}/create and /{users}/alice/x");
    let mut message = vec![0x0A, u8::try_from(string.len()).expect("short")];
    message.extend_from_slice(string.as_bytes());
    let mut out = BTreeSet::new();
    let mut deep = false;
    assert!(protobuf_strings(
        &message,
        0,
        &mut |field| foreign_homes(field, None, false, &mut out),
        &mut deep
    ));
    assert!(!deep);
    assert_eq!(out, named("alice"));
    assert_eq!(
        found(format!(r#""/{home}/us\u0065r/x""#), false),
        BTreeSet::new()
    );
    assert_eq!(found(format!("/{home}/us\u{2026}"), false), BTreeSet::new());
    assert_eq!(found(format!(r#""/{home}/us…""#), false), BTreeSet::new());
    assert_eq!(
        found(format!("/{home}/user\u{a0}failed"), false),
        BTreeSet::new()
    );
    assert_eq!(
        found(format!("x\u{a0}/{users}/alice/x"), false),
        named("alice")
    );
    // A protobuf length prefix that reads as a letter hides nothing.
    assert_eq!(found(format!("A/{users}/alice/x"), true), named("alice"));
    assert_eq!(
        found(format!("/{users}/{}", "a".repeat(80)), false).len(),
        1
    );
    // Base64, its `/` escaped or not.
    use base64::Engine as _;
    let inner = base64::engine::general_purpose::STANDARD.encode(format!("aa? /{users}/alice/x"));
    assert_eq!(
        found(
            format!("{{\"b\":\"{}\"}}", inner.replace('/', "\\/")),
            false
        ),
        named("alice")
    );
    // The running account's home outside the usual roots, however its letters are spelled.
    for spelled in [
        r#""/srv/Jos\u00e9/x""#,
        r#""/srv/Jos\u00E9/x""#,
        "\"/srv/José/x\"",
    ] {
        let mut out = BTreeSet::new();
        let mut deep = false;
        let own = ["srv".to_string(), "José".to_string()];
        decoded_layers(
            spelled.as_bytes(),
            MAX_BASE64_DEPTH,
            &mut |layer| foreign_homes(layer, Some(&own), false, &mut out),
            &mut deep,
        );
        assert_eq!(out, BTreeSet::from(["/srv/José".to_string()]), "{spelled}");
    }
    let mut out = BTreeSet::new();
    let mut deep = false;
    let own = ["srv".to_string(), "alice".to_string()];
    decoded_layers(
        br#""/srv/alice\u00e9/x""#,
        MAX_BASE64_DEPTH,
        &mut |layer| foreign_homes(layer, Some(&own), false, &mut out),
        &mut deep,
    );
    assert!(out.is_empty(), "{out:?}");
}
