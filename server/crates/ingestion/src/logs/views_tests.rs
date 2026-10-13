//! A log's body views are answered alike whether stored or rebuilt from its raw record.
//!
//! Ingest stores the body and its text only where the raw record, read back as stored, does not rebuild them
//! exactly (`ingest::settle_views`); a read answers the rest from the raw record (`domain::log_views`). The
//! property every reader rests on is that the answer is the derived views whichever way it is reached - checked
//! here over every captured log export and over generated values a capture does not hold.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::common::v1::{
    AnyValue, ArrayValue, KeyValue, KeyValueList, any_value,
};
use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};
use prost::Message;
use sideseat_domain::log_views::{LogViews, resolve_views};
use sideseat_ports::types::NormalizedLog;

fn received() -> DateTime<Utc> {
    DateTime::from_timestamp(1_800_000_000, 0).expect("an instant")
}

/// The views ingest derived, as their columns would hold them.
fn derived(log: &NormalizedLog) -> LogViews {
    LogViews {
        body: sideseat_core::utils::json::json_to_opt_string(&log.body),
        body_text: log.body_text.clone(),
        attributes: sideseat_core::utils::json::json_to_opt_string(&log.attributes),
    }
}

/// What a read answers for the stored row: the stored columns, the raw record as stored, the noted references.
fn answered(log: &NormalizedLog) -> LogViews {
    let (body, body_text, attributes) = log.stored_views();
    resolve_views(
        LogViews {
            body,
            body_text,
            attributes,
        },
        sideseat_core::utils::json::json_to_opt_string(&log.raw_log).as_deref(),
        &log.noted_references,
    )
}

fn settled(request: &ExportLogsServiceRequest) -> Vec<NormalizedLog> {
    let (mut logs, _) = super::extract::extract_logs_batch_with_media(request, received());
    for log in &mut logs {
        super::ingest::settle_views(log);
    }
    logs
}

fn log_exports() -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name.starts_with("logs-") && (name.ends_with(".pb") || name.ends_with(".json"))
                })
            {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    walk(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/messages"),
        &mut out,
    );
    out.sort();
    out
}

/// Every captured record's raw form rebuilds its views - so none is stored - and the read answers them exactly.
#[test]
fn every_captured_record_is_answered_from_its_raw_record() {
    let exports = log_exports();
    assert!(
        exports.len() > 200,
        "the corpus is missing: {} exports",
        exports.len()
    );
    let (mut records, mut stored) = (0usize, Vec::new());
    for path in &exports {
        let bytes = std::fs::read(path).expect("read export");
        let request: ExportLogsServiceRequest = if path.extension().is_some_and(|e| e == "json") {
            serde_json::from_slice(&bytes).expect("an OTLP/JSON export")
        } else {
            ExportLogsServiceRequest::decode(bytes.as_slice()).expect("an OTLP export")
        };
        for log in settled(&request) {
            records += 1;
            assert_eq!(answered(&log), derived(&log), "{}", path.display());
            if !log.views_rebuild_from_raw {
                stored.push(path.display().to_string());
            }
        }
    }
    assert!(records > 2_500, "only {records} records");
    assert!(
        stored.is_empty(),
        "records whose views are stored: {stored:?}"
    );
}

/// A small deterministic generator, so a failure reproduces.
struct Values(u64);

impl Values {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn pick<'a, T>(&mut self, from: &'a [T]) -> &'a T {
        &from[(self.next() % from.len() as u64) as usize]
    }

    fn text(&mut self) -> String {
        let hash = "a".repeat(64);
        self.pick(&[
            String::new(),
            "plain".to_string(),
            "ünïcødé ✓".to_string(),
            "quote \" and \\ backslash\n".to_string(),
            "#!".to_string(),
            "#!!".to_string(),
            format!("#!B64!#image/png::{hash}"),
            format!("see #!!B64!#image/png::{hash} here"),
            "{\"json\": [1, 2]}".to_string(),
        ])
        .clone()
    }

    fn value(&mut self, depth: u32) -> AnyValue {
        let kind = self.next() % if depth > 3 { 6 } else { 9 };
        let value = match kind {
            0 => None,
            1 => Some(any_value::Value::StringValue(self.text())),
            2 => Some(any_value::Value::BoolValue(self.next().is_multiple_of(2))),
            3 => Some(any_value::Value::IntValue(*self.pick(&[
                0,
                -1,
                42,
                i64::MAX,
                i64::MIN,
                9_007_199_254_740_993,
            ]))),
            4 => Some(any_value::Value::DoubleValue(*self.pick(&[
                0.0,
                -0.0,
                0.1,
                1.0,
                1e300,
                -2.5e-308,
                123_456.789,
                f64::NAN,
                f64::INFINITY,
            ]))),
            5 => Some(any_value::Value::BytesValue(
                (0..self.next() % 5).map(|n| n as u8 * 37).collect(),
            )),
            6 | 7 => Some(any_value::Value::KvlistValue(KeyValueList {
                values: (0..self.next() % 4)
                    .map(|_| KeyValue {
                        key: self.pick(&["k", "k", "#!x", "", "z"]).to_string(),
                        value: (!self.next().is_multiple_of(5)).then(|| self.value(depth + 1)),
                    })
                    .collect(),
            })),
            _ => Some(any_value::Value::ArrayValue(ArrayValue {
                values: (0..self.next() % 4)
                    .map(|_| self.value(depth + 1))
                    .collect(),
            })),
        };
        AnyValue { value }
    }
}

fn export_of(body: Option<AnyValue>) -> ExportLogsServiceRequest {
    ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            scope_logs: vec![ScopeLogs {
                log_records: vec![LogRecord {
                    time_unix_nano: 1_700_000_000_000_000_000,
                    trace_id: vec![7; 16],
                    body,
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        }],
    }
}

/// Whatever the body holds - nesting, repeated keys, absent values, escapes, reference-shaped text, numbers JSON
/// cannot hold or its text does not give back exactly - the read answers the views ingest derived, rebuilt or
/// stored; and every body without a non-finite double is rebuilt, so storing the views stays the exception.
#[test]
fn every_generated_body_is_answered_as_derived() {
    let mut values = Values(0x5eed_5eed_5eed_5eed);
    let (mut without_doubles, mut stored) = (0usize, 0usize);
    for _ in 0..5_000 {
        let body = values.value(0);
        let debug = format!("{body:?}");
        let has_double = debug.contains("NaN") || debug.contains("inf");
        for log in settled(&export_of(Some(body))) {
            assert_eq!(answered(&log), derived(&log), "{:?}", log.raw_log);
            if !has_double {
                without_doubles += 1;
                assert!(log.views_rebuild_from_raw, "{:?}", log.raw_log);
            }
            stored += usize::from(!log.views_rebuild_from_raw);
        }
    }
    assert!(
        without_doubles > 2_000,
        "{without_doubles} bodies without a double"
    );
    assert!(stored > 0, "no generated body exercised the stored views");
}

/// The attributes too: whatever they hold - absent values, repeated keys, nesting, escapes, bytes and doubles the
/// typed rendering tags - the read answers them as derived; and every list without bytes or a non-finite double is
/// rebuilt.
#[test]
fn every_generated_attribute_list_is_answered_as_derived() {
    let mut values = Values(0xa77a_a77a_a77a_a77a);
    let (mut plain, mut stored) = (0usize, 0usize);
    for _ in 0..5_000 {
        let attributes: Vec<KeyValue> = (0..values.next() % 5)
            .map(|_| KeyValue {
                key: values.pick(&["k", "k", "#!x", "", "z"]).to_string(),
                value: (!values.next().is_multiple_of(5)).then(|| values.value(1)),
            })
            .collect();
        let tagged = format!("{attributes:?}");
        let tagged =
            tagged.contains("NaN") || tagged.contains("inf") || tagged.contains("BytesValue");
        let mut export = export_of(None);
        export.resource_logs[0].scope_logs[0].log_records[0].attributes = attributes;
        for log in settled(&export) {
            assert_eq!(answered(&log), derived(&log), "{:?}", log.raw_log);
            if !tagged {
                plain += 1;
                assert!(log.views_rebuild_from_raw, "{:?}", log.raw_log);
            }
            stored += usize::from(!log.views_rebuild_from_raw);
        }
    }
    assert!(plain > 1_000, "{plain} plain attribute lists");
    assert!(
        stored > 0,
        "no generated attributes exercised the stored views"
    );
}

/// A record with no body, and one whose body holds no value, store nothing and are answered with no body views and
/// its empty attributes; a record whose views are all absent but which is not rebuilt stores the empty text that
/// says so.
#[test]
fn an_absent_body_is_answered_as_absent() {
    for body in [None, Some(AnyValue { value: None })] {
        let log = settled(&export_of(body)).remove(0);
        assert!(log.views_rebuild_from_raw);
        assert_eq!(log.stored_views(), (None, None, None));
        assert_eq!(
            answered(&log),
            LogViews {
                body: None,
                body_text: None,
                attributes: Some("{}".to_string()),
            }
        );
    }
    let mut log = settled(&export_of(None)).remove(0);
    log.views_rebuild_from_raw = false;
    log.attributes = serde_json::Value::Null;
    log.raw_log = serde_json::json!({"body": {"stringValue": "not this record's"}});
    assert_eq!(log.stored_views(), (None, Some(String::new()), None));
    assert_eq!(answered(&log), LogViews::default());
}

/// A double OTLP/JSON cannot hold - NaN, an infinity - keeps the record's views stored, and they are answered as
/// derived; every finite double is read back from its digits exactly, so it is rebuilt, the ones a rounding parse
/// reads a bit away included.
#[test]
fn only_a_double_the_raw_record_cannot_hold_keeps_its_views() {
    let double = |value: f64| {
        settled(&export_of(Some(AnyValue {
            value: Some(any_value::Value::DoubleValue(value)),
        })))
        .remove(0)
    };
    let log = double(f64::NAN);
    assert!(!log.views_rebuild_from_raw);
    assert_eq!(
        log.stored_views(),
        (None, Some("NaN".to_string()), Some("{}".to_string()))
    );
    assert_eq!(answered(&log).body_text.as_deref(), Some("NaN"));
    for finite in [
        0.1,
        -2.5e-308,
        0.014_521_500_000_000_001,
        0.002_615_499_999_999_999_8,
    ] {
        let log = double(finite);
        assert!(log.views_rebuild_from_raw, "{finite}");
        assert_eq!(answered(&log), derived(&log));
    }
}

/// A noted reference is rebuilt as its note: the raw record keeps the producer's text, the noted references are
/// stored, and the read shows the note in the body and its text alike.
#[test]
fn a_noted_reference_is_rebuilt_as_its_note() {
    let uri = format!("#!B64!#image/png::{}", "b".repeat(64));
    let body = AnyValue {
        value: Some(any_value::Value::KvlistValue(KeyValueList {
            values: vec![KeyValue {
                key: "image".to_string(),
                value: Some(AnyValue {
                    value: Some(any_value::Value::StringValue(format!("see {uri}"))),
                }),
            }],
        })),
    };
    let (mut logs, _) =
        super::extract::extract_logs_batch_with_media(&export_of(Some(body)), received());
    let mut log = logs.remove(0);
    super::ingest::note_unbacked(&mut log, std::slice::from_ref(&uri));
    super::ingest::settle_views(&mut log);
    assert!(log.views_rebuild_from_raw, "{:?}", log.raw_log);
    assert_eq!(log.stored_views(), (None, None, None));
    assert_eq!(log.noted_references, vec![uri.clone()]);
    let views = answered(&log);
    assert_eq!(views, derived(&log));
    for view in [
        views.body.unwrap_or_default(),
        views.body_text.unwrap_or_default(),
    ] {
        assert!(view.contains("[content not stored (image/png)]"), "{view}");
        assert!(!view.contains(&uri), "{view}");
    }
}
