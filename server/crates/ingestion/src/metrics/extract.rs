//! Metric extraction from OTLP protobuf.
//!
//! Extracts and flattens metrics into one NormalizedMetric per data point.
//! Supports all 5 OTLP metric types: Gauge, Sum, Histogram, ExponentialHistogram, Summary.

use std::collections::HashMap;

use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
use opentelemetry_proto::tonic::common::v1::KeyValue;
use opentelemetry_proto::tonic::metrics::v1::{
    Metric, exponential_histogram_data_point::Buckets, metric::Data, number_data_point,
};
use serde_json::{Value as JsonValue, json};

use crate::otlp::{
    PROJECT_ID_ATTR, attrs_to_typed_json, extract_attributes, get_environment, get_session_id,
    get_user_id, keys,
};
use sideseat_core::utils::time::{is_storable, nanos_to_datetime};
use sideseat_ports::types::{AggregationTemporality, MetricType, NormalizedMetric};

use super::identity::IdentityInputs;

/// Pair a datapoint with the OTLP material its identity needs.
#[allow(clippy::too_many_arguments)]
fn push_with_identity<'a>(
    result: &mut Vec<(NormalizedMetric, IdentityInputs<'a>)>,
    ctx: &ResourceContext<'a>,
    scope: &ScopeContext<'a>,
    attributes: &'a [KeyValue],
    time_unix_nano: u64,
    start_time_unix_nano: u64,
    metric: NormalizedMetric,
) {
    result.push((
        metric,
        IdentityInputs {
            attributes,
            resource_attributes: ctx.proto_attributes,
            scope_attributes: scope.proto_attributes,
            time_unix_nano,
            start_time_unix_nano,
        },
    ));
}

/// Extract and flatten all metrics from an OTLP request.
/// Returns one NormalizedMetric per data point.
pub fn extract_metrics_batch(request: &ExportMetricsServiceRequest) -> Vec<NormalizedMetric> {
    // Each datapoint with the OTLP material its identity needs. Collected rather than stamped inline so
    // that there is exactly *one* place that decides what makes two datapoints the same - it cannot end up
    // differing between a gauge and a histogram.
    let mut result: Vec<(NormalizedMetric, IdentityInputs<'_>)> = Vec::new();

    for resource_metrics in &request.resource_metrics {
        let resource = resource_metrics.resource.as_ref();
        let resource_attrs = resource
            .map(|r| extract_attributes(&r.attributes))
            .unwrap_or_default();
        // Typed too, for the datapoint identity - see attrs_to_typed_json.
        let typed_resource_attrs = resource
            .map(|r| attrs_to_typed_json(&r.attributes))
            .unwrap_or(JsonValue::Object(serde_json::Map::new()));

        const NO_ATTRS: &[KeyValue] = &[];
        let ctx = ResourceContext::from_attrs(
            &resource_attrs,
            typed_resource_attrs,
            (!resource_metrics.schema_url.is_empty()).then(|| resource_metrics.schema_url.clone()),
            resource
                .map(|r| r.attributes.as_slice())
                .unwrap_or(NO_ATTRS),
        );

        for scope_metrics in &resource_metrics.scope_metrics {
            let scope = scope_metrics.scope.as_ref();
            let scope_ctx = ScopeContext {
                proto_attributes: scope.map(|s| s.attributes.as_slice()).unwrap_or(NO_ATTRS),
                name: scope.map(|s| s.name.clone()).filter(|s| !s.is_empty()),
                version: scope.and_then(|s| (!s.version.is_empty()).then(|| s.version.clone())),
                attributes: scope
                    .map(|s| attrs_to_typed_json(&s.attributes))
                    .unwrap_or(JsonValue::Object(serde_json::Map::new())),
                schema_url: (!scope_metrics.schema_url.is_empty())
                    .then(|| scope_metrics.schema_url.clone()),
            };

            for metric in &scope_metrics.metrics {
                extract_metric_data_points(&mut result, metric, &ctx, &scope_ctx);
            }
        }
    }

    result
        .into_iter()
        .map(|(mut metric, inputs)| {
            metric.datapoint_id = super::identity::datapoint_id(&metric, &inputs);
            metric.content_digest = super::identity::content_digest(&metric);
            metric.logical_bytes = crate::accounting::metric_logical_bytes(&metric);
            metric
        })
        .collect()
}

/// Resource-level context extracted once per resource_metrics
struct ResourceContext<'a> {
    /// The protobuf attributes, for the identity - see `IdentityInputs` for why a rendering will not do.
    proto_attributes: &'a [KeyValue],
    project_id: Option<String>,
    service_name: Option<String>,
    service_version: Option<String>,
    service_namespace: Option<String>,
    service_instance_id: Option<String>,
    environment: Option<String>,
    resource_attributes: JsonValue,
    schema_url: Option<String>,
}

impl<'a> ResourceContext<'a> {
    fn from_attrs(
        attrs: &HashMap<String, String>,
        resource_attributes: JsonValue,
        schema_url: Option<String>,
        proto_attributes: &'a [KeyValue],
    ) -> Self {
        Self {
            proto_attributes,
            project_id: attrs.get(PROJECT_ID_ATTR).cloned(),
            service_name: attrs.get(keys::SERVICE_NAME).cloned(),
            service_version: attrs.get(keys::SERVICE_VERSION).cloned(),
            service_namespace: attrs.get(keys::SERVICE_NAMESPACE).cloned(),
            service_instance_id: attrs.get(keys::SERVICE_INSTANCE_ID).cloned(),
            environment: get_environment(attrs),
            resource_attributes,
            schema_url,
        }
    }
}

/// Scope-level context
struct ScopeContext<'a> {
    proto_attributes: &'a [KeyValue],
    name: Option<String>,
    version: Option<String>,
    /// Typed, for the same reason the datapoint's own attributes are - see `attrs_to_typed_json`.
    attributes: JsonValue,
    schema_url: Option<String>,
}

/// Metric base info (name, description, unit)
struct MetricBase {
    name: String,
    description: Option<String>,
    unit: Option<String>,
}

/// Extract data points from a single metric
fn extract_metric_data_points<'a>(
    result: &mut Vec<(NormalizedMetric, IdentityInputs<'a>)>,
    metric: &'a Metric,
    ctx: &ResourceContext<'a>,
    scope: &ScopeContext<'a>,
) {
    let base = MetricBase {
        name: metric.name.clone(),
        description: (!metric.description.is_empty()).then(|| metric.description.clone()),
        unit: (!metric.unit.is_empty()).then(|| metric.unit.clone()),
    };

    let Some(ref data) = metric.data else { return };

    match data {
        Data::Gauge(g) => {
            for dp in &g.data_points {
                push_with_identity(
                    result,
                    ctx,
                    scope,
                    dp.attributes.as_slice(),
                    dp.time_unix_nano,
                    dp.start_time_unix_nano,
                    extract_number_dp(
                        ctx,
                        scope,
                        &base,
                        dp,
                        MetricType::Gauge,
                        AggregationTemporality::Unspecified,
                        None,
                        metric,
                    ),
                );
            }
        }
        Data::Sum(s) => {
            let temporality = AggregationTemporality::from_i32(s.aggregation_temporality);
            for dp in &s.data_points {
                push_with_identity(
                    result,
                    ctx,
                    scope,
                    dp.attributes.as_slice(),
                    dp.time_unix_nano,
                    dp.start_time_unix_nano,
                    extract_number_dp(
                        ctx,
                        scope,
                        &base,
                        dp,
                        MetricType::Sum,
                        temporality,
                        Some(s.is_monotonic),
                        metric,
                    ),
                );
            }
        }
        Data::Histogram(h) => {
            let temporality = AggregationTemporality::from_i32(h.aggregation_temporality);
            for dp in &h.data_points {
                push_with_identity(
                    result,
                    ctx,
                    scope,
                    dp.attributes.as_slice(),
                    dp.time_unix_nano,
                    dp.start_time_unix_nano,
                    extract_histogram_dp(ctx, scope, &base, dp, temporality, metric),
                );
            }
        }
        Data::ExponentialHistogram(eh) => {
            let temporality = AggregationTemporality::from_i32(eh.aggregation_temporality);
            for dp in &eh.data_points {
                push_with_identity(
                    result,
                    ctx,
                    scope,
                    dp.attributes.as_slice(),
                    dp.time_unix_nano,
                    dp.start_time_unix_nano,
                    extract_exp_histogram_dp(ctx, scope, &base, dp, temporality, metric),
                );
            }
        }
        Data::Summary(s) => {
            for dp in &s.data_points {
                push_with_identity(
                    result,
                    ctx,
                    scope,
                    dp.attributes.as_slice(),
                    dp.time_unix_nano,
                    dp.start_time_unix_nano,
                    extract_summary_dp(ctx, scope, &base, dp, metric),
                );
            }
        }
    }
}

/// Extract a number data point (Gauge or Sum)
#[allow(clippy::too_many_arguments)]
fn extract_number_dp(
    ctx: &ResourceContext,
    scope: &ScopeContext,
    base: &MetricBase,
    dp: &opentelemetry_proto::tonic::metrics::v1::NumberDataPoint,
    metric_type: MetricType,
    temporality: AggregationTemporality,
    is_monotonic: Option<bool>,
    metric: &Metric,
) -> NormalizedMetric {
    let attrs = extract_attributes(&dp.attributes);
    let (value_int, value_double) = match dp.value {
        Some(number_data_point::Value::AsInt(i)) => (Some(i), None),
        Some(number_data_point::Value::AsDouble(d)) => (None, Some(d)),
        None => (None, None),
    };

    let storable = storable_exemplars(&dp.exemplars);
    let exemplar = storable.first().copied();

    NormalizedMetric {
        project_id: ctx.project_id.clone(),
        metric_name: base.name.clone(),
        metric_description: base.description.clone(),
        metric_unit: base.unit.clone(),
        metric_type,
        aggregation_temporality: temporality,
        is_monotonic,
        timestamp: nanos_to_datetime(dp.time_unix_nano),
        start_timestamp: (dp.start_time_unix_nano > 0)
            .then(|| nanos_to_datetime(dp.start_time_unix_nano)),
        value_int,
        value_double,
        session_id: get_session_id(&attrs),
        user_id: get_user_id(&attrs),
        environment: ctx.environment.clone().or_else(|| get_environment(&attrs)),
        service_name: ctx.service_name.clone(),
        service_version: ctx.service_version.clone(),
        service_namespace: ctx.service_namespace.clone(),
        service_instance_id: ctx.service_instance_id.clone(),
        scope_name: scope.name.clone(),
        scope_version: scope.version.clone(),
        scope_attributes: scope.attributes.clone(),
        scope_schema_url: scope.schema_url.clone(),
        resource_schema_url: ctx.schema_url.clone(),
        attributes: attrs_to_typed_json(&dp.attributes),
        resource_attributes: ctx.resource_attributes.clone(),
        exemplar_trace_id: extract_exemplar_trace_id(exemplar),
        exemplar_span_id: extract_exemplar_span_id(exemplar),
        exemplar_value_int: extract_exemplar_value_int(exemplar),
        exemplar_value_double: extract_exemplar_value_double(exemplar),
        exemplar_timestamp: extract_exemplar_timestamp(exemplar),
        exemplar_attributes: extract_exemplar_attrs(exemplar),
        exemplars: extract_all_exemplars(&storable),
        flags: dp.flags,
        raw_metric: build_raw_metric_json(metric, metric_type),
        ..Default::default()
    }
}

/// Extract a histogram data point
fn extract_histogram_dp(
    ctx: &ResourceContext,
    scope: &ScopeContext,
    base: &MetricBase,
    dp: &opentelemetry_proto::tonic::metrics::v1::HistogramDataPoint,
    temporality: AggregationTemporality,
    metric: &Metric,
) -> NormalizedMetric {
    let attrs = extract_attributes(&dp.attributes);
    let storable = storable_exemplars(&dp.exemplars);
    let exemplar = storable.first().copied();

    NormalizedMetric {
        project_id: ctx.project_id.clone(),
        metric_name: base.name.clone(),
        metric_description: base.description.clone(),
        metric_unit: base.unit.clone(),
        metric_type: MetricType::Histogram,
        aggregation_temporality: temporality,
        timestamp: nanos_to_datetime(dp.time_unix_nano),
        start_timestamp: (dp.start_time_unix_nano > 0)
            .then(|| nanos_to_datetime(dp.start_time_unix_nano)),
        histogram_count: Some(dp.count),
        histogram_sum: dp.sum,
        histogram_min: dp.min,
        histogram_max: dp.max,
        histogram_bucket_counts: json!(dp.bucket_counts),
        histogram_explicit_bounds: json!(dp.explicit_bounds),
        session_id: get_session_id(&attrs),
        user_id: get_user_id(&attrs),
        environment: ctx.environment.clone().or_else(|| get_environment(&attrs)),
        service_name: ctx.service_name.clone(),
        service_version: ctx.service_version.clone(),
        service_namespace: ctx.service_namespace.clone(),
        service_instance_id: ctx.service_instance_id.clone(),
        scope_name: scope.name.clone(),
        scope_version: scope.version.clone(),
        scope_attributes: scope.attributes.clone(),
        scope_schema_url: scope.schema_url.clone(),
        resource_schema_url: ctx.schema_url.clone(),
        attributes: attrs_to_typed_json(&dp.attributes),
        resource_attributes: ctx.resource_attributes.clone(),
        exemplar_trace_id: extract_exemplar_trace_id(exemplar),
        exemplar_span_id: extract_exemplar_span_id(exemplar),
        exemplar_value_int: extract_exemplar_value_int(exemplar),
        exemplar_value_double: extract_exemplar_value_double(exemplar),
        exemplar_timestamp: extract_exemplar_timestamp(exemplar),
        exemplar_attributes: extract_exemplar_attrs(exemplar),
        exemplars: extract_all_exemplars(&storable),
        flags: dp.flags,
        raw_metric: build_raw_metric_json(metric, MetricType::Histogram),
        ..Default::default()
    }
}

/// Extract an exponential histogram data point
fn extract_exp_histogram_dp(
    ctx: &ResourceContext,
    scope: &ScopeContext,
    base: &MetricBase,
    dp: &opentelemetry_proto::tonic::metrics::v1::ExponentialHistogramDataPoint,
    temporality: AggregationTemporality,
    metric: &Metric,
) -> NormalizedMetric {
    let attrs = extract_attributes(&dp.attributes);
    let storable = storable_exemplars(&dp.exemplars);
    let exemplar = storable.first().copied();

    NormalizedMetric {
        project_id: ctx.project_id.clone(),
        metric_name: base.name.clone(),
        metric_description: base.description.clone(),
        metric_unit: base.unit.clone(),
        metric_type: MetricType::ExponentialHistogram,
        aggregation_temporality: temporality,
        timestamp: nanos_to_datetime(dp.time_unix_nano),
        start_timestamp: (dp.start_time_unix_nano > 0)
            .then(|| nanos_to_datetime(dp.start_time_unix_nano)),
        histogram_count: Some(dp.count),
        histogram_sum: dp.sum,
        histogram_min: dp.min,
        histogram_max: dp.max,
        exp_histogram_scale: Some(dp.scale),
        exp_histogram_zero_count: Some(dp.zero_count),
        exp_histogram_zero_threshold: Some(dp.zero_threshold),
        exp_histogram_positive: buckets_to_json(dp.positive.as_ref()),
        exp_histogram_negative: buckets_to_json(dp.negative.as_ref()),
        session_id: get_session_id(&attrs),
        user_id: get_user_id(&attrs),
        environment: ctx.environment.clone().or_else(|| get_environment(&attrs)),
        service_name: ctx.service_name.clone(),
        service_version: ctx.service_version.clone(),
        service_namespace: ctx.service_namespace.clone(),
        service_instance_id: ctx.service_instance_id.clone(),
        scope_name: scope.name.clone(),
        scope_version: scope.version.clone(),
        scope_attributes: scope.attributes.clone(),
        scope_schema_url: scope.schema_url.clone(),
        resource_schema_url: ctx.schema_url.clone(),
        attributes: attrs_to_typed_json(&dp.attributes),
        resource_attributes: ctx.resource_attributes.clone(),
        exemplar_trace_id: extract_exemplar_trace_id(exemplar),
        exemplar_span_id: extract_exemplar_span_id(exemplar),
        exemplar_value_int: extract_exemplar_value_int(exemplar),
        exemplar_value_double: extract_exemplar_value_double(exemplar),
        exemplar_timestamp: extract_exemplar_timestamp(exemplar),
        exemplar_attributes: extract_exemplar_attrs(exemplar),
        exemplars: extract_all_exemplars(&storable),
        flags: dp.flags,
        raw_metric: build_raw_metric_json(metric, MetricType::ExponentialHistogram),
        ..Default::default()
    }
}

/// Extract a summary data point
fn extract_summary_dp(
    ctx: &ResourceContext,
    scope: &ScopeContext,
    base: &MetricBase,
    dp: &opentelemetry_proto::tonic::metrics::v1::SummaryDataPoint,
    metric: &Metric,
) -> NormalizedMetric {
    let attrs = extract_attributes(&dp.attributes);

    // Convert quantile values to JSON
    let quantiles: Vec<JsonValue> = dp
        .quantile_values
        .iter()
        .map(|q| json!({"quantile": q.quantile, "value": q.value}))
        .collect();

    NormalizedMetric {
        project_id: ctx.project_id.clone(),
        metric_name: base.name.clone(),
        metric_description: base.description.clone(),
        metric_unit: base.unit.clone(),
        metric_type: MetricType::Summary,
        aggregation_temporality: AggregationTemporality::Unspecified,
        timestamp: nanos_to_datetime(dp.time_unix_nano),
        start_timestamp: (dp.start_time_unix_nano > 0)
            .then(|| nanos_to_datetime(dp.start_time_unix_nano)),
        summary_count: Some(dp.count),
        summary_sum: Some(dp.sum),
        summary_quantiles: JsonValue::Array(quantiles),
        session_id: get_session_id(&attrs),
        user_id: get_user_id(&attrs),
        environment: ctx.environment.clone().or_else(|| get_environment(&attrs)),
        service_name: ctx.service_name.clone(),
        service_version: ctx.service_version.clone(),
        service_namespace: ctx.service_namespace.clone(),
        service_instance_id: ctx.service_instance_id.clone(),
        scope_name: scope.name.clone(),
        scope_version: scope.version.clone(),
        scope_attributes: scope.attributes.clone(),
        scope_schema_url: scope.schema_url.clone(),
        resource_schema_url: ctx.schema_url.clone(),
        attributes: attrs_to_typed_json(&dp.attributes),
        resource_attributes: ctx.resource_attributes.clone(),
        flags: dp.flags,
        raw_metric: build_raw_metric_json(metric, MetricType::Summary),
        ..Default::default()
    }
}

// ============================================================================
// EXEMPLAR HELPERS
// ============================================================================

fn extract_exemplar_trace_id(
    exemplar: Option<&opentelemetry_proto::tonic::metrics::v1::Exemplar>,
) -> Option<String> {
    exemplar
        .map(|e| hex::encode(&e.trace_id))
        .filter(|s| !s.is_empty() && s != "00000000000000000000000000000000")
}

fn extract_exemplar_span_id(
    exemplar: Option<&opentelemetry_proto::tonic::metrics::v1::Exemplar>,
) -> Option<String> {
    exemplar
        .map(|e| hex::encode(&e.span_id))
        .filter(|s| !s.is_empty() && s != "0000000000000000")
}

fn extract_exemplar_value_int(
    exemplar: Option<&opentelemetry_proto::tonic::metrics::v1::Exemplar>,
) -> Option<i64> {
    use opentelemetry_proto::tonic::metrics::v1::exemplar::Value;
    exemplar.and_then(|e| match &e.value {
        Some(Value::AsInt(i)) => Some(*i),
        _ => None,
    })
}

fn extract_exemplar_value_double(
    exemplar: Option<&opentelemetry_proto::tonic::metrics::v1::Exemplar>,
) -> Option<f64> {
    use opentelemetry_proto::tonic::metrics::v1::exemplar::Value;
    exemplar.and_then(|e| match &e.value {
        Some(Value::AsDouble(d)) => Some(*d),
        _ => None,
    })
}

fn extract_exemplar_timestamp(
    exemplar: Option<&opentelemetry_proto::tonic::metrics::v1::Exemplar>,
) -> Option<chrono::DateTime<chrono::Utc>> {
    exemplar
        .filter(|e| e.time_unix_nano > 0)
        .map(|e| nanos_to_datetime(e.time_unix_nano))
}

/// The exemplars a backend can actually store, in order.
///
/// Filtered *once*, and both the flat `exemplar_*` fields and the `exemplars` array are derived from the
/// result - otherwise the two disagree about the same data point. That is what happened when only the flat
/// fields were validated (in `metrics::ingest`): an exemplar dated year 3000 had its flat copy cleared while
/// the array still carried it, so the row simultaneously said "no exemplar" and held one. Filtering here
/// also means no timestamp is parsed back out of a string, and the *later* exemplars are checked at all -
/// only the first ever was.
///
/// A bad exemplar clock drops the exemplar, never the measurement: an exemplar is an auxiliary debugging
/// sample, and refusing a real measurement over one would be the wrong trade. `metrics::ingest` keeps its
/// own check as a backstop for anything not built through this extractor.
fn storable_exemplars(
    exemplars: &[opentelemetry_proto::tonic::metrics::v1::Exemplar],
) -> Vec<&opentelemetry_proto::tonic::metrics::v1::Exemplar> {
    use opentelemetry_proto::tonic::metrics::v1::exemplar::Value;
    exemplars
        .iter()
        .filter(|e| e.time_unix_nano == 0 || is_storable(nanos_to_datetime(e.time_unix_nano)))
        // A non-finite value is unstorable for the same reason an unstorable instant is: the two
        // representations cannot agree about it. The flat `exemplar_value_double` column keeps the NaN or
        // infinity, while the JSON array goes through `serde_json`, which has no non-finite numbers and
        // writes `null` - so the indexed column and the full array described different values for one
        // exemplar. Dropped here, in the one filter both derive from, so they agree by absence; the
        // measurement itself is untouched, exactly as for a bad exemplar clock.
        .filter(|e| !matches!(&e.value, Some(Value::AsDouble(d)) if !d.is_finite()))
        .collect()
}

/// Every storable exemplar of a data point, as a JSON array; `Null` when there are none.
///
/// The six flat `exemplar_*` columns keep the *first* one - they are what the trace-correlation index is
/// built on and what the queries read. But a histogram carries one exemplar **per bucket**, which is the
/// whole point of them: they are how a reader gets from a slow bucket to the trace that was slow. Keeping
/// only the first discarded every link but one, so a latency histogram with ten populated buckets offered
/// one trace out of ten and gave no sign the others had been received.
fn extract_all_exemplars(
    exemplars: &[&opentelemetry_proto::tonic::metrics::v1::Exemplar],
) -> JsonValue {
    use opentelemetry_proto::tonic::metrics::v1::exemplar::Value;

    if exemplars.is_empty() {
        return JsonValue::Null;
    }

    let entries: Vec<JsonValue> = exemplars
        .iter()
        .map(|e| {
            let mut entry = serde_json::Map::new();
            if let Some(trace_id) = Some(hex::encode(&e.trace_id))
                .filter(|s| !s.is_empty() && s != "00000000000000000000000000000000")
            {
                entry.insert("trace_id".to_string(), JsonValue::String(trace_id));
            }
            if let Some(span_id) =
                Some(hex::encode(&e.span_id)).filter(|s| !s.is_empty() && s != "0000000000000000")
            {
                entry.insert("span_id".to_string(), JsonValue::String(span_id));
            }
            match &e.value {
                Some(Value::AsInt(i)) => {
                    entry.insert("value_int".to_string(), serde_json::json!(i));
                }
                Some(Value::AsDouble(d)) => {
                    entry.insert("value_double".to_string(), serde_json::json!(d));
                }
                None => {}
            }
            if e.time_unix_nano > 0 {
                entry.insert(
                    "timestamp".to_string(),
                    JsonValue::String(nanos_to_datetime(e.time_unix_nano).to_rfc3339()),
                );
            }
            if !e.filtered_attributes.is_empty() {
                // Typed, not stringified. The ordinary extraction path turns every value into a string,
                // which is right for display and wrong for a record of what was received: `status_code=200`
                // as an int and `status_code="200"` as a string both became `"200"`, and a bool became
                // `"true"`. An exemplar exists to let someone get back to the exact call, so the value it
                // carries has to be the value that was sent.
                entry.insert(
                    "attributes".to_string(),
                    crate::otlp::attrs_to_typed_json(&e.filtered_attributes),
                );
            }
            JsonValue::Object(entry)
        })
        .collect();

    JsonValue::Array(entries)
}

/// The first exemplar's attributes, typed exactly as the `exemplars` array stores them.
///
/// Both representations describe the same exemplar, so they have to agree: with this one going through the
/// stringifying path, an integer `status_code=200` was `200` in the array and `"200"` in this column, and
/// this is the column the queries expose.
fn extract_exemplar_attrs(
    exemplar: Option<&opentelemetry_proto::tonic::metrics::v1::Exemplar>,
) -> JsonValue {
    exemplar
        .map(|e| crate::otlp::attrs_to_typed_json(&e.filtered_attributes))
        .unwrap_or(JsonValue::Null)
}

// ============================================================================
// HELPER FUNCTIONS
// ============================================================================

/// Convert exponential histogram buckets to JSON
fn buckets_to_json(buckets: Option<&Buckets>) -> JsonValue {
    match buckets {
        Some(b) => json!({
            "offset": b.offset,
            "bucket_counts": b.bucket_counts
        }),
        None => JsonValue::Null,
    }
}

/// Build raw metric JSON for debugging
fn build_raw_metric_json(metric: &Metric, metric_type: MetricType) -> JsonValue {
    let mut map = serde_json::Map::new();

    // Identity
    map.insert("name".into(), json!(&metric.name));
    map.insert("description".into(), json!(&metric.description));
    map.insert("unit".into(), json!(&metric.unit));
    map.insert("type".into(), json!(metric_type.as_str()));

    // Type-specific info
    if let Some(ref data) = metric.data {
        match data {
            Data::Sum(s) => {
                map.insert(
                    "aggregation_temporality".into(),
                    json!(s.aggregation_temporality),
                );
                map.insert("is_monotonic".into(), json!(s.is_monotonic));
            }
            Data::Histogram(h) => {
                map.insert(
                    "aggregation_temporality".into(),
                    json!(h.aggregation_temporality),
                );
            }
            Data::ExponentialHistogram(eh) => {
                map.insert(
                    "aggregation_temporality".into(),
                    json!(eh.aggregation_temporality),
                );
            }
            _ => {}
        }
    }

    JsonValue::Object(map)
}
#[cfg(test)]
#[path = "extract_tests.rs"]
mod tests;
