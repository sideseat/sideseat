//! Metric extraction from OTLP protobuf
//!
//! Extracts and flattens metrics into one NormalizedMetric per data point.
//! Supports all 5 OTLP metric types: Gauge, Sum, Histogram, ExponentialHistogram, Summary.

use std::collections::HashMap;

use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
use opentelemetry_proto::tonic::metrics::v1::{
    Metric, exponential_histogram_data_point::Buckets, metric::Data, number_data_point,
};
use serde_json::{Value as JsonValue, json};

use crate::data::types::{AggregationTemporality, MetricType, NormalizedMetric};
use crate::utils::otlp::{
    PROJECT_ID_ATTR, attrs_to_json, extract_attributes, get_environment, get_session_id,
    get_user_id, keys,
};
use crate::utils::time::nanos_to_datetime;

/// Extract and flatten all metrics from an OTLP request.
/// Returns one NormalizedMetric per data point.
pub fn extract_metrics_batch(request: &ExportMetricsServiceRequest) -> Vec<NormalizedMetric> {
    let mut result = Vec::new();

    for resource_metrics in &request.resource_metrics {
        let resource = resource_metrics.resource.as_ref();
        let resource_attrs = resource
            .map(|r| extract_attributes(&r.attributes))
            .unwrap_or_default();

        let ctx = ResourceContext::from_attrs(&resource_attrs);

        for scope_metrics in &resource_metrics.scope_metrics {
            let scope = scope_metrics.scope.as_ref();
            let scope_ctx = ScopeContext {
                name: scope.map(|s| s.name.clone()).filter(|s| !s.is_empty()),
                version: scope.and_then(|s| (!s.version.is_empty()).then(|| s.version.clone())),
            };

            for metric in &scope_metrics.metrics {
                extract_metric_data_points(&mut result, metric, &ctx, &scope_ctx);
            }
        }
    }

    result
}

/// Resource-level context extracted once per resource_metrics
struct ResourceContext {
    project_id: Option<String>,
    service_name: Option<String>,
    service_version: Option<String>,
    service_namespace: Option<String>,
    service_instance_id: Option<String>,
    environment: Option<String>,
    resource_attributes: JsonValue,
}

impl ResourceContext {
    fn from_attrs(attrs: &HashMap<String, String>) -> Self {
        Self {
            project_id: attrs.get(PROJECT_ID_ATTR).cloned(),
            service_name: attrs.get(keys::SERVICE_NAME).cloned(),
            service_version: attrs.get(keys::SERVICE_VERSION).cloned(),
            service_namespace: attrs.get(keys::SERVICE_NAMESPACE).cloned(),
            service_instance_id: attrs.get(keys::SERVICE_INSTANCE_ID).cloned(),
            environment: get_environment(attrs),
            resource_attributes: attrs_to_json(attrs),
        }
    }
}

/// Scope-level context
struct ScopeContext {
    name: Option<String>,
    version: Option<String>,
}

/// Metric base info (name, description, unit)
struct MetricBase {
    name: String,
    description: Option<String>,
    unit: Option<String>,
}

/// Extract data points from a single metric
fn extract_metric_data_points(
    result: &mut Vec<NormalizedMetric>,
    metric: &Metric,
    ctx: &ResourceContext,
    scope: &ScopeContext,
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
                result.push(extract_number_dp(
                    ctx,
                    scope,
                    &base,
                    dp,
                    MetricType::Gauge,
                    AggregationTemporality::Unspecified,
                    None,
                    metric,
                ));
            }
        }
        Data::Sum(s) => {
            let temporality = AggregationTemporality::from_i32(s.aggregation_temporality);
            for dp in &s.data_points {
                result.push(extract_number_dp(
                    ctx,
                    scope,
                    &base,
                    dp,
                    MetricType::Sum,
                    temporality,
                    Some(s.is_monotonic),
                    metric,
                ));
            }
        }
        Data::Histogram(h) => {
            let temporality = AggregationTemporality::from_i32(h.aggregation_temporality);
            for dp in &h.data_points {
                result.push(extract_histogram_dp(
                    ctx,
                    scope,
                    &base,
                    dp,
                    temporality,
                    metric,
                ));
            }
        }
        Data::ExponentialHistogram(eh) => {
            let temporality = AggregationTemporality::from_i32(eh.aggregation_temporality);
            for dp in &eh.data_points {
                result.push(extract_exp_histogram_dp(
                    ctx,
                    scope,
                    &base,
                    dp,
                    temporality,
                    metric,
                ));
            }
        }
        Data::Summary(s) => {
            for dp in &s.data_points {
                result.push(extract_summary_dp(ctx, scope, &base, dp, metric));
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

    let exemplar = dp.exemplars.first();

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
        attributes: attrs_to_json(&attrs),
        resource_attributes: ctx.resource_attributes.clone(),
        exemplar_trace_id: extract_exemplar_trace_id(exemplar),
        exemplar_span_id: extract_exemplar_span_id(exemplar),
        exemplar_value_int: extract_exemplar_value_int(exemplar),
        exemplar_value_double: extract_exemplar_value_double(exemplar),
        exemplar_timestamp: extract_exemplar_timestamp(exemplar),
        exemplar_attributes: extract_exemplar_attrs(exemplar),
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
    let exemplar = dp.exemplars.first();

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
        attributes: attrs_to_json(&attrs),
        resource_attributes: ctx.resource_attributes.clone(),
        exemplar_trace_id: extract_exemplar_trace_id(exemplar),
        exemplar_span_id: extract_exemplar_span_id(exemplar),
        exemplar_value_int: extract_exemplar_value_int(exemplar),
        exemplar_value_double: extract_exemplar_value_double(exemplar),
        exemplar_timestamp: extract_exemplar_timestamp(exemplar),
        exemplar_attributes: extract_exemplar_attrs(exemplar),
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
    let exemplar = dp.exemplars.first();

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
        attributes: attrs_to_json(&attrs),
        resource_attributes: ctx.resource_attributes.clone(),
        exemplar_trace_id: extract_exemplar_trace_id(exemplar),
        exemplar_span_id: extract_exemplar_span_id(exemplar),
        exemplar_value_int: extract_exemplar_value_int(exemplar),
        exemplar_value_double: extract_exemplar_value_double(exemplar),
        exemplar_timestamp: extract_exemplar_timestamp(exemplar),
        exemplar_attributes: extract_exemplar_attrs(exemplar),
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
        attributes: attrs_to_json(&attrs),
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
}
