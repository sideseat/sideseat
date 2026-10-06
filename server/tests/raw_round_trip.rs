//! Every captured export survives the raw record byte for byte.
//!
//! Raw telemetry is the authority every derived row is rebuilt from, so the stored form of an export must give
//! back exactly the bytes the producer sent: every trace, log and metric capture in the corpus, protobuf and
//! OTLP/JSON alike, is encoded into a SideSeat raw record with its media cut out to content-addressed objects,
//! and decoded again from the record and those objects alone.

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use sideseat_domain::raw_payload::{self, RawContent};

fn exports() -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let export = ["req-", "logs-", "metrics-"]
                .iter()
                .any(|prefix| name.starts_with(prefix));
            if export && (name.ends_with(".pb") || name.ends_with(".json")) {
                out.push(path);
            }
        }
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut out = Vec::new();
    walk(&root.join("messages"), &mut out);
    walk(&root.join("metrics"), &mut out);
    out.sort();
    out
}

#[test]
fn every_captured_export_round_trips_byte_exactly() {
    let exports = exports();
    assert!(
        exports.len() > 1000,
        "the corpus is missing: {} exports",
        exports.len()
    );
    let mut original = 0usize;
    let mut records = 0usize;
    // Media is stored once per object, as the store keeps it: one map for the whole corpus is the most
    // sharing possible, and the round trip must not depend on which export first carried an object.
    let mut media: HashMap<[u8; 32], Vec<u8>> = HashMap::new();
    let mut failures = Vec::new();
    for path in &exports {
        let raw = std::fs::read(path).expect("read export");
        let content = if path.extension().is_some_and(|e| e == "json") {
            RawContent::Json
        } else {
            RawContent::Protobuf
        };
        let encoded = raw_payload::encode(&raw, content);
        for object in encoded.media {
            media.entry(object.hash).or_insert(object.bytes);
        }
        original += raw.len();
        records += encoded.record.len();
        match raw_payload::decode(&encoded.record, |hash| {
            media.get(hash).map(|bytes| Cow::Borrowed(bytes.as_slice()))
        }) {
            Ok((decoded_content, decoded)) if decoded_content == content && decoded == raw => {}
            Ok(_) => failures.push(format!("{}: decoded bytes differ", path.display())),
            Err(error) => failures.push(format!("{}: {error}", path.display())),
        }
    }
    let media_bytes: usize = media.values().map(Vec::len).sum();
    println!(
        "{} exports: {original} bytes received -> {records} bytes of records + {} media objects \
         ({media_bytes} decoded bytes)",
        exports.len(),
        media.len(),
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
