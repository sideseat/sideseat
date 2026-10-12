//! A text a payload holds only in pieces.
//!
//! A segmented or streamed export cuts a text into pieces - the parts of a list, the events or log records of
//! one span - so no one string holds a fragment the search looks for. The pieces are a span's strings that sit
//! under an index - `parts.3.text`, `events[2]...`, `output.0|json[1]` - and the strings of every log record the
//! span emitted, in any export. The text is exported when one of its fragments - its head, its tail, a whole
//! sentence, or all of a text too short to have one - is a run of whole pieces, the first allowed to be the end
//! of a longer piece and the last its start, in any order: what order a producer gave its pieces (indices, event
//! times, record timestamps, export files) never decides. Each piece is every representation the search read
//! (as written and as decoded, decoded bytes that are no text alone included), compared as bytes without
//! whitespace, since each piece's own edges are collapsed.
//!
//! A refutation that joins too much refuses a gap the capture would have proven, which the corpus sweep shows;
//! one that joins too little accepts a gap over exported content, which nothing would. Strings that sit under
//! no index of a span - its own metadata, words the text happens to quote - are never pieces.

use std::collections::{BTreeMap, HashSet};

use super::haystack::Carrier;

/// Whether a location, within its carrier, sits under an index: `.N` followed by the end, a `.`, a `[` or a
/// decoding annotation, or `[N]`. Decoding annotations (`|json`, `|base64`) are read past.
pub(super) fn is_indexed(within: &str) -> bool {
    let at = undecorated(within);
    let bytes = at.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() && i > 0 && matches!(bytes[i - 1], b'.' | b'[') {
            let before = bytes[i - 1];
            let mut end = i;
            while end < bytes.len() && bytes[end].is_ascii_digit() {
                end += 1;
            }
            let after = bytes.get(end).copied();
            let indexed = (before == b'.' && matches!(after, None | Some(b'.') | Some(b'[')))
                || (before == b'[' && after == Some(b']'));
            if indexed {
                return true;
            }
            i = end;
        } else {
            i += 1;
        }
    }
    false
}

/// Whether a carrier's string at `at` is a piece: anything a log record holds, or what sits under an index within
/// the carrier (its root - export file, span or record - aside).
pub(super) fn is_piece(carrier: &Carrier, at: &str) -> bool {
    carrier.record || is_indexed(at.strip_prefix(carrier.root.as_str()).unwrap_or(at))
}

/// A location without the decoding annotations the search adds (`|base64`, `|json`), its child path kept.
pub(super) fn undecorated(at: &str) -> String {
    let mut out = String::with_capacity(at.len());
    let mut chars = at.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '|' {
            while chars
                .peek()
                .is_some_and(|next| next.is_ascii_alphanumeric() || matches!(next, '_' | '-'))
            {
                chars.next();
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// A text's bytes without its whitespace: pieces are compared as bytes, so a character cut across two pieces -
/// each decoded alone, neither text alone - still joins.
fn squash(text: &str) -> Vec<u8> {
    text.chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .into_bytes()
}

/// Where the text is held in pieces, if it is: each span's carriers searched together, and every carrier of no
/// span on its own.
pub(super) fn in_pieces(carriers: &[Carrier], needle: &str) -> Option<String> {
    let whole = squash(needle);
    if whole.is_empty() {
        return None;
    }
    let mut anchors: Vec<Vec<u8>> = super::fragments(needle)
        .iter()
        .map(|fragment| squash(fragment))
        .collect();
    // Too short to have a fragment is counted in characters, as fragments are.
    if needle.chars().filter(|c| !c.is_whitespace()).count() <= super::FRAGMENT {
        anchors.push(whole);
    }
    let mut spans: BTreeMap<(&str, &str), Vec<&Carrier>> = BTreeMap::new();
    let mut alone: Vec<Vec<&Carrier>> = Vec::new();
    for carrier in carriers {
        match (carrier.trace.as_deref(), carrier.span.as_deref()) {
            (Some(trace), Some(span)) => spans.entry((trace, span)).or_default().push(carrier),
            _ => alone.push(vec![carrier]),
        }
    }
    spans.into_values().chain(alone).find_map(|group| {
        let mut first_at: BTreeMap<Vec<u8>, &str> = BTreeMap::new();
        for carrier in &group {
            for (at, value) in &carrier.strings {
                let piece = squash(value);
                if !piece.is_empty() && is_piece(carrier, at) {
                    first_at.entry(piece).or_insert(at.as_str());
                }
            }
            for (at, bytes) in &carrier.byte_pieces {
                for piece in byte_variants(bytes) {
                    if !piece.is_empty() {
                        first_at.entry(piece).or_insert(at.as_str());
                    }
                }
            }
        }
        let pieces: Vec<&Vec<u8>> = first_at.keys().collect();
        anchors
            .iter()
            .any(|anchor| covered(anchor, &pieces))
            .then(|| first_at.values().next().map(|at| at.to_string()))
            .flatten()
    })
}

/// A byte piece without whitespace, as the needle is compared: every whitespace character it holds whole
/// removed, and - since a piece may be cut inside one - each reading that drops a whitespace character's tail at
/// its start or its head at its end. The readings are alternatives, the piece as it stands among them.
fn byte_variants(bytes: &[u8]) -> Vec<Vec<u8>> {
    let encodings: Vec<Vec<u8>> = ('\0'..='\u{3000}')
        .filter(|c| c.is_whitespace())
        .map(|c| c.to_string().into_bytes())
        .collect();
    // Every encoding starts with a byte that begins a character, so one found here is that character.
    let mut core = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        match encodings.iter().find(|e| bytes[at..].starts_with(e)) {
            Some(encoding) => at += encoding.len(),
            None => {
                core.push(bytes[at]);
                at += 1;
            }
        }
    }
    let mut heads = vec![0];
    let mut tails = vec![0];
    for encoding in &encodings {
        for cut in 1..encoding.len() {
            if core.starts_with(&encoding[cut..]) {
                heads.push(encoding.len() - cut);
            }
            if core.ends_with(&encoding[..cut]) {
                tails.push(cut);
            }
        }
    }
    let mut variants = vec![bytes.to_vec()];
    for &head in &heads {
        for &tail in &tails {
            if head + tail <= core.len() {
                variants.push(core[head..core.len() - tail].to_vec());
            }
        }
    }
    variants
}

/// Whether `anchor` is a run of `pieces`, in any order and with repeats: whole pieces, the first allowed to be
/// the end of a longer piece and the last the start of one.
fn covered(anchor: &[u8], pieces: &[&Vec<u8>]) -> bool {
    let n = anchor.len();
    let whole: HashSet<&[u8]> = pieces.iter().map(|piece| piece.as_slice()).collect();
    let longest = pieces.iter().map(|piece| piece.len()).max().unwrap_or(0);
    // Where a run may start: the anchor's start, or just after a head of it that ends a longer piece.
    let mut reached = vec![false; n + 1];
    reached[0] = true;
    for (start, slot) in reached.iter_mut().enumerate().take(n).skip(1) {
        let head = &anchor[..start];
        *slot = pieces
            .iter()
            .any(|piece| piece.len() > start && piece.ends_with(head));
    }
    for at in 0..n {
        if !reached[at] {
            continue;
        }
        let rest = &anchor[at..];
        // The run ends inside a piece that starts with what remains.
        if pieces.iter().any(|piece| piece.starts_with(rest)) {
            return true;
        }
        for len in 1..=longest.min(n - at) {
            if whole.contains(&rest[..len]) {
                reached[at + len] = true;
            }
        }
    }
    reached[n]
}

#[cfg(test)]
mod tests {
    use super::{covered, is_indexed};

    #[test]
    fn a_location_is_indexed_by_a_position_in_a_list() {
        assert!(is_indexed("span.parts.3.text"));
        assert!(is_indexed("span.content[12].text"));
        assert!(is_indexed("span.messages.0"));
        assert!(is_indexed("span.output.0|json[1]"));
        assert!(is_indexed("span.parts.1.text|base64"));
        assert!(!is_indexed("span.process.cwd"));
        assert!(!is_indexed("req-001.pb span \"chat\".gpt4o"));
        assert!(!is_indexed("span.v1.2alpha"));
    }

    #[test]
    fn a_run_of_pieces_covers_an_anchor_in_any_order() {
        let bytes = |text: &str| text.as_bytes().to_vec();
        let pieces = [bytes("text"), bytes("Read"), bytes("exactly.")];
        let pieces: Vec<&Vec<u8>> = pieces.iter().collect();
        assert!(covered(&bytes("Readtextexactly."), &pieces));
        // The first piece may be the end of a longer one, the last the start of one.
        assert!(covered(&bytes("adtextexa"), &pieces));
        // A gap no piece fills is no run.
        assert!(!covered(&bytes("Readthetext"), &pieces));
    }
}
