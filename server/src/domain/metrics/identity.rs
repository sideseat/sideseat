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

use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue, any_value};

use crate::data::types::NormalizedMetric;

/// The OTLP material an identity needs that `NormalizedMetric` cannot carry losslessly.
///
/// The attribute sets are hashed from the **protobuf**, not from their JSON rendering, and the timestamps
/// from the raw nanosecond counts. Both for the same reason: a rendering can be imitated, and a conversion
/// can lose range.
///
/// JSON has no bytes type and no non-finite numbers, so any encoding of them is expressible as some *other*
/// OTLP value - `["__otlp:bytes", "dead"]` is exactly what an OTLP array of those two strings produces. No
/// choice of tag string fixes that; only hashing the variant does. And `DateTime::timestamp_nanos_opt`
/// answers `None` beyond 2262, where substituting zero made every datapoint of every later year collide.
pub struct IdentityInputs<'a> {
    pub attributes: &'a [KeyValue],
    pub resource_attributes: &'a [KeyValue],
    pub scope_attributes: &'a [KeyValue],
    pub time_unix_nano: u64,
    pub start_time_unix_nano: u64,
}

/// The identity of a datapoint, as a hex digest.
pub fn datapoint_id(metric: &NormalizedMetric, inputs: &IdentityInputs<'_>) -> String {
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
        // The schema URLs pin the *meaning* of the attribute names, so two resources that differ only here
        // describe different streams under identical labels.
        metric.scope_schema_url.as_deref(),
        metric.resource_schema_url.as_deref(),
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

    // The raw nanosecond counts, not the converted `DateTime`. `timestamp_nanos_opt` is `None` beyond 2262
    // and the fallback was zero, so every datapoint of every later year shared an instant.
    hasher.update(&inputs.time_unix_nano.to_le_bytes());
    hasher.update(&inputs.start_time_unix_nano.to_le_bytes());

    // The attribute sets *are* the series - the field the ClickHouse sort key was missing - hashed from the
    // protobuf so that no value can imitate another's encoding.
    write_attributes(&mut hasher, inputs.attributes);
    write_attributes(&mut hasher, inputs.resource_attributes);
    // Scope attributes too: OTel counts the instrumentation scope as part of a stream's identity, and two
    // scopes sharing a name and version can still differ here.
    write_attributes(&mut hasher, inputs.scope_attributes);

    hasher.finalize().to_hex().to_string()
}

/// Hash an OTLP attribute set: keys sorted, each value tagged by its protobuf variant.
///
/// Sorted because an exporter may list the same labels in any order and they describe one series. Tagged by
/// *variant*, which is what makes the encoding unforgeable: a value of one kind can never produce the byte
/// stream of another, however its contents are chosen.
fn write_attributes(hasher: &mut blake3::Hasher, attributes: &[KeyValue]) {
    let mut sorted: Vec<&KeyValue> = attributes.iter().collect();
    sorted.sort_by(|a, b| a.key.cmp(&b.key));
    hasher.update(&(sorted.len() as u64).to_le_bytes());
    for kv in sorted {
        write_optional_str(hasher, Some(&kv.key));
        write_any_value(hasher, kv.value.as_ref());
    }
}

/// Hash one OTLP value, tagged by variant and recursing structurally.
fn write_any_value(hasher: &mut blake3::Hasher, value: Option<&AnyValue>) {
    let Some(inner) = value.and_then(|v| v.value.as_ref()) else {
        // Absent, which is distinct from every present value including an empty string.
        hasher.update(&[0u8]);
        return;
    };
    match inner {
        any_value::Value::StringValue(s) => {
            hasher.update(&[1u8]);
            write_optional_str(hasher, Some(s));
        }
        any_value::Value::BoolValue(b) => {
            hasher.update(&[2u8, u8::from(*b)]);
        }
        any_value::Value::IntValue(i) => {
            hasher.update(&[3u8]);
            hasher.update(&i.to_le_bytes());
        }
        // The bit pattern, so a NaN is a stable, distinct value rather than something unrepresentable.
        any_value::Value::DoubleValue(d) => {
            hasher.update(&[4u8]);
            hasher.update(&d.to_bits().to_le_bytes());
        }
        any_value::Value::BytesValue(bytes) => {
            hasher.update(&[5u8]);
            hasher.update(&(bytes.len() as u64).to_le_bytes());
            hasher.update(bytes);
        }
        any_value::Value::ArrayValue(array) => {
            hasher.update(&[6u8]);
            hasher.update(&(array.values.len() as u64).to_le_bytes());
            for item in &array.values {
                write_any_value(hasher, Some(item));
            }
        }
        any_value::Value::KvlistValue(list) => {
            hasher.update(&[7u8]);
            write_attributes(hasher, &list.values);
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::types::MetricType;

    fn kv(key: &str, value: any_value::Value) -> KeyValue {
        KeyValue {
            key: key.to_string(),
            value: Some(AnyValue { value: Some(value) }),
        }
    }

    fn text(key: &str, value: &str) -> KeyValue {
        kv(key, any_value::Value::StringValue(value.to_string()))
    }

    fn base() -> NormalizedMetric {
        NormalizedMetric {
            project_id: Some("default".to_string()),
            metric_name: "http.server.requests".to_string(),
            metric_type: MetricType::Sum,
            ..Default::default()
        }
    }

    /// The identity of a datapoint with the given attributes, everything else held fixed.
    fn id_of(metric: &NormalizedMetric, attributes: &[KeyValue]) -> String {
        datapoint_id(
            metric,
            &IdentityInputs {
                attributes,
                resource_attributes: &[],
                scope_attributes: &[],
                time_unix_nano: 1_700_000_000_000_000_000,
                start_time_unix_nano: 0,
            },
        )
    }

    /// Two labelled series recorded at the same instant are two datapoints.
    ///
    /// This is the defect the id exists for: the ClickHouse sort key held no attributes, so a
    /// `ReplacingMergeTree` merge kept one of these and deleted the other.
    #[test]
    fn two_label_sets_at_one_instant_are_two_datapoints() {
        let m = base();
        let ok = id_of(&m, &[kv("code", any_value::Value::IntValue(200))]);
        let failed = id_of(&m, &[kv("code", any_value::Value::IntValue(500))]);
        assert_ne!(ok, failed);
    }

    /// The same series and instant is one datapoint, whatever order the labels arrived in.
    #[test]
    fn label_order_does_not_make_a_second_datapoint() {
        let m = base();
        let one = id_of(&m, &[text("method", "GET"), text("route", "/v1/traces")]);
        let other = id_of(&m, &[text("route", "/v1/traces"), text("method", "GET")]);
        assert_eq!(one, other);
    }

    /// No OTLP value can imitate another's encoding.
    ///
    /// The reason the attribute sets are hashed from the protobuf and not from a JSON rendering. JSON has no
    /// bytes type and no non-finite numbers, so *any* encoding of them is expressible as some other OTLP
    /// value - a string prefix collides with that string, and a tagged array collides with an array of those
    /// strings. Tagging the protobuf *variant* is the only form that cannot be forged.
    #[test]
    fn no_value_can_imitate_another_kind() {
        let m = base();
        let raw = id_of(
            &m,
            &[kv("id", any_value::Value::BytesValue(vec![0xde, 0xad]))],
        );

        // A string that renders the same hex.
        assert_ne!(raw, id_of(&m, &[text("id", "dead")]));
        // A string carrying a tag someone might have chosen.
        assert_ne!(raw, id_of(&m, &[text("id", "__otlp:bytes")]));
        // And an *array* of exactly the strings a tagged-array encoding would produce.
        let array = kv(
            "id",
            any_value::Value::ArrayValue(opentelemetry_proto::tonic::common::v1::ArrayValue {
                values: vec![
                    AnyValue {
                        value: Some(any_value::Value::StringValue("__otlp:bytes".to_string())),
                    },
                    AnyValue {
                        value: Some(any_value::Value::StringValue("dead".to_string())),
                    },
                ],
            }),
        );
        assert_ne!(
            raw,
            id_of(&m, &[array]),
            "an OTLP array of the tag strings must not equal the bytes it was meant to encode"
        );

        // Numbers, booleans and their string spellings.
        assert_ne!(
            id_of(&m, &[kv("v", any_value::Value::IntValue(200))]),
            id_of(&m, &[text("v", "200")])
        );
        assert_ne!(
            id_of(&m, &[kv("v", any_value::Value::BoolValue(true))]),
            id_of(&m, &[text("v", "true")])
        );

        // Non-finite doubles keep their own identity, distinct from each other and from strings.
        let nan = id_of(&m, &[kv("v", any_value::Value::DoubleValue(f64::NAN))]);
        let inf = id_of(&m, &[kv("v", any_value::Value::DoubleValue(f64::INFINITY))]);
        assert_ne!(nan, inf);
        assert_ne!(nan, id_of(&m, &[text("v", "NaN")]));
        // And a NaN is stable: the bit pattern is hashed, not a rendering.
        assert_eq!(
            nan,
            id_of(&m, &[kv("v", any_value::Value::DoubleValue(f64::NAN))])
        );

        // An absent value is not an empty string.
        let absent = KeyValue {
            key: "v".to_string(),
            value: None,
        };
        assert_ne!(id_of(&m, &[absent]), id_of(&m, &[text("v", "")]));
    }

    /// Timestamps beyond 2262 are distinct, where a `DateTime` conversion collapses them.
    ///
    /// `timestamp_nanos_opt` answers `None` past that year and the fallback was zero, so every datapoint of
    /// every later year shared an instant. The raw nanosecond count has no such range.
    #[test]
    fn far_future_timestamps_do_not_collide() {
        let m = base();
        let at = |nanos: u64| {
            datapoint_id(
                &m,
                &IdentityInputs {
                    attributes: &[],
                    resource_attributes: &[],
                    scope_attributes: &[],
                    time_unix_nano: nanos,
                    start_time_unix_nano: 0,
                },
            )
        };
        // Both beyond what an i64 of nanoseconds can express as a DateTime.
        let year_2270 = 9_500_000_000_000_000_000u64;
        let year_2271 = 9_530_000_000_000_000_000u64;
        assert_ne!(at(year_2270), at(year_2271));
        assert_ne!(at(year_2270), at(0));
    }

    /// A corrected value is the same datapoint, so the later row replaces the earlier one.
    #[test]
    fn the_measurement_is_not_part_of_the_identity() {
        let attrs = [text("route", "/v1/traces")];
        let first = NormalizedMetric {
            value_int: Some(7),
            ..base()
        };
        let corrected = NormalizedMetric {
            value_int: Some(9),
            ..base()
        };
        assert_eq!(id_of(&first, &attrs), id_of(&corrected, &attrs));
    }

    /// Everything that names the series changes it.
    #[test]
    fn the_series_and_the_instant_are_part_of_the_identity() {
        let attrs = [text("route", "/v1/traces")];
        let m = base();
        let id = id_of(&m, &attrs);

        for (label, other) in [
            (
                "metric name",
                NormalizedMetric {
                    metric_name: "http.server.duration".to_string(),
                    ..base()
                },
            ),
            (
                "project",
                NormalizedMetric {
                    project_id: Some("other".to_string()),
                    ..base()
                },
            ),
            (
                "scope",
                NormalizedMetric {
                    scope_name: Some("some.instrumentation".to_string()),
                    ..base()
                },
            ),
            (
                "unit",
                NormalizedMetric {
                    metric_unit: Some("ms".to_string()),
                    ..base()
                },
            ),
            (
                "monotonicity",
                NormalizedMetric {
                    is_monotonic: Some(true),
                    ..base()
                },
            ),
            (
                "scope schema url",
                NormalizedMetric {
                    scope_schema_url: Some("https://opentelemetry.io/schemas/1.30.0".to_string()),
                    ..base()
                },
            ),
            (
                "resource schema url",
                NormalizedMetric {
                    resource_schema_url: Some(
                        "https://opentelemetry.io/schemas/1.30.0".to_string(),
                    ),
                    ..base()
                },
            ),
        ] {
            assert_ne!(
                id,
                id_of(&other, &attrs),
                "{label} must change the identity"
            );
        }

        // The instant, and the delta window.
        let later = datapoint_id(
            &m,
            &IdentityInputs {
                attributes: &attrs,
                resource_attributes: &[],
                scope_attributes: &[],
                time_unix_nano: 1_700_000_000_000_000_001,
                start_time_unix_nano: 0,
            },
        );
        assert_ne!(id, later, "the instant must change the identity");
        let windowed = datapoint_id(
            &m,
            &IdentityInputs {
                attributes: &attrs,
                resource_attributes: &[],
                scope_attributes: &[],
                time_unix_nano: 1_700_000_000_000_000_000,
                start_time_unix_nano: 1_699_000_000_000_000_000,
            },
        );
        assert_ne!(id, windowed, "a delta window must change the identity");
    }

    /// The resource and the scope are part of the series, not only the datapoint's own labels.
    #[test]
    fn the_resource_and_scope_attributes_are_part_of_the_identity() {
        let m = base();
        let plain = datapoint_id(
            &m,
            &IdentityInputs {
                attributes: &[],
                resource_attributes: &[],
                scope_attributes: &[],
                time_unix_nano: 1,
                start_time_unix_nano: 0,
            },
        );
        let with_resource = datapoint_id(
            &m,
            &IdentityInputs {
                attributes: &[],
                resource_attributes: &[text("service.name", "api")],
                scope_attributes: &[],
                time_unix_nano: 1,
                start_time_unix_nano: 0,
            },
        );
        let with_scope = datapoint_id(
            &m,
            &IdentityInputs {
                attributes: &[],
                resource_attributes: &[],
                scope_attributes: &[text("library.tier", "beta")],
                time_unix_nano: 1,
                start_time_unix_nano: 0,
            },
        );
        assert_ne!(plain, with_resource);
        assert_ne!(plain, with_scope);
        assert_ne!(
            with_resource, with_scope,
            "the resource and the scope are different fields, not one"
        );
    }

    /// A nested label value is part of the series, not flattened away.
    #[test]
    fn nested_attribute_values_are_part_of_the_identity() {
        let m = base();
        let nested = |port: i64| {
            kv(
                "peer",
                any_value::Value::KvlistValue(
                    opentelemetry_proto::tonic::common::v1::KeyValueList {
                        values: vec![
                            text("host", "a"),
                            kv("port", any_value::Value::IntValue(port)),
                        ],
                    },
                ),
            )
        };
        assert_ne!(id_of(&m, &[nested(1)]), id_of(&m, &[nested(2)]));
    }
}
