//! What makes two metric datapoints the same datapoint.
//!
//! # Why this exists
//!
//! Both analytics backends stored metrics with no identity at all. On ClickHouse the consequence was
//! silent deletion: `otel_metrics` is a `ReplacingMergeTree` sorted by
//! `(project_id, metric_name, toDate(timestamp), timestamp)`, and a replacing engine treats rows with
//! equal sort keys as *versions of one row*. Labels are not in that key, so
//! `http.requests{status=200}` and `http.requests{status=500}` recorded at the same instant - the normal
//! output of a single metric export, not an edge case - collapsed into one row at the next merge. The
//! ingest returned 200 and half the series was gone. On DuckDB the table is append-only with no key, so
//! the same re-delivered payload accumulated duplicate rows instead.
//!
//! Both are the same missing definition, so both get the same one, computed here once and written to a
//! `datapoint_id` column that ClickHouse sorts by and DuckDB deduplicates on at query time - exactly the
//! division of labour that spans already use.
//!
//! # What identifies a datapoint, and what does not
//!
//! A datapoint is a measurement *of a series* *at an instant*. OTel says the series is the resource, the
//! scope, the metric name and the attribute set; the instant is `time_unix_nano`, with
//! `start_time_unix_nano` distinguishing the windows of a delta stream. All of that is identity.
//!
//! The measurement itself - the value, the bucket counts, the exemplar - is deliberately **not**, so that
//! a re-delivery carrying a corrected value replaces the earlier row rather than sitting beside it. That
//! is the same rule spans follow, where a re-delivered span id overwrites.
//!
//! Descriptions and units are also excluded: they describe the metric, not the datapoint, and a producer
//! that adds a unit between two exports has not created a second measurement.
//!
//! # Why not `std::hash`
//!
//! This id is persisted and compared across processes, so it must not depend on anything about the
//! machine that produced it. `Hash for usize` writes native-endian bytes, so two replicas on different
//! architectures would disagree about whether a datapoint is the same one - and disagreeing here means
//! either a duplicate or a deletion. Everything below is written little-endian and length-prefixed by
//! hand, and BLAKE3 rather than a 64-bit hash because a collision does not merely group two datapoints,
//! it discards one.

use serde_json::Value as JsonValue;

use crate::data::types::NormalizedMetric;

/// The identity of a datapoint, as a hex digest.
pub fn datapoint_id(metric: &NormalizedMetric) -> String {
    let mut hasher = blake3::Hasher::new();

    // Field order is fixed and every field is length-prefixed, so no two different field lists can
    // produce the same byte stream.
    for field in [
        metric.project_id.as_deref(),
        Some(metric.metric_name.as_str()),
        Some(metric.metric_type.as_str()),
        Some(metric.aggregation_temporality.as_str()),
        // The unit is part of the stream's semantics, not decoration: the same name reported in `ms` and
        // in `s` is two streams, and treating them as one datapoint would keep whichever merged last.
        metric.metric_unit.as_deref(),
        metric.scope_name.as_deref(),
        metric.scope_version.as_deref(),
    ] {
        write_optional_str(&mut hasher, field);
    }

    // Monotonicity too, for the same reason: a monotonic sum and a non-monotonic one are different
    // streams even under one name, and OTel treats the flag as part of the instrument's definition.
    match metric.is_monotonic {
        Some(monotonic) => {
            hasher.update(&[1u8, u8::from(monotonic)]);
        }
        None => {
            hasher.update(&[0u8, 0u8]);
        }
    }

    hasher.update(
        &metric
            .timestamp
            .timestamp_nanos_opt()
            .unwrap_or(0)
            .to_le_bytes(),
    );
    match metric.start_timestamp {
        Some(t) => {
            hasher.update(&[1u8]);
            hasher.update(&t.timestamp_nanos_opt().unwrap_or(0).to_le_bytes());
        }
        None => {
            hasher.update(&[0u8]);
        }
    }

    // The attribute set *is* the series, so this is the field that the ClickHouse sort key was missing.
    write_canonical_json(&mut hasher, &metric.attributes);
    write_canonical_json(&mut hasher, &metric.resource_attributes);

    hasher.finalize().to_hex().to_string()
}

/// Present-or-absent is written as well as the value, so an absent field and an empty one differ.
fn write_optional_str(hasher: &mut blake3::Hasher, value: Option<&str>) {
    match value {
        Some(s) => {
            hasher.update(&[1u8]);
            hasher.update(&(s.len() as u64).to_le_bytes());
            hasher.update(s.as_bytes());
        }
        None => {
            hasher.update(&[0u8]);
        }
    }
}

/// Write a JSON value canonically: object keys sorted, every kind tagged.
///
/// Sorted because `serde_json` is built with `preserve_order` here, so an object's key order is the
/// order it was parsed in - and two exporters listing the same two labels in different orders are
/// describing the same series. Tagged by kind so the string `"1"` cannot hash as the number `1`.
fn write_canonical_json(hasher: &mut blake3::Hasher, value: &JsonValue) {
    match value {
        JsonValue::Object(map) => {
            hasher.update(&[0u8]);
            let mut keys: Vec<&str> = map.keys().map(String::as_str).collect();
            keys.sort_unstable();
            hasher.update(&(keys.len() as u64).to_le_bytes());
            for key in keys {
                write_optional_str(hasher, Some(key));
                if let Some(member) = map.get(key) {
                    write_canonical_json(hasher, member);
                }
            }
        }
        JsonValue::Array(items) => {
            hasher.update(&[1u8]);
            hasher.update(&(items.len() as u64).to_le_bytes());
            for item in items {
                write_canonical_json(hasher, item);
            }
        }
        JsonValue::String(s) => {
            hasher.update(&[2u8]);
            write_optional_str(hasher, Some(s));
        }
        JsonValue::Number(n) => {
            hasher.update(&[3u8]);
            write_optional_str(hasher, Some(&n.to_string()));
        }
        JsonValue::Bool(b) => {
            hasher.update(&[4u8]);
            hasher.update(&[u8::from(*b)]);
        }
        JsonValue::Null => {
            hasher.update(&[5u8]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use serde_json::json;

    fn datapoint(attributes: JsonValue) -> NormalizedMetric {
        NormalizedMetric {
            project_id: Some("default".to_string()),
            metric_name: "http.server.requests".to_string(),
            timestamp: Utc.with_ymd_and_hms(2026, 3, 1, 12, 0, 0).single().unwrap(),
            attributes,
            ..Default::default()
        }
    }

    /// Two labelled series recorded at the same instant are two datapoints.
    ///
    /// This is the defect the id exists for: the ClickHouse sort key held no attributes, so a
    /// `ReplacingMergeTree` merge kept one of these and deleted the other.
    #[test]
    fn two_label_sets_at_one_instant_are_two_datapoints() {
        let ok = datapoint(json!({"http.response.status_code": 200}));
        let failed = datapoint(json!({"http.response.status_code": 500}));
        assert_ne!(datapoint_id(&ok), datapoint_id(&failed));
    }

    /// The same series and instant is one datapoint, whatever order the labels arrived in.
    ///
    /// `preserve_order` means the parsed object keeps its input order, so without canonicalisation a
    /// re-delivery that happened to list the labels differently would be stored twice.
    #[test]
    fn label_order_does_not_make_a_second_datapoint() {
        let one = datapoint(json!({"method": "GET", "route": "/v1/traces"}));
        let other = datapoint(json!({"route": "/v1/traces", "method": "GET"}));
        assert_eq!(datapoint_id(&one), datapoint_id(&other));
    }

    /// A corrected value is the same datapoint, so the later row replaces the earlier one.
    #[test]
    fn the_measurement_is_not_part_of_the_identity() {
        let first = NormalizedMetric {
            value_int: Some(7),
            ..datapoint(json!({"route": "/v1/traces"}))
        };
        let corrected = NormalizedMetric {
            value_int: Some(9),
            ..datapoint(json!({"route": "/v1/traces"}))
        };
        assert_eq!(datapoint_id(&first), datapoint_id(&corrected));
    }

    /// Everything that names the series or the instant does change it.
    #[test]
    fn the_series_and_the_instant_are_part_of_the_identity() {
        let base = datapoint(json!({"route": "/v1/traces"}));
        let id = datapoint_id(&base);

        let renamed = NormalizedMetric {
            metric_name: "http.server.duration".to_string(),
            ..base.clone()
        };
        let later = NormalizedMetric {
            timestamp: base.timestamp + chrono::Duration::seconds(1),
            ..base.clone()
        };
        let other_project = NormalizedMetric {
            project_id: Some("other".to_string()),
            ..base.clone()
        };
        let other_scope = NormalizedMetric {
            scope_name: Some("some.instrumentation".to_string()),
            ..base.clone()
        };
        let other_resource = NormalizedMetric {
            resource_attributes: json!({"service.name": "api"}),
            ..base.clone()
        };
        let other_window = NormalizedMetric {
            start_timestamp: Some(base.timestamp - chrono::Duration::seconds(60)),
            ..base.clone()
        };
        for (label, other) in [
            ("metric name", renamed),
            ("timestamp", later),
            ("project", other_project),
            ("scope", other_scope),
            ("resource", other_resource),
            ("delta window", other_window),
        ] {
            assert_ne!(id, datapoint_id(&other), "{label} must change the identity");
        }
    }

    /// The unit and the monotonicity flag are part of the stream, not commentary on it.
    ///
    /// The same metric name reported in `ms` and in `s` measures two different things, and a monotonic
    /// sum is a different instrument from a non-monotonic one. Excluded, they shared an identity and the
    /// replacing engine kept whichever merged last.
    #[test]
    fn the_unit_and_monotonicity_are_part_of_the_identity() {
        let base = datapoint(json!({"route": "/v1/traces"}));
        let id = datapoint_id(&base);

        let millis = NormalizedMetric {
            metric_unit: Some("ms".to_string()),
            ..base.clone()
        };
        let seconds = NormalizedMetric {
            metric_unit: Some("s".to_string()),
            ..base.clone()
        };
        assert_ne!(
            id,
            datapoint_id(&millis),
            "adding a unit changes the stream"
        );
        assert_ne!(
            datapoint_id(&millis),
            datapoint_id(&seconds),
            "ms and s are different streams under one name"
        );

        let monotonic = NormalizedMetric {
            is_monotonic: Some(true),
            ..base.clone()
        };
        let not_monotonic = NormalizedMetric {
            is_monotonic: Some(false),
            ..base.clone()
        };
        assert_ne!(
            id,
            datapoint_id(&monotonic),
            "declaring monotonicity matters"
        );
        assert_ne!(
            datapoint_id(&monotonic),
            datapoint_id(&not_monotonic),
            "a monotonic sum is not the same instrument as a non-monotonic one"
        );
    }

    /// A nested label value is part of the series, not flattened away.
    #[test]
    fn nested_attribute_values_are_part_of_the_identity() {
        let one = datapoint(json!({"peer": {"host": "a", "port": 1}}));
        let other = datapoint(json!({"peer": {"host": "a", "port": 2}}));
        assert_ne!(datapoint_id(&one), datapoint_id(&other));
    }

    /// A string label and a numeric one that render alike are different.
    #[test]
    fn a_kind_change_is_a_different_series() {
        let text = datapoint(json!({"code": "200"}));
        let number = datapoint(json!({"code": 200}));
        assert_ne!(datapoint_id(&text), datapoint_id(&number));
    }
}
