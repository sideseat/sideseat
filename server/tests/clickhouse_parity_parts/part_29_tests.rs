/// Bytes as lowercase hex, as the server spelled ids.
fn lower_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// What the API answered about a data point's exemplars before they became one list, written the way the server
/// wrote them then: the first storable exemplar's trace and span in hex and its instant at microseconds, and every
/// storable exemplar as the JSON array. The oracle the list's answers are held to.
fn exemplar_answers_as_stored_before(
    exemplars: &[opentelemetry_proto::tonic::metrics::v1::Exemplar],
) -> (
    Option<String>,
    Option<String>,
    Option<DateTime<Utc>>,
    Option<String>,
) {
    use opentelemetry_proto::tonic::metrics::v1::exemplar::Value;
    use sideseat_core::utils::time::{is_storable, nanos_to_datetime};
    let storable: Vec<_> = exemplars
        .iter()
        .filter(|e| e.time_unix_nano == 0 || is_storable(nanos_to_datetime(e.time_unix_nano)))
        .filter(|e| !matches!(&e.value, Some(Value::AsDouble(d)) if !d.is_finite()))
        .collect();
    let trace = |e: &opentelemetry_proto::tonic::metrics::v1::Exemplar| {
        Some(lower_hex(&e.trace_id))
            .filter(|s| !s.is_empty() && s != "00000000000000000000000000000000")
    };
    let span = |e: &opentelemetry_proto::tonic::metrics::v1::Exemplar| {
        Some(lower_hex(&e.span_id)).filter(|s| !s.is_empty() && s != "0000000000000000")
    };
    let first = storable.first();
    let array = (!storable.is_empty()).then(|| {
        let entries: Vec<serde_json::Value> = storable
            .iter()
            .map(|e| {
                let mut entry = serde_json::Map::new();
                if let Some(trace_id) = trace(e) {
                    entry.insert("trace_id".to_string(), serde_json::Value::String(trace_id));
                }
                if let Some(span_id) = span(e) {
                    entry.insert("span_id".to_string(), serde_json::Value::String(span_id));
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
                        serde_json::Value::String(nanos_to_datetime(e.time_unix_nano).to_rfc3339()),
                    );
                }
                if !e.filtered_attributes.is_empty() {
                    entry.insert(
                        "attributes".to_string(),
                        sideseat_ingestion::otlp::attrs_to_typed_json(&e.filtered_attributes),
                    );
                }
                serde_json::Value::Object(entry)
            })
            .collect();
        serde_json::Value::Array(entries).to_string()
    });
    (
        first.and_then(|e| trace(e)),
        first.and_then(|e| span(e)),
        first.filter(|e| e.time_unix_nano > 0).and_then(|e| {
            DateTime::from_timestamp_micros(nanos_to_datetime(e.time_unix_nano).timestamp_micros())
        }),
        array,
    )
}

/// Every captured metric export's data points with their exemplars as the protobuf carried them, in the order the
/// extraction returns the points, and the points themselves, stamped for one project.
fn captured_metric_points() -> Vec<(
    NormalizedMetric,
    Vec<opentelemetry_proto::tonic::metrics::v1::Exemplar>,
)> {
    use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
    use opentelemetry_proto::tonic::metrics::v1::metric::Data;
    use prost::Message;
    fn walk(dir: &std::path::Path, found: &mut Vec<std::path::PathBuf>) {
        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .expect("fixture directory")
            .map(|entry| entry.expect("entry").path())
            .collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                walk(&path, found);
            } else if path.extension().is_some_and(|extension| extension == "pb") {
                found.push(path);
            }
        }
    }
    let mut files = Vec::new();
    walk(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/metrics"),
        &mut files,
    );
    let mut points = Vec::new();
    for (position, path) in files.iter().enumerate() {
        let request =
            ExportMetricsServiceRequest::decode(std::fs::read(path).expect("export").as_slice())
                .expect("a metric export");
        let mut sent = Vec::new();
        for rm in &request.resource_metrics {
            for sm in &rm.scope_metrics {
                for metric in &sm.metrics {
                    match &metric.data {
                        Some(Data::Gauge(data)) => {
                            sent.extend(data.data_points.iter().map(|p| p.exemplars.clone()))
                        }
                        Some(Data::Sum(data)) => {
                            sent.extend(data.data_points.iter().map(|p| p.exemplars.clone()))
                        }
                        Some(Data::Histogram(data)) => {
                            sent.extend(data.data_points.iter().map(|p| p.exemplars.clone()))
                        }
                        Some(Data::ExponentialHistogram(data)) => {
                            sent.extend(data.data_points.iter().map(|p| p.exemplars.clone()));
                        }
                        Some(Data::Summary(data)) => {
                            sent.extend(data.data_points.iter().map(|_| Vec::new()))
                        }
                        None => {}
                    }
                }
            }
        }
        let extracted = sideseat_ingestion::metrics::extract_metrics_batch(&request);
        assert_eq!(
            extracted.len(),
            sent.len(),
            "{} extracts one point per point sent",
            path.display()
        );
        for (mut point, exemplars) in extracted.into_iter().zip(sent) {
            point.project_id = Some(PROJECT.to_string());
            point.ingested_at = Some(ts(i64::try_from(position).expect("a file position")));
            points.push((point, exemplars));
        }
    }
    points
}

/// Every captured exemplar is answered by both backends exactly as the server answered it before exemplars became
/// one list per point - the first exemplar's trace, span and instant, and the whole array - and the
/// trace-correlation filter still matches the first exemplar's trace and only it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn captured_exemplars_are_answered_as_before_on_both_backends() {
    let Ok(url) = std::env::var(URL_ENV) else {
        eprintln!("clickhouse parity: skipped - set {URL_ENV} (or run `make test-clickhouse`)");
        return;
    };
    let (_duck_temp, duck) = duckdb_backend().await;
    let ch = clickhouse_backend(&url, "sideseat_parity_exemplar_list").await;
    let points = captured_metric_points();
    // The last delivery of a datapoint is the one stored.
    let mut expected: std::collections::BTreeMap<String, _> = std::collections::BTreeMap::new();
    for (point, exemplars) in &points {
        expected.insert(
            point.datapoint_id.clone(),
            exemplar_answers_as_stored_before(exemplars),
        );
    }
    assert!(
        expected
            .values()
            .filter(|answers| answers.3.is_some())
            .count()
            > 10,
        "the corpus carries exemplars to compare"
    );
    let stored: Vec<NormalizedMetric> = points.into_iter().map(|(point, _)| point).collect();
    let project = ProjectId::from(PROJECT);
    for repo in [&duck as &dyn AnalyticsRepository, &ch] {
        for chunk in stored.chunks(200) {
            repo.insert_metrics(chunk).await.expect("metrics");
        }
        for (datapoint_id, answers) in &expected {
            let row = repo
                .get_metric(&project, datapoint_id)
                .await
                .expect("get metric")
                .expect("metric exists");
            assert_eq!(
                (
                    row.exemplar_trace_id.clone(),
                    row.exemplar_span_id.clone(),
                    row.exemplar_timestamp,
                    row.exemplars.clone(),
                ),
                answers.clone(),
                "datapoint {datapoint_id}"
            );
        }
        // The filter matches a point by its first exemplar's trace, and a later exemplar's trace finds nothing.
        let (first_owner, (first_trace, ..)) = expected
            .iter()
            .find(|(_, answers)| answers.0.is_some())
            .expect("a point whose first exemplar names a trace");
        let params = sideseat_ports::types::ListMetricsParams {
            project_id: project.clone(),
            page: 1,
            limit: 100,
            exemplar_trace_id: first_trace.clone(),
            ..Default::default()
        };
        let (found, _) = repo.list_metrics(&params).await.expect("filtered list");
        assert!(found.iter().any(|row| &row.datapoint_id == first_owner));
        let later_trace = expected.values().find_map(|answers| {
            let array: serde_json::Value = serde_json::from_str(answers.3.as_deref()?).ok()?;
            let later = array
                .as_array()?
                .get(1)?
                .get("trace_id")?
                .as_str()?
                .to_string();
            (Some(&later) != answers.0.as_ref()).then_some(later)
        });
        if let Some(later_trace) = later_trace {
            let only_later = expected
                .values()
                .all(|answers| answers.0.as_deref() != Some(later_trace.as_str()));
            let params = sideseat_ports::types::ListMetricsParams {
                exemplar_trace_id: Some(later_trace),
                ..params
            };
            let (found, _) = repo.list_metrics(&params).await.expect("filtered list");
            if only_later {
                assert!(found.is_empty(), "a later exemplar's trace is not matched");
            }
        }
    }
}
