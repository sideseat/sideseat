//! Metric extraction from OTLP protobuf
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

use crate::data::types::{AggregationTemporality, MetricType, NormalizedMetric};
use crate::utils::otlp::{
    PROJECT_ID_ATTR, attrs_to_json, attrs_to_typed_json, extract_attributes, get_environment,
    get_session_id, get_user_id, keys,
};
use crate::utils::time::{is_storable, nanos_to_datetime};

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
    exemplars
        .iter()
        .filter(|e| e.time_unix_nano == 0 || is_storable(nanos_to_datetime(e.time_unix_nano)))
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
                    crate::utils::otlp::attrs_to_typed_json(&e.filtered_attributes),
                );
            }
            JsonValue::Object(entry)
        })
        .collect();

    JsonValue::Array(entries)
}

fn extract_exemplar_attrs(
    exemplar: Option<&opentelemetry_proto::tonic::metrics::v1::Exemplar>,
) -> JsonValue {
    exemplar
        .map(|e| attrs_to_json(&extract_attributes(&e.filtered_attributes)))
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
mod tests {
    use super::*;
    use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue, any_value};
    use opentelemetry_proto::tonic::metrics::v1::{
        Gauge, Histogram, HistogramDataPoint, NumberDataPoint, ResourceMetrics, ScopeMetrics, Sum,
    };
    use opentelemetry_proto::tonic::resource::v1::Resource;

    fn make_key_value(key: &str, value: &str) -> KeyValue {
        KeyValue {
            key: key.to_string(),
            value: Some(AnyValue {
                value: Some(any_value::Value::StringValue(value.to_string())),
            }),
        }
    }

    #[test]
    fn test_extract_empty_request() {
        let request = ExportMetricsServiceRequest::default();
        let result = extract_metrics_batch(&request);
        assert!(result.is_empty());
    }

    #[test]
    fn test_extract_gauge_metric() {
        let request = ExportMetricsServiceRequest {
            resource_metrics: vec![ResourceMetrics {
                resource: Some(Resource {
                    attributes: vec![make_key_value("sideseat.project_id", "test-project")],
                    ..Default::default()
                }),
                scope_metrics: vec![ScopeMetrics {
                    metrics: vec![Metric {
                        name: "test.gauge".to_string(),
                        description: "A test gauge".to_string(),
                        unit: "1".to_string(),
                        data: Some(Data::Gauge(Gauge {
                            data_points: vec![NumberDataPoint {
                                time_unix_nano: 1_704_067_200_000_000_000,
                                value: Some(number_data_point::Value::AsDouble(42.5)),
                                ..Default::default()
                            }],
                        })),
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            }],
        };

        let result = extract_metrics_batch(&request);
        assert_eq!(result.len(), 1);

        let metric = &result[0];
        assert_eq!(metric.project_id, Some("test-project".to_string()));
        assert_eq!(metric.metric_name, "test.gauge");
        assert_eq!(metric.metric_type, MetricType::Gauge);
        assert_eq!(metric.value_double, Some(42.5));
        assert_eq!(
            metric.aggregation_temporality,
            AggregationTemporality::Unspecified
        );
    }

    #[test]
    fn test_extract_sum_metric() {
        let request = ExportMetricsServiceRequest {
            resource_metrics: vec![ResourceMetrics {
                scope_metrics: vec![ScopeMetrics {
                    metrics: vec![Metric {
                        name: "test.counter".to_string(),
                        data: Some(Data::Sum(Sum {
                            data_points: vec![NumberDataPoint {
                                time_unix_nano: 1_704_067_200_000_000_000,
                                value: Some(number_data_point::Value::AsInt(100)),
                                ..Default::default()
                            }],
                            aggregation_temporality: 2, // Cumulative
                            is_monotonic: true,
                        })),
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            }],
        };

        let result = extract_metrics_batch(&request);
        assert_eq!(result.len(), 1);

        let metric = &result[0];
        assert_eq!(metric.metric_type, MetricType::Sum);
        assert_eq!(metric.value_int, Some(100));
        assert_eq!(metric.is_monotonic, Some(true));
        assert_eq!(
            metric.aggregation_temporality,
            AggregationTemporality::Cumulative
        );
    }

    #[test]
    fn test_extract_histogram_metric() {
        let request = ExportMetricsServiceRequest {
            resource_metrics: vec![ResourceMetrics {
                scope_metrics: vec![ScopeMetrics {
                    metrics: vec![Metric {
                        name: "test.histogram".to_string(),
                        data: Some(Data::Histogram(Histogram {
                            data_points: vec![HistogramDataPoint {
                                time_unix_nano: 1_704_067_200_000_000_000,
                                count: 100,
                                sum: Some(500.0),
                                min: Some(1.0),
                                max: Some(10.0),
                                bucket_counts: vec![10, 20, 30, 40],
                                explicit_bounds: vec![1.0, 5.0, 10.0],
                                ..Default::default()
                            }],
                            aggregation_temporality: 1, // Delta
                        })),
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            }],
        };

        let result = extract_metrics_batch(&request);
        assert_eq!(result.len(), 1);

        let metric = &result[0];
        assert_eq!(metric.metric_type, MetricType::Histogram);
        assert_eq!(metric.histogram_count, Some(100));
        assert_eq!(metric.histogram_sum, Some(500.0));
        assert_eq!(
            metric.aggregation_temporality,
            AggregationTemporality::Delta
        );
    }

    #[test]
    fn test_extract_context_from_resource_attrs() {
        let request = ExportMetricsServiceRequest {
            resource_metrics: vec![ResourceMetrics {
                resource: Some(Resource {
                    attributes: vec![
                        make_key_value("sideseat.project_id", "my-project"),
                        make_key_value("service.name", "my-service"),
                        make_key_value("service.version", "1.0.0"),
                        make_key_value("deployment.environment", "production"),
                    ],
                    ..Default::default()
                }),
                scope_metrics: vec![ScopeMetrics {
                    metrics: vec![Metric {
                        name: "test.metric".to_string(),
                        data: Some(Data::Gauge(Gauge {
                            data_points: vec![NumberDataPoint {
                                time_unix_nano: 1_704_067_200_000_000_000,
                                value: Some(number_data_point::Value::AsDouble(1.0)),
                                ..Default::default()
                            }],
                        })),
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            }],
        };

        let result = extract_metrics_batch(&request);
        assert_eq!(result.len(), 1);

        let metric = &result[0];
        assert_eq!(metric.project_id, Some("my-project".to_string()));
        assert_eq!(metric.service_name, Some("my-service".to_string()));
        assert_eq!(metric.service_version, Some("1.0.0".to_string()));
        assert_eq!(metric.environment, Some("production".to_string()));
    }

    #[test]
    fn test_extract_exponential_histogram_metric() {
        use opentelemetry_proto::tonic::metrics::v1::{
            ExponentialHistogram, ExponentialHistogramDataPoint,
        };

        let request = ExportMetricsServiceRequest {
            resource_metrics: vec![ResourceMetrics {
                scope_metrics: vec![ScopeMetrics {
                    metrics: vec![Metric {
                        name: "test.exp_histogram".to_string(),
                        data: Some(Data::ExponentialHistogram(ExponentialHistogram {
                            data_points: vec![ExponentialHistogramDataPoint {
                                time_unix_nano: 1_704_067_200_000_000_000,
                                count: 50,
                                sum: Some(250.0),
                                scale: 3,
                                zero_count: 5,
                                zero_threshold: 0.001,
                                positive: Some(
                                    opentelemetry_proto::tonic::metrics::v1::exponential_histogram_data_point::Buckets {
                                        offset: 0,
                                        bucket_counts: vec![10, 15, 20],
                                    },
                                ),
                                negative: None,
                                ..Default::default()
                            }],
                            aggregation_temporality: 2, // Cumulative
                        })),
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            }],
        };

        let result = extract_metrics_batch(&request);
        assert_eq!(result.len(), 1);

        let metric = &result[0];
        assert_eq!(metric.metric_type, MetricType::ExponentialHistogram);
        assert_eq!(metric.histogram_count, Some(50));
        assert_eq!(metric.histogram_sum, Some(250.0));
        assert_eq!(metric.exp_histogram_scale, Some(3));
        assert_eq!(metric.exp_histogram_zero_count, Some(5));
        assert_eq!(
            metric.aggregation_temporality,
            AggregationTemporality::Cumulative
        );
    }

    #[test]
    fn test_extract_summary_metric() {
        use opentelemetry_proto::tonic::metrics::v1::{
            Summary, SummaryDataPoint, summary_data_point::ValueAtQuantile,
        };

        let request = ExportMetricsServiceRequest {
            resource_metrics: vec![ResourceMetrics {
                scope_metrics: vec![ScopeMetrics {
                    metrics: vec![Metric {
                        name: "test.summary".to_string(),
                        data: Some(Data::Summary(Summary {
                            data_points: vec![SummaryDataPoint {
                                time_unix_nano: 1_704_067_200_000_000_000,
                                count: 1000,
                                sum: 5000.0,
                                quantile_values: vec![
                                    ValueAtQuantile {
                                        quantile: 0.5,
                                        value: 4.5,
                                    },
                                    ValueAtQuantile {
                                        quantile: 0.99,
                                        value: 9.8,
                                    },
                                ],
                                ..Default::default()
                            }],
                        })),
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            }],
        };

        let result = extract_metrics_batch(&request);
        assert_eq!(result.len(), 1);

        let metric = &result[0];
        assert_eq!(metric.metric_type, MetricType::Summary);
        assert_eq!(metric.summary_count, Some(1000));
        assert_eq!(metric.summary_sum, Some(5000.0));

        // Check quantiles
        let quantiles = metric.summary_quantiles.as_array().unwrap();
        assert_eq!(quantiles.len(), 2);
        assert_eq!(quantiles[0]["quantile"], 0.5);
        assert_eq!(quantiles[0]["value"], 4.5);
        assert_eq!(quantiles[1]["quantile"], 0.99);
        assert_eq!(quantiles[1]["value"], 9.8);
    }

    /// Every exemplar reaches storage, not only the first.
    ///
    /// A histogram carries one exemplar per bucket, so a latency histogram with three populated buckets
    /// offers three traces to jump to - and keeping `exemplars.first()` alone silently discarded two of
    /// them, with nothing in the row to say they had been received.
    #[test]
    fn every_exemplar_of_a_data_point_is_kept() {
        use opentelemetry_proto::tonic::metrics::v1::{Exemplar, exemplar};

        fn exemplar_at(trace: u8, value: f64) -> Exemplar {
            Exemplar {
                trace_id: vec![trace; 16],
                span_id: vec![trace; 8],
                time_unix_nano: 1_704_067_200_000_000_000,
                value: Some(exemplar::Value::AsDouble(value)),
                filtered_attributes: vec![make_key_value("bucket", &trace.to_string())],
            }
        }

        let request = ExportMetricsServiceRequest {
            resource_metrics: vec![ResourceMetrics {
                resource: Some(Resource {
                    attributes: vec![make_key_value("sideseat.project_id", "test-project")],
                    ..Default::default()
                }),
                scope_metrics: vec![ScopeMetrics {
                    metrics: vec![Metric {
                        name: "http.server.duration".to_string(),
                        data: Some(Data::Histogram(Histogram {
                            data_points: vec![HistogramDataPoint {
                                time_unix_nano: 1_704_067_200_000_000_000,
                                count: 3,
                                sum: Some(6.0),
                                exemplars: vec![
                                    exemplar_at(1, 0.5),
                                    exemplar_at(2, 2.0),
                                    exemplar_at(3, 3.5),
                                ],
                                ..Default::default()
                            }],
                            ..Default::default()
                        })),
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            }],
        };

        let result = extract_metrics_batch(&request);
        assert_eq!(result.len(), 1);
        let metric = &result[0];

        // The flat columns still carry the first, because the trace-correlation index is built on them.
        assert_eq!(
            metric.exemplar_trace_id.as_deref(),
            Some(&"01".repeat(16)[..])
        );

        let all = metric
            .exemplars
            .as_array()
            .expect("exemplars must be an array when the data point carried any");
        assert_eq!(all.len(), 3, "all three bucket exemplars must be kept");
        assert_eq!(all[1]["trace_id"], "02".repeat(16));
        assert_eq!(all[1]["span_id"], "02".repeat(8));
        assert_eq!(all[1]["value_double"], 2.0);
        assert_eq!(all[2]["attributes"]["bucket"], "3");
        assert!(
            all[0]["timestamp"].is_string(),
            "an exemplar's own timestamp is what links it to its trace's instant"
        );
    }

    /// No exemplars means no array, rather than an empty one: nothing was received and the row says so.
    #[test]
    fn a_data_point_with_no_exemplars_stores_none() {
        let request = ExportMetricsServiceRequest {
            resource_metrics: vec![ResourceMetrics {
                resource: Some(Resource {
                    attributes: vec![make_key_value("sideseat.project_id", "test-project")],
                    ..Default::default()
                }),
                scope_metrics: vec![ScopeMetrics {
                    metrics: vec![Metric {
                        name: "test.gauge".to_string(),
                        data: Some(Data::Gauge(Gauge {
                            data_points: vec![NumberDataPoint {
                                time_unix_nano: 1_704_067_200_000_000_000,
                                value: Some(number_data_point::Value::AsDouble(1.0)),
                                ..Default::default()
                            }],
                        })),
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            }],
        };

        let result = extract_metrics_batch(&request);
        assert!(result[0].exemplars.is_null());
    }

    /// An exemplar with an unstorable clock is dropped from *both* representations, and the ones after it
    /// survive.
    ///
    /// Validating only the flat first-exemplar fields left the row contradicting itself: the flat copy was
    /// cleared while the array still held the same year-3000 instant. And a bad timestamp on any exemplar
    /// but the first was never examined at all.
    #[test]
    fn an_unstorable_exemplar_is_dropped_from_both_representations() {
        use opentelemetry_proto::tonic::metrics::v1::{Exemplar, exemplar};

        // Year ~2554: past what a microsecond-precision column can hold.
        const UNSTORABLE_NANOS: u64 = 18_446_744_073_000_000_000;
        const GOOD_NANOS: u64 = 1_704_067_200_000_000_000;

        fn at(nanos: u64, trace: u8) -> Exemplar {
            Exemplar {
                trace_id: vec![trace; 16],
                span_id: vec![trace; 8],
                time_unix_nano: nanos,
                value: Some(exemplar::Value::AsDouble(1.0)),
                filtered_attributes: vec![],
            }
        }

        let request = ExportMetricsServiceRequest {
            resource_metrics: vec![ResourceMetrics {
                resource: Some(Resource {
                    attributes: vec![make_key_value("sideseat.project_id", "test-project")],
                    ..Default::default()
                }),
                scope_metrics: vec![ScopeMetrics {
                    metrics: vec![Metric {
                        name: "http.server.duration".to_string(),
                        data: Some(Data::Histogram(Histogram {
                            data_points: vec![HistogramDataPoint {
                                time_unix_nano: GOOD_NANOS,
                                count: 2,
                                // The bad one first, so the flat fields would have taken it.
                                exemplars: vec![at(UNSTORABLE_NANOS, 1), at(GOOD_NANOS, 2)],
                                ..Default::default()
                            }],
                            ..Default::default()
                        })),
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            }],
        };

        let result = extract_metrics_batch(&request);
        assert_eq!(result.len(), 1, "the measurement itself must survive");
        let metric = &result[0];

        let all = metric
            .exemplars
            .as_array()
            .expect("the good exemplar remains");
        assert_eq!(all.len(), 1, "only the storable exemplar is kept");
        assert_eq!(all[0]["trace_id"], "02".repeat(16));

        // And the flat fields name the same one, rather than the dropped one or nothing.
        assert_eq!(
            metric.exemplar_trace_id.as_deref(),
            Some(&"02".repeat(16)[..]),
            "the flat fields and the array must describe the same exemplar"
        );
    }
}
