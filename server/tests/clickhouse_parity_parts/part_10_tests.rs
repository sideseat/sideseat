// Raw-record lifecycle parity.
//
// The reconciler (`sideseat_ingestion::traces::pipeline::raw_lifecycle`) is one piece of Rust over the
// `RawStore` port, so what can differ between the backends is the store beneath it: which version a read
// returns, whether a presented record is stored twice, which records a deletion enqueues, what a legal hold
// and a project deletion reach. Each of those is asked of both backends here and required to answer the same,
// which is what makes the reconciler's behaviour identical on either. The protocol it implements is
// `server/specs/RawRecordOwnership.tla`.

use sideseat_ports::types::{RawOrigin, RawRecordRow, StagedSignal};

fn raw_row(raw_id: &str, version: i64, traces: &[&str], body: &[u8]) -> RawRecordRow {
    RawRecordRow {
        project_id: ProjectId::from(PROJECT),
        raw_id: raw_id.to_string(),
        signal: StagedSignal::Traces,
        received_at: ts(100),
        origin: RawOrigin::Received,
        version,
        signal_until: ts(100),
        hold_until: None,
        trace_ids: traces.iter().map(|t| t.to_string()).collect(),
        record: body.to_vec(),
    }
}

fn raw_span(trace_id: &str, span_id: &str, raw_id: &str) -> NormalizedSpan {
    NormalizedSpan {
        project_id: Some(PROJECT.to_string()),
        trace_id: trace_id.to_string(),
        span_id: span_id.to_string(),
        span_name: format!("{trace_id}-{span_id}"),
        timestamp_start: ts(100),
        ingested_at: Some(ts(100)),
        raw_id: Some(raw_id.to_string()),
        ..Default::default()
    }
}

/// Everything the reconciler reads or writes, as one ordered list of answers, so a divergence names the step.
async fn raw_lifecycle_answers(store: &(impl AnalyticsRepository + ?Sized)) -> Vec<String> {
    let project = ProjectId::from(PROJECT);
    let mut answers = Vec::new();
    let say = |answers: &mut Vec<String>, step: &str, value: String| {
        answers.push(format!("{step}: {value}"));
    };

    // Presented twice, stored once; then two more versions, the second of which is the latest.
    let first = raw_row("raw-a", 1, &["trace-a", "trace-shared"], b"received-a");
    store
        .insert_raw_records(&[first.clone(), first.clone()])
        .await
        .expect("insert");
    store
        .insert_raw_records(std::slice::from_ref(&first))
        .await
        .expect("insert again");
    store
        .insert_raw_records(&[raw_row("raw-b", 1, &["trace-b"], b"received-b")])
        .await
        .expect("insert b");
    store
        .append_raw_records(&[
            RawRecordRow {
                origin: RawOrigin::Deleted,
                version: 3,
                record: b"rewritten-a".to_vec(),
                trace_ids: Vec::new(),
                ..first.clone()
            },
            RawRecordRow {
                origin: RawOrigin::Fenced,
                version: 2,
                record: b"fenced-a".to_vec(),
                trace_ids: Vec::new(),
                ..first.clone()
            },
        ])
        .await
        .expect("append versions");
    let latest = store
        .get_raw_records(&project, &["raw-a".to_string(), "raw-b".to_string()])
        .await
        .expect("read back");
    say(
        &mut answers,
        "latest",
        latest
            .iter()
            .map(|row| {
                format!(
                    "{}/{}/{}/{}",
                    row.raw_id,
                    row.version,
                    row.origin.as_str(),
                    String::from_utf8_lossy(&row.record)
                )
            })
            .collect::<Vec<_>>()
            .join(","),
    );
    say(
        &mut answers,
        "page",
        store
            .raw_records_page(&project, None, 10)
            .await
            .expect("page")
            .iter()
            .map(|row| format!("{}/{}", row.raw_id, row.version))
            .collect::<Vec<_>>()
            .join(","),
    );

    // Only one record has rows, so only one is named; both are still reachable by a deletion.
    store
        .insert_spans(vec![raw_span("trace-a", "span-a", "raw-a")])
        .await
        .expect("insert spans");
    let mut named = store
        .raw_records_named(&project, &["raw-a".to_string(), "raw-b".to_string()])
        .await
        .expect("named")
        .into_iter()
        .collect::<Vec<_>>();
    named.sort();
    say(&mut answers, "named", named.join(","));

    // A deletion of a trace whose record has no rows must still enqueue it: the trace index, not the rows,
    // is what a deletion looks at.
    store
        .delete_traces(&project, &["trace-b".to_string()])
        .await
        .expect("delete a trace with no rows");
    store
        .delete_spans(&project, &[("trace-a".to_string(), "span-a".to_string())])
        .await
        .expect("delete a span");
    let mut queued = store
        .pending_raw_records(usize::MAX)
        .await
        .expect("queue")
        .into_iter()
        .map(|entry| entry.raw_id)
        .collect::<Vec<_>>();
    queued.sort();
    queued.dedup();
    say(&mut answers, "queued", queued.join(","));

    // Clearing takes exactly the entries read: one enqueued afterwards stays.
    let entries = store.pending_raw_records(usize::MAX).await.expect("queue");
    store
        .enqueue_raw_records(&project, &["raw-a".to_string()])
        .await
        .expect("enqueue again");
    store
        .clear_raw_pending(&entries)
        .await
        .expect("clear the entries read");
    say(
        &mut answers,
        "left queued",
        store
            .pending_raw_records(usize::MAX)
            .await
            .expect("queue")
            .len()
            .to_string(),
    );

    // The hold reaches the records.
    store
        .patch_project_hold(&project, ts(9_000))
        .await
        .expect("hold");
    say(
        &mut answers,
        "held",
        store
            .get_raw_records(&project, &["raw-a".to_string()])
            .await
            .expect("held")
            .iter()
            .map(|row| row.hold_until.is_some().to_string())
            .collect::<Vec<_>>()
            .join(","),
    );

    // A record deleted by the reconciler goes entirely, index included - so a later deletion of the same
    // trace enqueues nothing.
    store
        .delete_raw_records(&project, &["raw-b".to_string()])
        .await
        .expect("delete a record");
    store
        .clear_raw_pending(&store.pending_raw_records(usize::MAX).await.expect("queue"))
        .await
        .expect("clear");
    store
        .delete_traces(&project, &["trace-b".to_string()])
        .await
        .expect("delete the trace again");
    say(
        &mut answers,
        "after collection",
        format!(
            "{}/{}",
            store
                .get_raw_records(&project, &["raw-b".to_string()])
                .await
                .expect("gone")
                .len(),
            store
                .pending_raw_records(usize::MAX)
                .await
                .expect("queue")
                .len()
        ),
    );

    // The project deletion takes the records, the index and the queue with it.
    store
        .delete_project_data(&project)
        .await
        .expect("delete the project");
    say(
        &mut answers,
        "after the project",
        format!(
            "{}/{}",
            store
                .raw_records_page(&project, None, 10)
                .await
                .expect("records")
                .len(),
            store
                .pending_raw_records(usize::MAX)
                .await
                .expect("queue")
                .len()
        ),
    );
    answers
}

/// The store the reconciler stands on answers the same on both backends, step by step.
#[tokio::test]
async fn raw_record_lifecycle_matches_across_backends() {
    let Ok(url) = std::env::var(URL_ENV) else {
        eprintln!("clickhouse parity: skipped - set {URL_ENV} (or run `make test-clickhouse`)");
        return;
    };
    let (_temp, duckdb) = duckdb_backend().await;
    let clickhouse = clickhouse_backend(&url, "sideseat_parity_raw_lifecycle").await;

    let embedded = raw_lifecycle_answers(&duckdb).await;
    let distributed = raw_lifecycle_answers(&clickhouse).await;
    assert_eq!(
        embedded, distributed,
        "the raw-record lifecycle differs between the backends"
    );
    assert!(
        embedded.iter().any(|answer| answer.contains("rewritten-a")),
        "the latest version must be the rewrite: {embedded:?}"
    );
}
