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
        views_rebuild_from_raw: true,
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

/// A raw version past its time to live - every span it holds older than the 90 days ClickHouse keeps, no hold -
/// reads as absent on ClickHouse at once, before any merge deletes it: to the reconciler's read, a re-derivation's
/// page and survivor reconciliation alike, and an older version of the record does not take its place. DuckDB
/// has no time to live, so it keeps serving it; a live version reads the same on both.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_raw_version_past_its_time_to_live_reads_as_absent() {
    let Ok(url) = std::env::var(URL_ENV) else {
        eprintln!("clickhouse parity: skipped - set {URL_ENV} (or run `make test-clickhouse`)");
        return;
    };
    let database = "sideseat_parity_raw_ttl";
    let clickhouse = clickhouse_backend(&url, database).await;
    // Merges apply the TTL, and fold versions, at a time of their choosing; held off, both stay for the readers.
    raw_client(&url, database)
        .query("SYSTEM STOP MERGES otel_raw")
        .execute()
        .await
        .expect("stop merges");
    let (_duck_dir, duckdb) = duckdb_backend().await;
    let project = ProjectId::from(PROJECT);
    let long_ago = Utc::now() - chrono::Duration::days(200);
    let record =
        |raw_id: &str, trace: &str, signal_until, version| sideseat_ports::types::RawRecordRow {
            project_id: project.clone(),
            raw_id: raw_id.to_string(),
            signal: sideseat_ports::types::StagedSignal::Traces,
            received_at: Utc::now(),
            origin: sideseat_ports::types::RawOrigin::Received,
            version,
            signal_until,
            hold_until: None,
            trace_ids: vec![trace.to_string()],
            record: format!("{raw_id}-{version}").into_bytes(),
        };
    let rows = [
        record("expired", "trace-expired", long_ago, 1),
        record("live", "trace-live", Utc::now(), 1),
        sideseat_ports::types::RawRecordRow {
            hold_until: Some(Utc::now() + chrono::Duration::days(30)),
            ..record("held", "trace-held", long_ago, 1)
        },
    ];
    // A record whose older version is still within its time to live and whose latest is past it: as versions of
    // one record never are, since a rewrite keeps its record's `signal_until` - here to show the latest decides.
    let pair = [
        record("pair", "trace-pair", Utc::now(), 1),
        record("pair", "trace-pair", long_ago, 2),
    ];
    let spans: Vec<NormalizedSpan> = ["expired", "live", "held"]
        .iter()
        .map(|raw| NormalizedSpan {
            project_id: Some(PROJECT.to_string()),
            trace_id: format!("trace-{raw}"),
            span_id: format!("span-{raw}"),
            span_name: (*raw).to_string(),
            timestamp_start: Utc::now(),
            ingested_at: Some(Utc::now()),
            raw_id: Some((*raw).to_string()),
            ..Default::default()
        })
        .collect();
    let ids: Vec<String> = ["expired", "held", "live"].map(String::from).to_vec();
    let mut answers = Vec::new();
    for backend in [
        &clickhouse as &(dyn AnalyticsRepository + Send + Sync),
        &duckdb as &(dyn AnalyticsRepository + Send + Sync),
    ] {
        backend.insert_raw_records(&rows).await.expect("records");
        backend.append_raw_records(&pair[..1]).await.expect("older");
        backend
            .append_raw_records(&pair[1..])
            .await
            .expect("latest");
        backend.insert_spans(spans.clone()).await.expect("spans");
        let mut got: Vec<String> = backend
            .get_raw_records(&project, &[ids.clone(), vec!["pair".to_string()]].concat())
            .await
            .expect("get")
            .into_iter()
            .map(|r| r.raw_id)
            .collect();
        got.sort();
        let mut paged: Vec<String> = backend
            .raw_records_page(&project, None, 10)
            .await
            .expect("page")
            .into_iter()
            .map(|r| r.raw_id)
            .collect();
        paged.sort();
        let traces: Vec<String> = ["expired", "held", "live"]
            .map(|raw| format!("trace-{raw}"))
            .to_vec();
        let mut survivors: Vec<String> = backend
            .survivor_raw_records(&project, &traces)
            .await
            .expect("survivors")
            .into_iter()
            .map(|record| String::from_utf8(record).expect("utf-8"))
            .collect();
        survivors.sort();
        answers.push((got, paged, survivors));
    }
    let clickhouse_sees = vec!["held".to_string(), "live".to_string()];
    assert_eq!(
        answers[0],
        (
            clickhouse_sees.clone(),
            clickhouse_sees,
            vec!["held-1".to_string(), "live-1".to_string()]
        ),
        "ClickHouse"
    );
    let all = ["expired", "held", "live", "pair"]
        .map(String::from)
        .to_vec();
    assert_eq!(
        answers[1],
        (
            all.clone(),
            all,
            ["expired-1", "held-1", "live-1"].map(String::from).to_vec()
        ),
        "DuckDB, with no time to live, serves them all"
    );
}
