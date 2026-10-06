//! The SideSeat raw record: an export exactly as it was received, with its media held by content hash.
//!
//! Raw telemetry is the authority every derived row is rebuilt from, so it is kept byte for byte - the
//! decompressed request body, not a re-encoding of the decoded message. Re-encoding would not be the producer's
//! bytes: field order, unknown fields, non-minimal varints and OTLP/JSON spelling all differ between encoders,
//! and a parsing defect discovered later could then not be re-parsed from what was actually sent.
//!
//! Media is the one thing that is not stored inline. A base64 run long enough to be an attachment is cut out and
//! stored once per project as its decoded bytes (the file store's content addressing, so a file a span already
//! extracted is the same object), and the record keeps where it was:
//!
//! ```text
//! "SSR1"  magic and version
//! u8      content: 0 = OTLP protobuf, 1 = OTLP/JSON
//! varint  number of media runs
//! per run, in order:
//!   varint  bytes of the original between the previous run's end and this run's start
//!   varint  length of the base64 run
//!   [32]    SHA-256 of the decoded bytes
//! ...     every byte of the original that is not inside a run
//! ```
//!
//! Decoding splices `base64(decoded)` back at each recorded position. Nothing about the payload's structure is
//! interpreted, so a non-canonical protobuf, a JSON export and a run that happens to start one byte early in the
//! framing all come back identically. A run is only cut out when encoding its decoded bytes reproduces it
//! exactly - padding, alphabet and length - so the splice is always the original text.

use std::borrow::Cow;
use std::collections::BTreeMap;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use sha2::{Digest, Sha256};
use thiserror::Error;

const MAGIC: &[u8; 4] = b"SSR1";

/// Shorter base64 is left inline: below this a reference costs about as much as the text, and short runs are
/// mostly identifiers, not attachments. The same threshold the media extraction and the corpus measurements use.
pub const MIN_MEDIA_RUN: usize = 256;

/// How the received body was encoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawContent {
    Protobuf,
    Json,
}

impl RawContent {
    fn tag(self) -> u8 {
        match self {
            Self::Protobuf => 0,
            Self::Json => 1,
        }
    }

    fn from_tag(tag: u8) -> Option<Self> {
        match tag {
            0 => Some(Self::Protobuf),
            1 => Some(Self::Json),
            _ => None,
        }
    }
}

/// One media object cut out of a payload: its SHA-256 and decoded bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawMedia {
    pub sha256: [u8; 32],
    pub bytes: Vec<u8>,
}

impl RawMedia {
    /// The hash as the file store spells it.
    pub fn hash_hex(&self) -> String {
        hex::encode(self.sha256)
    }
}

/// A received payload ready to store: the record, and the media it references, each listed once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedRaw {
    pub record: Vec<u8>,
    pub media: Vec<RawMedia>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum RawPayloadError {
    #[error("not a SideSeat raw record")]
    NotARecord,
    #[error("the raw record is truncated or malformed")]
    Malformed,
    #[error("media {0} referenced by the raw record is not available")]
    MissingMedia(String),
    #[error("media {0} does not re-encode to the recorded length")]
    MediaMismatch(String),
}

/// Cut the media out of a received payload.
pub fn encode(raw: &[u8], content: RawContent) -> EncodedRaw {
    let runs = media_runs(raw);
    let mut record = Vec::with_capacity(raw.len() + 16);
    record.extend_from_slice(MAGIC);
    record.push(content.tag());
    put_varint(&mut record, runs.len() as u64);
    let mut media: BTreeMap<[u8; 32], Vec<u8>> = BTreeMap::new();
    let mut cursor = 0usize;
    for run in &runs {
        put_varint(&mut record, (run.start - cursor) as u64);
        put_varint(&mut record, (run.end - run.start) as u64);
        record.extend_from_slice(&run.sha256);
        media
            .entry(run.sha256)
            .or_insert_with(|| run.decoded.clone());
        cursor = run.end;
    }
    let mut cursor = 0usize;
    for run in &runs {
        record.extend_from_slice(&raw[cursor..run.start]);
        cursor = run.end;
    }
    record.extend_from_slice(&raw[cursor..]);
    EncodedRaw {
        record,
        media: media
            .into_iter()
            .map(|(sha256, bytes)| RawMedia { sha256, bytes })
            .collect(),
    }
}

/// A record of the received payload with nothing cut out.
///
/// What a durability buffer holds before the media can be stored: it is the same format, so a reader of one
/// reads the other, and it is never the stored raw record - see [`encode`].
pub fn wrap(raw: &[u8], content: RawContent) -> Vec<u8> {
    let mut record = Vec::with_capacity(raw.len() + MAGIC.len() + 2);
    record.extend_from_slice(MAGIC);
    record.push(content.tag());
    put_varint(&mut record, 0);
    record.extend_from_slice(raw);
    record
}

/// Whether bytes are a raw record, as opposed to a payload staged before raw records existed.
pub fn is_record(bytes: &[u8]) -> bool {
    bytes.len() > MAGIC.len() && &bytes[..MAGIC.len()] == MAGIC
}

/// The media hashes a record references, in order of first appearance, without decoding the payload.
pub fn media_hashes(record: &[u8]) -> Result<Vec<[u8; 32]>, RawPayloadError> {
    let header = Header::parse(record)?;
    let mut seen = Vec::with_capacity(header.runs.len());
    for run in &header.runs {
        if !seen.contains(&run.sha256) {
            seen.push(run.sha256);
        }
    }
    Ok(seen)
}

/// Rebuild the received payload byte for byte. `media` answers the decoded bytes of a hash.
pub fn decode<'a, F>(record: &[u8], media: F) -> Result<(RawContent, Vec<u8>), RawPayloadError>
where
    F: Fn(&[u8; 32]) -> Option<Cow<'a, [u8]>>,
{
    let header = Header::parse(record)?;
    let body = &record[header.body_offset..];
    let mut out =
        Vec::with_capacity(body.len() + header.runs.iter().map(|r| r.length).sum::<usize>());
    let mut cursor = 0usize;
    for run in &header.runs {
        let end = cursor
            .checked_add(run.gap)
            .filter(|end| *end <= body.len())
            .ok_or(RawPayloadError::Malformed)?;
        out.extend_from_slice(&body[cursor..end]);
        cursor = end;
        let bytes = media(&run.sha256)
            .ok_or_else(|| RawPayloadError::MissingMedia(hex::encode(run.sha256)))?;
        let text = STANDARD.encode(bytes.as_ref());
        if text.len() != run.length {
            return Err(RawPayloadError::MediaMismatch(hex::encode(run.sha256)));
        }
        out.extend_from_slice(text.as_bytes());
    }
    out.extend_from_slice(&body[cursor..]);
    Ok((header.content, out))
}

struct RecordedRun {
    gap: usize,
    length: usize,
    sha256: [u8; 32],
}

struct Header {
    content: RawContent,
    runs: Vec<RecordedRun>,
    body_offset: usize,
}

impl Header {
    fn parse(record: &[u8]) -> Result<Self, RawPayloadError> {
        if record.len() < MAGIC.len() + 1 || &record[..MAGIC.len()] != MAGIC {
            return Err(RawPayloadError::NotARecord);
        }
        let content =
            RawContent::from_tag(record[MAGIC.len()]).ok_or(RawPayloadError::Malformed)?;
        let mut at = MAGIC.len() + 1;
        let count = get_varint(record, &mut at)?;
        // Each run takes at least 34 bytes of header, so a count beyond that is corrupt, not a reason to
        // allocate.
        if count > (record.len() / 34) as u64 {
            return Err(RawPayloadError::Malformed);
        }
        let mut runs = Vec::with_capacity(count as usize);
        for _ in 0..count {
            let gap = usize::try_from(get_varint(record, &mut at)?)
                .map_err(|_| RawPayloadError::Malformed)?;
            let length = usize::try_from(get_varint(record, &mut at)?)
                .map_err(|_| RawPayloadError::Malformed)?;
            let hash = record.get(at..at + 32).ok_or(RawPayloadError::Malformed)?;
            at += 32;
            let mut sha256 = [0u8; 32];
            sha256.copy_from_slice(hash);
            runs.push(RecordedRun {
                gap,
                length,
                sha256,
            });
        }
        Ok(Self {
            content,
            runs,
            body_offset: at,
        })
    }
}

struct MediaRun {
    start: usize,
    end: usize,
    sha256: [u8; 32],
    decoded: Vec<u8>,
}

fn is_base64(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'+' || byte == b'/'
}

/// Every base64 run of at least [`MIN_MEDIA_RUN`] characters whose decoding re-encodes to exactly itself.
///
/// A run is a maximal stretch of the standard alphabet plus up to two `=`. Unpadded, its length is trimmed to
/// a multiple of four, because the framing byte after a string can itself be an alphabet character; the trimmed
/// characters simply stay inline. Whatever the alignment, only exactly reproducible text is cut, so a run that
/// begins inside the framing is still restored exactly - it only fails to share an object with the
/// extracted file.
fn media_runs(raw: &[u8]) -> Vec<MediaRun> {
    let mut runs = Vec::new();
    let mut at = 0usize;
    while at < raw.len() {
        if !is_base64(raw[at]) {
            at += 1;
            continue;
        }
        let start = at;
        while at < raw.len() && is_base64(raw[at]) {
            at += 1;
        }
        let mut end = at;
        let mut padding = 0;
        while padding < 2 && end < raw.len() && raw[end] == b'=' {
            end += 1;
            padding += 1;
        }
        at = end;
        if padding == 0 {
            end = start + (end - start) / 4 * 4;
        }
        if end - start < MIN_MEDIA_RUN || !(end - start).is_multiple_of(4) {
            continue;
        }
        let text = &raw[start..end];
        let Ok(decoded) = STANDARD.decode(text) else {
            continue;
        };
        if STANDARD.encode(&decoded).as_bytes() != text {
            continue;
        }
        let sha256: [u8; 32] = Sha256::digest(&decoded).into();
        runs.push(MediaRun {
            start,
            end,
            sha256,
            decoded,
        });
    }
    runs
}

fn put_varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push((value as u8) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn get_varint(bytes: &[u8], at: &mut usize) -> Result<u64, RawPayloadError> {
    let mut value = 0u64;
    for shift in (0..64).step_by(7) {
        let byte = *bytes.get(*at).ok_or(RawPayloadError::Malformed)?;
        *at += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte < 0x80 {
            return Ok(value);
        }
    }
    Err(RawPayloadError::Malformed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn round_trip(raw: &[u8], content: RawContent) -> EncodedRaw {
        let encoded = encode(raw, content);
        let lookup = |hash: &[u8; 32]| {
            encoded
                .media
                .iter()
                .find(|m| &m.sha256 == hash)
                .map(|m| Cow::Borrowed(m.bytes.as_slice()))
        };
        let (decoded_content, decoded) = decode(&encoded.record, lookup).unwrap();
        assert_eq!(decoded_content, content);
        assert_eq!(decoded, raw);
        encoded
    }

    fn image(seed: u8, len: usize) -> Vec<u8> {
        (0..len)
            .map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed))
            .collect()
    }

    #[test]
    fn media_is_cut_out_once_and_spliced_back_exactly() {
        let picture = STANDARD.encode(image(7, 3000));
        let raw = format!("\x0a\x12prefix {picture}\x22 middle \x2a{picture}2 tail");
        let encoded = round_trip(raw.as_bytes(), RawContent::Protobuf);
        assert_eq!(
            encoded.media.len(),
            1,
            "the same picture twice is one object"
        );
        assert!(encoded.record.len() < raw.len() / 2);
        assert_eq!(media_hashes(&encoded.record).unwrap().len(), 1);
    }

    #[test]
    fn short_or_non_reproducible_base64_stays_inline() {
        let short = STANDARD.encode(image(1, 100));
        // Unpadded and not a multiple of four after trimming cannot occur; a run with a wrong padding count
        // does not re-encode to itself, so it is left as text.
        let odd = format!("{}=", &STANDARD.encode(image(2, 600))[..799]);
        for raw in [short, odd] {
            let encoded = round_trip(raw.as_bytes(), RawContent::Json);
            assert!(encoded.media.is_empty());
        }
    }

    #[test]
    fn a_run_glued_to_framing_bytes_still_round_trips() {
        // The varint length byte 'A' and the next tag '2' are both alphabet characters.
        let picture = STANDARD.encode(image(3, 999));
        let raw = format!("A{picture}2");
        round_trip(raw.as_bytes(), RawContent::Protobuf);
    }

    #[test]
    fn decoding_refuses_a_missing_or_altered_medium() {
        let raw = STANDARD.encode(image(4, 2000));
        let encoded = encode(raw.as_bytes(), RawContent::Protobuf);
        assert!(matches!(
            decode(&encoded.record, |_| None),
            Err(RawPayloadError::MissingMedia(_))
        ));
        assert!(matches!(
            decode(&encoded.record, |_| Some(Cow::Owned(vec![1, 2, 3]))),
            Err(RawPayloadError::MediaMismatch(_))
        ));
        assert_eq!(decode(b"nope", |_| None), Err(RawPayloadError::NotARecord));
        let mut truncated = encoded.record.clone();
        truncated.truncate(8);
        assert_eq!(
            decode(&truncated, |_| None),
            Err(RawPayloadError::Malformed)
        );
    }

    #[test]
    fn a_wrapped_payload_is_a_record_without_media() {
        let raw = format!("x{}", STANDARD.encode(image(5, 900)));
        let wrapped = wrap(raw.as_bytes(), RawContent::Json);
        assert!(is_record(&wrapped));
        assert!(!is_record(raw.as_bytes()));
        assert!(media_hashes(&wrapped).unwrap().is_empty());
        assert_eq!(
            decode(&wrapped, |_| None).unwrap(),
            (RawContent::Json, raw.into_bytes())
        );
    }

    proptest! {
        /// Any bytes, with or without embedded media, come back exactly.
        #[test]
        fn any_payload_round_trips(
            noise in proptest::collection::vec(any::<u8>(), 0..2048),
            media_len in 0usize..1500,
            at in 0usize..2048,
        ) {
            let mut raw = noise;
            let picture = STANDARD.encode(image(9, media_len));
            let at = at.min(raw.len());
            raw.splice(at..at, picture.into_bytes());
            round_trip(&raw, RawContent::Protobuf);
        }
    }
}
