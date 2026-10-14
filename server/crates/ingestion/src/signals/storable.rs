//! What an export holds that cannot be stored: records whose instants are outside what the stores represent.

use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
use opentelemetry_proto::tonic::metrics::v1::metric;

fn storable_nanos(value: u64) -> bool {
    value == 0
        || sideseat_core::utils::time::is_storable(sideseat_core::utils::time::nanos_to_datetime(
            value,
        ))
}

pub(crate) fn strip_unstorable_metrics(request: &mut ExportMetricsServiceRequest) -> usize {
    let mut removed = 0usize;
    for resource in &mut request.resource_metrics {
        for scope in &mut resource.scope_metrics {
            for metric in &mut scope.metrics {
                let keep = |time: u64, start: u64| storable_nanos(time) && storable_nanos(start);
                match metric.data.as_mut() {
                    Some(metric::Data::Gauge(data)) => {
                        let before = data.data_points.len();
                        data.data_points
                            .retain(|point| keep(point.time_unix_nano, point.start_time_unix_nano));
                        removed += before - data.data_points.len();
                    }
                    Some(metric::Data::Sum(data)) => {
                        let before = data.data_points.len();
                        data.data_points
                            .retain(|point| keep(point.time_unix_nano, point.start_time_unix_nano));
                        removed += before - data.data_points.len();
                    }
                    Some(metric::Data::Histogram(data)) => {
                        let before = data.data_points.len();
                        data.data_points
                            .retain(|point| keep(point.time_unix_nano, point.start_time_unix_nano));
                        removed += before - data.data_points.len();
                    }
                    Some(metric::Data::ExponentialHistogram(data)) => {
                        let before = data.data_points.len();
                        data.data_points
                            .retain(|point| keep(point.time_unix_nano, point.start_time_unix_nano));
                        removed += before - data.data_points.len();
                    }
                    Some(metric::Data::Summary(data)) => {
                        let before = data.data_points.len();
                        data.data_points
                            .retain(|point| keep(point.time_unix_nano, point.start_time_unix_nano));
                        removed += before - data.data_points.len();
                    }
                    None => {}
                }
            }
            scope.metrics.retain(|metric| match metric.data.as_ref() {
                Some(metric::Data::Gauge(data)) => !data.data_points.is_empty(),
                Some(metric::Data::Sum(data)) => !data.data_points.is_empty(),
                Some(metric::Data::Histogram(data)) => !data.data_points.is_empty(),
                Some(metric::Data::ExponentialHistogram(data)) => !data.data_points.is_empty(),
                Some(metric::Data::Summary(data)) => !data.data_points.is_empty(),
                None => false,
            });
        }
    }
    removed
}

pub(crate) fn strip_unstorable_logs(request: &mut ExportLogsServiceRequest) -> usize {
    let mut removed = 0usize;
    for resource in &mut request.resource_logs {
        for scope in &mut resource.scope_logs {
            let before = scope.log_records.len();
            scope.log_records.retain(|record| {
                storable_nanos(record.time_unix_nano)
                    && storable_nanos(record.observed_time_unix_nano)
            });
            removed += before - scope.log_records.len();
        }
    }
    removed
}
