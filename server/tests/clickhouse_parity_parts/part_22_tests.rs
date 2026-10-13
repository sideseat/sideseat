/// A log's media is named where a release looks, on both backends alike, and ClickHouse's age pass records a
/// media-bearing log's trace for release before it deletes - as it records the span rows' traces.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_log_s_media_is_seen_by_the_release_and_recorded_by_the_age_pass() {
    let Ok(url) = std::env::var(URL_ENV) else {
        eprintln!("clickhouse parity: skipped - set {URL_ENV} (or run `make test-clickhouse`)");
        return;
    };
    let clickhouse = clickhouse_backend(&url, "sideseat_parity_log_media").await;
    let (_duck_dir, duckdb) = duckdb_backend().await;
    let project = ProjectId::from(PROJECT);
    let uri = format!("#!B64!#image/png::{}", "e".repeat(64));
    let log = sideseat_ports::types::NormalizedLog {
        project_id: Some(PROJECT.to_string()),
        log_digest: "f".repeat(64),
        trace_id: Some("trace-media".to_string()),
        span_id: Some("span-media".to_string()),
        timestamp: Utc::now() - chrono::Duration::days(10),
        ingested_at: Some(Utc::now()),
        raw_log: serde_json::json!({"body": {"bytesValue": uri.clone()}}),
        media: vec![uri.clone()],
        ..Default::default()
    };
    let plain = sideseat_ports::types::NormalizedLog {
        log_digest: "0".repeat(64),
        trace_id: Some("trace-plain".to_string()),
        raw_log: serde_json::json!({"body": {"stringValue": "no media"}}),
        media: Vec::new(),
        ..log.clone()
    };
    for backend in [
        &clickhouse as &(dyn AnalyticsRepository + Send + Sync),
        &duckdb as &(dyn AnalyticsRepository + Send + Sync),
    ] {
        backend
            .insert_logs(&[log.clone(), plain.clone()])
            .await
            .expect("insert the logs");
    }
    let named = |fields: Vec<String>| {
        let mut uris = Vec::new();
        for field in &fields {
            sideseat_domain::files::collect_file_references_in_str(field, &mut uris);
        }
        uris.sort_unstable();
        uris.dedup();
        uris
    };
    let trace = ["trace-media".to_string()];
    let on_clickhouse = named(
        clickhouse
            .file_reference_fields_for_traces(&project, &trace)
            .await
            .expect("clickhouse references"),
    );
    let on_duckdb = named(
        duckdb
            .file_reference_fields_for_traces(&project, &trace)
            .await
            .expect("duckdb references"),
    );
    assert_eq!(on_clickhouse, vec![uri.clone()]);
    assert_eq!(
        on_clickhouse, on_duckdb,
        "the backends name different media"
    );

    // A producer's verified reference in a body the raw record rebuilds, so not stored: the raw record holds it
    // escaped, and the release still sees it on both backends - while the same spelling as a key stays text.
    let verified = format!("#!B64!#image/png::{}", "d".repeat(64));
    let rebuilt = sideseat_ports::types::NormalizedLog {
        log_digest: "2".repeat(64),
        trace_id: Some("trace-verified".to_string()),
        body: serde_json::json!(format!("see {verified}")),
        body_text: Some(format!("see {verified}")),
        raw_log: serde_json::json!({"body": {"kvlistValue": {"values": [
            {"key": "#!!B64!#image/png::".to_string() + &"c".repeat(64), "value": {"stringValue":
                format!("see {}", verified.replacen("#!", "#!!", 1))}}
        ]}}}),
        media: vec![verified.clone()],
        body_rebuilds_from_raw: true,
        ..log.clone()
    };
    for backend in [
        &clickhouse as &(dyn AnalyticsRepository + Send + Sync),
        &duckdb as &(dyn AnalyticsRepository + Send + Sync),
    ] {
        backend
            .insert_logs(std::slice::from_ref(&rebuilt))
            .await
            .expect("insert the rebuilt log");
        let fields = backend
            .file_reference_fields_for_traces(&project, &["trace-verified".to_string()])
            .await
            .expect("references");
        assert_eq!(
            named(fields),
            vec![verified.clone()],
            "the verified reference is unseen"
        );
    }

    // A key spelled like a reference is the producer's text on both backends: the release never counts it.
    let keyed = sideseat_ports::types::NormalizedLog {
        log_digest: "1".repeat(64),
        trace_id: Some("trace-key".to_string()),
        body: serde_json::json!({ uri.clone(): "text" }),
        raw_log: serde_json::json!({"body": {"stringValue": "escaped"}}),
        media: Vec::new(),
        ..log.clone()
    };
    for backend in [
        &clickhouse as &(dyn AnalyticsRepository + Send + Sync),
        &duckdb as &(dyn AnalyticsRepository + Send + Sync),
    ] {
        backend
            .insert_logs(std::slice::from_ref(&keyed))
            .await
            .expect("insert the keyed log");
        let fields = backend
            .file_reference_fields_for_traces(&project, &["trace-key".to_string()])
            .await
            .expect("references");
        assert!(named(fields).is_empty(), "a key was read as a reference");
    }

    let temp = tempfile::TempDir::new().expect("temp dir");
    let sqlite = retention_transactional(&temp).await;
    let recorded = clickhouse
        .record_age_expiry(
            &project,
            Utc::now() - chrono::Duration::days(1),
            Utc::now(),
            sqlite.as_ref(),
        )
        .await
        .expect("record the expiring traces");
    assert_eq!(
        recorded, 4,
        "every expiring traced log's trace owes a release: a redelivery may have dropped a reference the \
         association still holds"
    );
}
