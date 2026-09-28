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

/// A non-finite exemplar value is dropped from both representations, for the same reason.
///
/// JSON has no NaN or infinity: the array goes through `serde_json`, which writes `null`, while the flat
/// `exemplar_value_double` column keeps the float - so the indexed column and the full array described
/// different values for one exemplar. Unstorable, therefore, exactly as an unstorable instant is, and
/// dropped in the one filter both derive from.
#[test]
fn a_non_finite_exemplar_value_is_dropped_from_both_representations() {
    use opentelemetry_proto::tonic::metrics::v1::{Exemplar, exemplar};

    const GOOD_NANOS: u64 = 1_704_067_200_000_000_000;

    fn with_value(value: f64, trace: u8) -> Exemplar {
        Exemplar {
            trace_id: vec![trace; 16],
            span_id: vec![trace; 8],
            time_unix_nano: GOOD_NANOS,
            value: Some(exemplar::Value::AsDouble(value)),
            filtered_attributes: vec![],
        }
    }

    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
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
                                exemplars: vec![with_value(bad, 1), with_value(1.0, 2)],
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
        assert_eq!(result.len(), 1, "the measurement itself must survive {bad}");
        let metric = &result[0];
        let all = metric.exemplars.as_array().expect("the finite one remains");
        assert_eq!(all.len(), 1, "only the finite exemplar is kept for {bad}");
        assert_eq!(all[0]["value_double"], 1.0);
        assert_eq!(
            metric.exemplar_value_double,
            Some(1.0),
            "the flat value and the array must agree for {bad}"
        );
        assert_eq!(
            metric.exemplar_trace_id.as_deref(),
            Some(&"02".repeat(16)[..])
        );
    }
}
