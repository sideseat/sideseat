//! What an export holds that cannot be stored: records whose instants are outside what the stores represent.

use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
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

/// The instant, in Unix nanoseconds, a backend's expiry horizon `horizon` ends at `now`: a record older is one the
/// store drops once it is written ([`crate::staging::StagingService::expiry_horizon`]).
pub(crate) fn horizon_nanos(now: chrono::DateTime<chrono::Utc>, horizon: chrono::TimeDelta) -> u64 {
    (now - horizon)
        .timestamp_nanos_opt()
        .map_or(0, |nanos| u64::try_from(nanos).unwrap_or(0))
}

/// Spans that started before `cutoff`: the start the store keeps them by.
pub(crate) fn expired_spans(request: &ExportTraceServiceRequest, cutoff: u64) -> usize {
    request
        .resource_spans
        .iter()
        .flat_map(|resource| &resource.scope_spans)
        .flat_map(|scope| &scope.spans)
        .filter(|span| span.start_time_unix_nano < cutoff)
        .count()
}

/// Data points timestamped before `cutoff`.
pub(crate) fn expired_metrics(request: &ExportMetricsServiceRequest, cutoff: u64) -> usize {
    let mut expired = 0usize;
    for metric in request
        .resource_metrics
        .iter()
        .flat_map(|resource| &resource.scope_metrics)
        .flat_map(|scope| &scope.metrics)
    {
        macro_rules! points {
            ($data:expr) => {
                $data
                    .data_points
                    .iter()
                    .filter(|point| point.time_unix_nano < cutoff)
                    .count()
            };
        }
        expired += match metric.data.as_ref() {
            Some(metric::Data::Gauge(data)) => points!(data),
            Some(metric::Data::Sum(data)) => points!(data),
            Some(metric::Data::Histogram(data)) => points!(data),
            Some(metric::Data::ExponentialHistogram(data)) => points!(data),
            Some(metric::Data::Summary(data)) => points!(data),
            None => 0,
        };
    }
    expired
}

/// Log records whose own instant - their time, else their observed time - is before `cutoff`. A record with
/// neither is stored at its receipt, which is never expired.
pub(crate) fn expired_logs(request: &ExportLogsServiceRequest, cutoff: u64) -> usize {
    request
        .resource_logs
        .iter()
        .flat_map(|resource| &resource.scope_logs)
        .flat_map(|scope| &scope.log_records)
        .filter(|record| {
            let instant = if record.time_unix_nano != 0 {
                record.time_unix_nano
            } else {
                record.observed_time_unix_nano
            };
            instant != 0 && instant < cutoff
        })
        .count()
}
