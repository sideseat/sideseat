// Retention and its time predicates, on both analytics backends: by age under a legal hold and after it, and by
// size under quota pressure. Span-count retention is part 31's.

/// One plain span of trace `[trace; 16]`, started `age` ago: no attachment, so retention's only work is the row.
fn export_aged(
    trace: u8,
    age: chrono::TimeDelta,
) -> opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest {
    use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
    use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue, any_value};
    use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, Span};

    let start = u64::try_from(
        (chrono::Utc::now() - age)
            .timestamp_nanos_opt()
            .expect("nanoseconds"),
    )
    .expect("after the epoch");
    ExportTraceServiceRequest {
        resource_spans: vec![ResourceSpans {
            scope_spans: vec![ScopeSpans {
                spans: vec![Span {
                    trace_id: vec![trace; 16],
                    span_id: vec![1; 8],
                    name: "generation".into(),
                    start_time_unix_nano: start,
                    end_time_unix_nano: start + 100,
                    attributes: vec![KeyValue {
                        key: "input.value".into(),
                        value: Some(AnyValue {
                            value: Some(any_value::Value::StringValue(format!("question {trace}"))),
                        }),
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        }],
    }
}

/// The traces of `ids` that still have a span, as their ids' first byte.
async fn traces_left(analytics: &(dyn AnalyticsRepository + Send + Sync), ids: &[u8]) -> Vec<u8> {
    let project = ProjectId::from("default");
    let mut left = Vec::new();
    for &id in ids {
        let spans = analytics
            .get_spans_for_trace(&project, &format!("{id:02x}").repeat(16), 10)
            .await
            .expect("spans");
        if !spans.is_empty() {
            left.push(id);
        }
    }
    left
}

/// Age retention against a row-level legal hold, as a transcript: what the hold measures, that an age pass leaves
/// the held rows, and that the pass after the hold has lapsed takes them.
async fn held_age_retention_answers(
    analytics: Arc<dyn AnalyticsRepository + Send + Sync>,
    retention: impl AsyncFn(
        Arc<dyn sideseat_ports::traits::TransactionalRepository + Send + Sync>,
        Arc<dyn sideseat_ports::traits::StorageGovernance + Send + Sync>,
    ),
) -> Vec<String> {
    let (
        FilePipeline {
            _temp,
            pipeline,
            transactional,
            files: _,
            blobs: _,
        },
        governance,
    ) = file_pipeline_governed(&analytics).await;
    let project = ProjectId::from("default");
    let traces = [0x61, 0x62];
    let exports: Vec<_> = traces
        .iter()
        .map(|&trace| export_aged(trace, chrono::TimeDelta::hours(2)))
        .collect();
    let mut answers = vec![format!(
        "ingested: {:?}",
        pipeline.run_batch_outcomes_for_test(&exports).await
    )];
    let total = analytics
        .project_logical_bytes(&project)
        .await
        .expect("bytes");

    // Held until tomorrow, on the rows themselves: the age pass's predicate is what keeps them.
    analytics
        .patch_project_hold(&project, chrono::Utc::now() + chrono::TimeDelta::days(1))
        .await
        .expect("hold");
    let held = analytics
        .project_held_logical_bytes(&project, chrono::Utc::now())
        .await
        .expect("held bytes");
    answers.push(format!(
        "held bytes are all of them: {}",
        held == total && total > 0
    ));
    // A server starting over this store measures every project's held bytes first, and refuses to start if it
    // cannot: this read is on the startup path.
    let startup = sideseat_domain::storage_governance::StorageGovernanceService::new(
        transactional.clone(),
        governance.clone(),
        analytics.clone(),
        Arc::new(sideseat_server::runtime::clock::SystemClock),
        u64::MAX,
    )
    .validate_startup()
    .await;
    answers.push(format!(
        "a server starts over it: {:?}",
        startup.map_err(|error| error.to_string())
    ));
    retention(transactional.clone(), governance.clone()).await;
    answers.push(format!(
        "left under the hold: {:?}",
        traces_left(analytics.as_ref(), &traces).await
    ));

    // Lapsed: a deadline already past protects nothing.
    analytics
        .patch_project_hold(&project, chrono::Utc::now() - chrono::TimeDelta::minutes(1))
        .await
        .expect("lapse");
    let held = analytics
        .project_held_logical_bytes(&project, chrono::Utc::now())
        .await
        .expect("held bytes");
    answers.push(format!("held bytes once lapsed: {held}"));
    retention(transactional, governance).await;
    answers.push(format!(
        "left once lapsed: {:?}",
        traces_left(analytics.as_ref(), &traces).await
    ));
    answers
}

/// A legal hold keeps rows an age pass would take, and its measure counts them - the measure a server takes of
/// every project before it starts - alike on both backends; once it lapses, the next pass takes them, alike. The
/// hold's deadline and the age cutoff are both instants compared with `DateTime64` columns, which ClickHouse must
/// be given as such.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn age_retention_and_its_hold_answer_alike() {
    let Ok(url) = std::env::var(URL_ENV) else {
        eprintln!("clickhouse parity: skipped - set {URL_ENV} (or run `make test-clickhouse`)");
        return;
    };
    let config = sideseat_core::config::RetentionConfig {
        max_age_minutes: Some(60),
        max_spans: None,
        ..Default::default()
    };

    let (_duck_temp, duckdb) = duckdb_backend().await;
    let duck_service = Arc::clone(&duckdb.0);
    let embedded = held_age_retention_answers(Arc::new(duckdb), async |database, governance| {
        duck_service
            .run_retention_to_completion(&config, u64::MAX, None, database, governance)
            .await
            .expect("DuckDB retention");
    })
    .await;

    let clickhouse = clickhouse_backend(&url, "sideseat_parity_held_age").await;
    let ch_service = Arc::clone(&clickhouse.0);
    let distributed =
        held_age_retention_answers(Arc::new(clickhouse), async |database, governance| {
            ch_service
                .run_retention_to_completion(&config, u64::MAX, None, database, governance)
                .await
                .expect("ClickHouse retention");
        })
        .await;

    assert_eq!(
        embedded,
        vec![
            "ingested: [Stored, Stored]".to_string(),
            "held bytes are all of them: true".to_string(),
            "a server starts over it: Ok(())".to_string(),
            "left under the hold: [97, 98]".to_string(),
            "held bytes once lapsed: 0".to_string(),
            "left once lapsed: []".to_string(),
        ]
    );
    assert_eq!(embedded, distributed, "DuckDB (reference) and ClickHouse");
}

/// Quota pressure, as a transcript: a project at its limit admits one byte more by evicting its oldest span, and
/// a held project evicts nothing and refuses.
async fn pressure_answers(analytics: Arc<dyn AnalyticsRepository + Send + Sync>) -> Vec<String> {
    let (
        FilePipeline {
            _temp,
            pipeline,
            transactional,
            files: _,
            blobs: _,
        },
        governance,
    ) = file_pipeline_governed(&analytics).await;
    let project = ProjectId::from("default");
    let clock: Arc<dyn sideseat_ports::clock::Clock> =
        Arc::new(sideseat_server::runtime::clock::SystemClock);
    // Oldest first: 0x71 three hours ago, 0x72 two, 0x73 one.
    let traces = [0x71, 0x72, 0x73];
    let exports: Vec<_> = traces
        .iter()
        .zip([3, 2, 1])
        .map(|(&trace, hours)| export_aged(trace, chrono::TimeDelta::hours(hours)))
        .collect();
    let mut answers = vec![format!(
        "ingested: {:?}",
        pipeline.run_batch_outcomes_for_test(&exports).await
    )];

    let service = |quota: u64| {
        sideseat_domain::storage_governance::StorageGovernanceService::new(
            transactional.clone(),
            governance.clone(),
            analytics.clone(),
            clock.clone(),
            quota,
        )
    };
    let measured = service(u64::MAX)
        .reconcile_project(&project)
        .await
        .expect("measure")
        .logical_bytes;
    let reserve = service(u64::MAX).maintenance_reserve_bytes();
    // At the limit exactly: one more byte is over it, and the oldest span is what makes room.
    let at_limit = service(reserve + measured);
    let admitted = at_limit.admit(&project, 1).await;
    answers.push(format!("one byte more admitted: {}", admitted.is_ok()));
    answers.push(format!(
        "left after pressure: {:?}",
        traces_left(analytics.as_ref(), &traces).await
    ));

    // Held rows are not reclaimable, so pressure finds nothing to take and the admission is refused.
    analytics
        .patch_project_hold(&project, chrono::Utc::now() + chrono::TimeDelta::days(1))
        .await
        .expect("hold");
    let measured = service(u64::MAX)
        .reconcile_project(&project)
        .await
        .expect("measure again")
        .logical_bytes;
    let held_limit = service(reserve + measured);
    let refused = held_limit.admit(&project, 1).await;
    answers.push(format!(
        "one byte more admitted under a hold: {}",
        refused.is_ok()
    ));
    answers.push(format!(
        "left under the hold: {:?}",
        traces_left(analytics.as_ref(), &traces).await
    ));
    answers
}

/// Size-based retention - eviction under quota pressure - takes the oldest span and spares a held one, alike: the
/// pressure batch's read compares the hold deadline with an instant, the comparison ClickHouse failed with a
/// decimal overflow while it was bound as bare microseconds.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pressure_eviction_takes_the_oldest_and_spares_a_hold_alike() {
    let Ok(url) = std::env::var(URL_ENV) else {
        eprintln!("clickhouse parity: skipped - set {URL_ENV} (or run `make test-clickhouse`)");
        return;
    };
    let (_duck_temp, duckdb) = duckdb_backend().await;
    let embedded = pressure_answers(Arc::new(duckdb)).await;
    let clickhouse = clickhouse_backend(&url, "sideseat_parity_pressure").await;
    let distributed = pressure_answers(Arc::new(clickhouse)).await;

    assert_eq!(
        embedded,
        vec![
            "ingested: [Stored, Stored, Stored]".to_string(),
            "one byte more admitted: true".to_string(),
            "left after pressure: [114, 115]".to_string(),
            "one byte more admitted under a hold: false".to_string(),
            "left under the hold: [114, 115]".to_string(),
        ]
    );
    assert_eq!(embedded, distributed, "DuckDB (reference) and ClickHouse");
}
