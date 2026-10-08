//! Attachments, searched for by what identifies them: inline bytes by their digest or a piece of their
//! encoding, and an attachment sent without its bytes by the place it names.

use std::collections::BTreeMap;

use super::haystack::Haystack;
use super::{MIN_ID, Proof};
use crate::message_truth::truth;

/// An attachment is present where a payload decodes to its bytes, and partly present where a payload's text
/// holds a piece of its base64 encoding: an attachment written inside some other text - a Java `toString()`
/// naming `base64Data = "..."` - is never isolated as a value of its own, so a digest search alone would
/// call it absent. Without the attachment's bytes the second search cannot run, and absence is unprovable.
pub(super) fn prove_attachment(digest: &str, haystack: &Haystack) -> Proof {
    if let Some((at, _)) = haystack
        .carriers
        .iter()
        .flat_map(|c| &c.digests)
        .find(|(_, d)| d == digest)
    {
        return Proof::Present(at.clone());
    }
    let Some(bytes) = script_asset(digest) else {
        return Proof::Unprovable(format!(
            "the attachment {digest} is none of the script assets, so its encoding cannot be searched for"
        ));
    };
    match find_base64_piece(bytes, haystack) {
        Some(at) => Proof::Partial(at),
        None => Proof::Absent,
    }
}

/// The bytes of a script asset by SHA-256: the attachments the scenarios send, read from the files they
/// send them from.
fn script_asset(digest: &str) -> Option<&'static [u8]> {
    use sha2::Digest as _;
    static ASSETS: std::sync::OnceLock<BTreeMap<String, Vec<u8>>> = std::sync::OnceLock::new();
    ASSETS
        .get_or_init(|| {
            let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/assets");
            std::fs::read_dir(&dir)
                .unwrap_or_else(|e| panic!("the script assets at {}: {e}", dir.display()))
                .flatten()
                .filter(|entry| entry.path().is_file())
                .map(|entry| {
                    let bytes = std::fs::read(entry.path()).expect("a script asset reads");
                    (truth::hex_digest(&sha2::Sha256::digest(&bytes)), bytes)
                })
                .collect()
        })
        .get(digest)
        .map(Vec::as_slice)
}

/// Where a payload string holds a piece of these bytes' base64 encoding, in either alphabet.
///
/// Pieces of 48 bytes, at offsets on a 3-byte boundary, encode to the same 64 characters inside any
/// encoding of the whole: one from the start, and one from the middle for a payload that holds the
/// encoding cut short of its beginning. Whitespace is ignored, so a line-wrapped encoding is found too.
fn find_base64_piece(bytes: &[u8], haystack: &Haystack) -> Option<String> {
    use base64::Engine as _;
    use base64::engine::general_purpose::{STANDARD, URL_SAFE};
    const PIECE: usize = 48;
    let pieces: Vec<String> = [0, bytes.len() / 2 / 3 * 3]
        .into_iter()
        .filter(|offset| offset + PIECE <= bytes.len())
        .flat_map(|offset| {
            let piece = &bytes[offset..offset + PIECE];
            [STANDARD.encode(piece), URL_SAFE.encode(piece)]
        })
        .collect();
    haystack
        .carriers
        .iter()
        .flat_map(|c| &c.strings)
        .find(|(_, text)| {
            let compact: String = text.chars().filter(|c| !c.is_whitespace()).collect();
            pieces.iter().any(|piece| compact.contains(piece.as_str()))
        })
        .map(|(at, _)| at.clone())
}

/// An attachment sent without its bytes is present wherever the place it names is: a payload string that is
/// the reference, or holds it as a token of its own. A longer id or URL that merely begins with it names
/// another place, and a shared host or prefix proves nothing, so a reference is never partly present.
pub(super) fn prove_reference(reference: &str, haystack: &Haystack) -> Proof {
    if reference.chars().count() < MIN_ID {
        return Proof::Unprovable(format!(
            "{reference:?} is too short to tell apart from other content"
        ));
    }
    let continues = |c: char| c.is_alphanumeric() || matches!(c, '-' | '_');
    let holds = |text: &str| {
        text.match_indices(reference).any(|(at, _)| {
            let before = text[..at].chars().next_back();
            let after = text[at + reference.len()..].chars().next();
            !before.is_some_and(continues) && !after.is_some_and(continues)
        })
    };
    match haystack.carriers.iter().find_map(|carrier| {
        carrier
            .strings
            .iter()
            .find(|(_, text)| holds(text))
            .map(|(at, _)| at.as_str())
    }) {
        Some(at) => Proof::Present(at.to_string()),
        None => Proof::Absent,
    }
}
