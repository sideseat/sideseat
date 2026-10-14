use super::*;

/// The column's type, as `schema.rs` declares it (`the_tested_type_is_the_schemas`).
const EXEMPLARS_TYPE: &str = "STRUCT(trace_id VARCHAR, span_id VARCHAR, time_unix_nano UBIGINT, \
     value_int BIGINT, value_double DOUBLE, attributes VARCHAR)[]";

fn round_trip(exemplars: &[MetricExemplar]) -> Vec<MetricExemplar> {
    let conn = duckdb::Connection::open_in_memory().expect("connection");
    conn.execute_batch(&format!(
        "CREATE TABLE t (id INTEGER, exemplars {EXEMPLARS_TYPE})"
    ))
    .expect("table");
    prepare_stage(&conn, "t").expect("stage");
    {
        let mut appender = conn.appender(STAGE).expect("appender");
        appender
            .append_row(duckdb::params![1, staged_exemplars(exemplars).as_deref()])
            .expect("append");
        appender.flush().expect("flush");
    }
    commit_stage(&conn, "t").expect("commit");
    let value: Value = conn
        .query_row("SELECT exemplars FROM t", [], |row| row.get(0))
        .expect("read");
    exemplars_from_value(value)
}

/// Every field an exemplar carries comes back exactly through the stage's JSON and the list it is converted to: ids of
/// any length, integer and double values at their extremes, the instant to the nanosecond, and attribute text with
/// every character a literal could trip on.
#[test]
fn exemplars_come_back_exactly_as_they_were_appended() {
    let exemplars = vec![
        MetricExemplar {
            trace_id: Some((0..16).collect()),
            span_id: Some(vec![0xff; 8]),
            time_unix_nano: 1_790_000_000_123_456_789,
            value_int: Some(i64::MIN),
            value_double: None,
            attributes: Some(
                r#"{"quote":"it's","slash":"a\\b","brace":"}]{[","nl":"x\ny","uni":"é✓","n":200}"#
                    .to_string(),
            ),
        },
        MetricExemplar {
            trace_id: Some(vec![1, 2, 3]),
            span_id: None,
            time_unix_nano: 0,
            value_int: None,
            value_double: Some(0.1 + 0.2),
            attributes: None,
        },
        MetricExemplar {
            trace_id: None,
            span_id: Some(vec![0; 3]),
            time_unix_nano: u64::MAX,
            value_int: None,
            value_double: Some(-1.7976931348623157e308),
            attributes: Some("{}".to_string()),
        },
        MetricExemplar {
            value_double: Some(5e-324),
            ..MetricExemplar::default()
        },
        MetricExemplar {
            value_double: Some(-0.0),
            value_int: None,
            ..MetricExemplar::default()
        },
    ];
    let back = round_trip(&exemplars);
    assert_eq!(back, exemplars);
    // `==` takes -0.0 for 0.0; the bits do not.
    let bits = |all: &[MetricExemplar]| -> Vec<Option<u64>> {
        all.iter()
            .map(|e| e.value_double.map(f64::to_bits))
            .collect()
    };
    assert_eq!(bits(&back), bits(&exemplars));
}

/// A point without exemplars stores none and reads back none.
#[test]
fn a_point_without_exemplars_stores_null() {
    assert_eq!(staged_exemplars(&[]), None);
    assert!(round_trip(&[]).is_empty());
}

/// The type these tests append to is the one the schema gives the column.
#[test]
fn the_tested_type_is_the_schemas() {
    let conn = duckdb::Connection::open_in_memory().expect("connection");
    crate::migrations::ensure_schema(&conn, &crate::TestClock).expect("schema");
    conn.execute_batch(&format!("CREATE TABLE t (e {EXEMPLARS_TYPE})"))
        .expect("table");
    let types: Vec<String> = conn
        .prepare(
            "SELECT data_type FROM duckdb_columns() \
             WHERE (table_name, column_name) IN (('otel_metrics', 'exemplars'), ('t', 'e'))",
        )
        .expect("prepare")
        .query_map([], |row| row.get(0))
        .expect("query")
        .collect::<Result<_, _>>()
        .expect("types");
    assert_eq!(types.len(), 2);
    assert_eq!(types[0], types[1]);
}

/// A seeded stream of characters a literal could misread: its quotes, backslashes, braces, brackets, colons and
/// commas, `NULL` spelled as text, control characters, spaces where trimming could eat them, and unicode up to the
/// astral planes.
struct Awkward(u64);

impl Awkward {
    fn next(&mut self) -> u64 {
        // xorshift64*: deterministic, so a failure names its seed.
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn text(&mut self) -> String {
        const PIECES: &[&str] = &[
            "'",
            "\"",
            "\\",
            "\\\\",
            "{",
            "}",
            "[",
            "]",
            ":",
            ",",
            "NULL",
            "null",
            " ",
            "  ",
            "\t",
            "\n",
            "\r",
            "\u{0}",
            "\u{1}",
            "\u{1f}",
            "\u{7f}",
            "é",
            "✓",
            "𝄞",
            "\u{200b}",
            "a",
            "Z",
            "0",
            "-",
            "=",
            "'NULL'",
            "{'k': 'v'}",
            "[1, 2]",
            "\\'",
            "''",
        ];
        let length = self.next() % 12;
        (0..length)
            .map(|_| match self.next() % 8 {
                // `from_u32` refuses a surrogate, which no `str` can hold.
                0 => char::from_u32((self.next() % 0x10_ffff) as u32)
                    .unwrap_or('x')
                    .to_string(),
                _ => PIECES[(self.next() % PIECES.len() as u64) as usize].to_string(),
            })
            .collect()
    }
}

/// Attribute text of every awkward kind, as values and as keys, round-trips exactly through the stage: a value
/// DuckDB read differently from how it is written would corrupt an exemplar silently.
#[test]
fn awkward_attribute_text_round_trips_exactly() {
    let mut awkward = Awkward(0x5eed_5eed_5eed_5eed);
    for case in 0..1_000 {
        let exemplars: Vec<MetricExemplar> = (0..1 + awkward.next() % 4)
            .map(|_| {
                let mut object = serde_json::Map::new();
                for _ in 0..awkward.next() % 4 {
                    object.insert(awkward.text(), serde_json::Value::String(awkward.text()));
                }
                let attributes = match awkward.next() % 3 {
                    0 => None,
                    1 => Some(serde_json::Value::Object(object).to_string()),
                    _ => Some(awkward.text()),
                };
                MetricExemplar {
                    trace_id: Some(awkward.text().into_bytes()),
                    span_id: None,
                    time_unix_nano: awkward.next(),
                    value_int: None,
                    value_double: Some(f64::from_bits(awkward.next())).filter(|v| v.is_finite()),
                    attributes,
                }
            })
            .collect();
        assert_eq!(round_trip(&exemplars), exemplars, "case {case}");
    }
}
