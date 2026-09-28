
/// A span re-delivered *during* a traversal is still visible, in both backends, at its earlier version.
///
/// This is the case a watermark exists for, and the one both backends got wrong in opposite ways. The
/// deduplication picks the newest row of a span; a watermark applied *outside* it then rejects that choice
/// without promoting the row the traversal should have seen, so the span disappeared from every page - the
/// new version filtered out, the old one never selected. DuckDB's message queries had been fixed;
/// `get_feed_spans` there and *all three* watermarked queries on ClickHouse had not, which after the DuckDB
/// fix would have made the two backends answer differently about the same table.
///
/// Written as its own test because it needs control of `ingested_at`, which the shared fixture does not vary.
#[tokio::test]
async fn a_span_redelivered_during_a_traversal_stays_visible_in_both_backends() {
    let Ok(url) = std::env::var(URL_ENV) else {
        eprintln!("clickhouse parity: skipped - set {URL_ENV} (or run `make test-clickhouse`)");
        return;
    };

    let (_temp, duck) = duckdb_backend().await;
    let ch = clickhouse_backend(&url, "sideseat_parity_watermark").await;

    // Ingested *before* the traversal begins.
    let original = NormalizedSpan {
        project_id: Some(PROJECT.to_string()),
        trace_id: "trace-w".to_string(),
        span_id: "span-w".to_string(),
        span_name: "generation".to_string(),
        observation_type: Some(ObservationType::Generation),
        timestamp_start: ts(0),
        timestamp_end: Some(ts(1)),
        duration_ms: 1000,
        ingested_at: Some(ts(10)),
        gen_ai_usage_input_tokens: 100,
        ..Default::default()
    };
    // The traversal's watermark sits between the two deliveries.
    let watermark_us = ts(20).timestamp_micros();
    // The same span, re-delivered after the traversal started.
    let redelivered = NormalizedSpan {
        ingested_at: Some(ts(30)),
        gen_ai_usage_input_tokens: 900,
        ..original.clone()
    };

    for backend_spans in [vec![original.clone()], vec![redelivered.clone()]] {
        duck.insert_spans(backend_spans.clone())
            .await
            .expect("duckdb insert");
        ch.insert_spans(backend_spans)
            .await
            .expect("clickhouse insert");
    }

    let params = FeedSpansParams {
        project_id: ProjectId::from(PROJECT),
        limit: 50,
        ingested_before_us: Some(watermark_us),
        ..Default::default()
    };
    let d = duck
        .get_feed_spans(&params)
        .await
        .expect("duckdb feed spans");
    let c = ch
        .get_feed_spans(&params)
        .await
        .expect("clickhouse feed spans");

    assert_eq!(
        d.len(),
        1,
        "the span must be visible at its pre-traversal version, not filtered out of existence"
    );
    assert_eq!(
        d.len(),
        c.len(),
        "the two backends disagree about whether a re-delivered span exists as of the watermark"
    );
    assert_eq!(
        d[0].gen_ai_usage_input_tokens, 100,
        "the traversal must see the version that existed when it began"
    );
    assert_eq!(
        d[0].gen_ai_usage_input_tokens, c[0].gen_ai_usage_input_tokens,
        "the two backends chose different versions of the same span"
    );

    // The same question through the message path, which the reconstruction reads.
    let msg_params = MessageQueryParams {
        project_id: ProjectId::from(PROJECT),
        trace_ids: Some(vec!["trace-w".to_string()]),
        ingested_before_us: Some(watermark_us),
        ..Default::default()
    };
    let dm = duck
        .get_messages(&msg_params)
        .await
        .expect("duckdb messages");
    let cm = ch.get_messages(&msg_params).await.expect("ch messages");
    assert_eq!(
        dm.rows.len(),
        cm.rows.len(),
        "the context load disagrees between backends as of the watermark"
    );
}

/// Two deliveries of one span in the same stored microsecond yield exactly one row on both backends.
///
/// The `MAX(ingested_at)` join DuckDB's dedup used returned *every* row tied at the maximum, so a span
/// re-delivered within the same microsecond appeared twice - a duplicate, the one thing the feed must never
/// emit - while ClickHouse's `FINAL`/`LIMIT 1 BY` returned one. `QUALIFY ROW_NUMBER() … = 1` makes DuckDB
/// pick exactly one too, so the backends agree on a tie.
#[tokio::test]
async fn a_same_microsecond_redelivery_is_one_row_on_both_backends() {
    let Ok(url) = std::env::var(URL_ENV) else {
        eprintln!("clickhouse parity: skipped - set {URL_ENV} (or run `make test-clickhouse`)");
        return;
    };

    let (_temp, duck) = duckdb_backend().await;
    let ch = clickhouse_backend(&url, "sideseat_parity_tie").await;

    // Same identity and the *same* ingested_at, different token counts: the tie.
    let ingested = ts(10);
    let base = NormalizedSpan {
        project_id: Some(PROJECT.to_string()),
        trace_id: "trace-tie".to_string(),
        span_id: "span-tie".to_string(),
        span_name: "generation".to_string(),
        observation_type: Some(ObservationType::Generation),
        timestamp_start: ts(0),
        timestamp_end: Some(ts(1)),
        duration_ms: 1000,
        ingested_at: Some(ingested),
        ..Default::default()
    };
    let v1 = NormalizedSpan {
        gen_ai_usage_input_tokens: 100,
        ..base.clone()
    };
    let v2 = NormalizedSpan {
        gen_ai_usage_input_tokens: 900,
        ..base
    };
    for backend_spans in [vec![v1], vec![v2]] {
        duck.insert_spans(backend_spans.clone())
            .await
            .expect("duckdb insert");
        ch.insert_spans(backend_spans)
            .await
            .expect("clickhouse insert");
    }

    let params = FeedSpansParams {
        project_id: ProjectId::from(PROJECT),
        limit: 50,
        ..Default::default()
    };
    let d = duck
        .get_feed_spans(&params)
        .await
        .expect("duckdb feed spans");
    let c = ch
        .get_feed_spans(&params)
        .await
        .expect("clickhouse feed spans");
    assert_eq!(
        d.len(),
        1,
        "a same-microsecond re-delivery must not duplicate the span on DuckDB"
    );
    assert_eq!(
        d.len(),
        c.len(),
        "the two backends disagree on how many rows a same-microsecond tie yields"
    );
    // Latest delivery wins on DuckDB: v2 (900 tokens) was inserted after v1 (100), so its higher rowid
    // breaks the tie. Without the rowid tiebreak the engine could keep v1, silently ignoring the correction.
    assert_eq!(
        d[0].gen_ai_usage_input_tokens, 900,
        "the later same-microsecond delivery must win on DuckDB, not the earlier one"
    );
}

/// A corrected re-delivery whose `timestamp_start` **crosses midnight UTC** is still one span.
///
/// This is the defect the v3 sorting key exists to fix. `ReplacingMergeTree` identifies duplicates *by the
/// sorting key*, and that key contained `toDate(timestamp_start)` - so a producer re-sending a span with a
/// corrected start time on the other side of midnight produced a row with a **different** key, and `FINAL`
/// returned both revisions. A duplicate span is the one thing the feed must never produce, and it was
/// invisible to every other test here because a re-delivery normally carries an identical timestamp.
///
/// Reproduced against 25.8 before the fix: two rows. The month-boundary case is *not* covered, and
/// deliberately - see the residual in the plan's step 0. `do_not_merge_across_partitions_select_final`
/// keeps `FINAL` per-partition, and turning it off costs 10-12x on the trace lookup and trace-list page
/// (measured), so that case is reported rather than fixed here.
#[tokio::test]
async fn a_correction_crossing_midnight_utc_is_one_span_on_both_backends() {
    let Ok(url) = std::env::var(URL_ENV) else {
        eprintln!("clickhouse parity: skipped - set {URL_ENV} (or run `make test-clickhouse`)");
        return;
    };

    let (_temp, duck) = duckdb_backend().await;
    let ch = clickhouse_backend(&url, "sideseat_parity_midnight").await;

    // Two instants either side of a UTC midnight, in the **current** month: same partition, so the
    // partition boundary is not what is being tested, and recent enough to sit inside the table's 90-day
    // TTL. A fixed past date is what a first draft used, and it made the test pass for the wrong reason -
    // both rows were already TTL-expired, one had been merged away, and a single row came back whatever
    // the sorting key was. The mutation check is what exposed that.
    let now = Utc::now();
    let before = Utc
        .with_ymd_and_hms(now.year(), now.month(), 10, 23, 30, 0)
        .unwrap();
    let after = Utc
        .with_ymd_and_hms(now.year(), now.month(), 11, 0, 30, 0)
        .unwrap();
    let base = NormalizedSpan {
        project_id: Some(PROJECT.to_string()),
        trace_id: "trace-midnight".to_string(),
        span_id: "span-midnight".to_string(),
        span_name: "generation".to_string(),
        observation_type: Some(ObservationType::Generation),
        duration_ms: 1000,
        ..Default::default()
    };
    let first = NormalizedSpan {
        timestamp_start: before,
        timestamp_end: Some(before),
        ingested_at: Some(ts(10)),
        gen_ai_usage_input_tokens: 100,
        ..base.clone()
    };
    // The correction moves the start time across midnight *and* fixes the token count.
    let corrected = NormalizedSpan {
        timestamp_start: after,
        timestamp_end: Some(after),
        ingested_at: Some(ts(20)),
        gen_ai_usage_input_tokens: 900,
        ..base
    };
    for spans in [vec![first], vec![corrected]] {
        duck.insert_spans(spans.clone())
            .await
            .expect("duckdb insert");
        ch.insert_spans(spans).await.expect("clickhouse insert");
    }

    let params = FeedSpansParams {
        project_id: ProjectId::from(PROJECT),
        limit: 50,
        ..Default::default()
    };
    let d = duck
        .get_feed_spans(&params)
        .await
        .expect("duckdb feed spans");
    let c = ch
        .get_feed_spans(&params)
        .await
        .expect("clickhouse feed spans");

    assert_eq!(
        c.len(),
        1,
        "a correction crossing midnight UTC must be one span on ClickHouse - with `toDate` in the sorting \
         key it was two, because the two revisions had different keys"
    );
    assert_eq!(
        d.len(),
        c.len(),
        "the two backends disagree on how many rows a midnight-crossing correction yields"
    );
    assert_eq!(
        c[0].gen_ai_usage_input_tokens, 900,
        "the correction must win on ClickHouse, not the revision it replaced"
    );
    assert_eq!(
        d[0].gen_ai_usage_input_tokens, c[0].gen_ai_usage_input_tokens,
        "the two backends disagree about which revision survived"
    );
}

/// A correction crossing a **month** boundary is returned twice, and the consistency check must say so.
///
/// This is the residual schema v3 leaves and cannot close. Taking `toDate(timestamp_start)` out of the span
/// sorting key collapses the *midnight*-crossing duplicate, which is the common case; but partitioning stays
/// monthly, parts in different partitions never merge, and `do_not_merge_across_partitions_select_final`
/// makes `FINAL` per-partition — so two revisions in different partitions both survive.
///
/// Two assertions, and the first is the uncomfortable one. It **pins the defect**: ClickHouse really does
/// return two rows here where DuckDB returns one. Asserting parity instead would fail, and "fixing" it would
/// mean either the 10-12x read regression of turning the setting off or a read-time version selection with no
/// stable tie-break available. So the answer is that the check *reports* it — which is only worth anything if
/// the report actually fires, which is the second assertion.
///
/// A test asserting only that the two backends agree everywhere else would pass while this residual was
/// silently wrong, which is exactly the shape of gate this repository keeps getting caught by.
#[tokio::test]
async fn a_correction_crossing_a_month_boundary_is_reported_by_the_consistency_check() {
    let Ok(url) = std::env::var(URL_ENV) else {
        eprintln!("clickhouse parity: skipped - set {URL_ENV} (or run `make test-clickhouse`)");
        return;
    };

    let (_temp, duck) = duckdb_backend().await;
    let ch = clickhouse_backend(&url, "sideseat_parity_crossmonth").await;

    // Two instants in **different months**, both recent enough to sit inside the 90-day TTL - so an expiry
    // cannot be what makes a row disappear, which is the trap that made the midnight test pass for the wrong
    // reason. Anchored to the current month and the one before it.
    let now = Utc::now();
    let this_month = Utc
        .with_ymd_and_hms(now.year(), now.month(), 5, 12, 0, 0)
        .unwrap();
    let last_month = this_month - chrono::Duration::days(20);
    assert_ne!(
        this_month.month(),
        last_month.month(),
        "the two instants must be in different months or the test proves nothing"
    );

    let base = NormalizedSpan {
        project_id: Some(PROJECT.to_string()),
        trace_id: "trace-crossmonth".to_string(),
        span_id: "span-crossmonth".to_string(),
        span_name: "generation".to_string(),
        observation_type: Some(ObservationType::Generation),
        duration_ms: 1000,
        ..Default::default()
    };
    let first = NormalizedSpan {
        timestamp_start: last_month,
        timestamp_end: Some(last_month),
        ingested_at: Some(ts(10)),
        gen_ai_usage_input_tokens: 100,
        ..base.clone()
    };
    let corrected = NormalizedSpan {
        timestamp_start: this_month,
        timestamp_end: Some(this_month),
        ingested_at: Some(ts(20)),
        gen_ai_usage_input_tokens: 900,
        ..base
    };
    for spans in [vec![first], vec![corrected]] {
        duck.insert_spans(spans.clone())
            .await
            .expect("duckdb insert");
        ch.insert_spans(spans).await.expect("clickhouse insert");
    }

    let params = FeedSpansParams {
        project_id: ProjectId::from(PROJECT),
        limit: 50,
        ..Default::default()
    };
    let d = duck.get_feed_spans(&params).await.expect("duckdb read");
    let c = ch.get_feed_spans(&params).await.expect("clickhouse read");

    // The residual, pinned rather than wished away.
    assert_eq!(
        d.len(),
        1,
        "DuckDB deduplicates by identity regardless of layout"
    );
    assert_eq!(
        c.len(),
        2,
        "the stated cross-month residual: both revisions are visible on ClickHouse. If this is ever 1 the \
         residual has been closed and this test plus the comments describing it are out of date"
    );

    // And the check has to report it, or the residual is silent rather than stated.
    let outcome = ch
        .check_partition_consistency()
        .await
        .expect("the consistency check runs");
    assert_eq!(
        outcome.anomalies, 1,
        "the check must report the identity whose revisions span two partitions, got {outcome:?}"
    );

    let recorded = ch
        .partition_anomalies()
        .await
        .expect("the anomaly record is readable");
    assert_eq!(recorded.len(), 1, "one durable record, got {recorded:?}");
    assert_eq!(recorded[0].span_id, "span-crossmonth");
    assert_eq!(
        recorded[0].revisions, 2,
        "both physical revisions are counted"
    );
    assert_eq!(
        recorded[0].partitions.len(),
        2,
        "the record names both partitions, got {:?}",
        recorded[0].partitions
    );

    // The record is *durable*, which is the whole point: the state that produced the finding can disappear
    // (a backward move expires the newer revision first, leaving the obsolete one alone and a current-state
    // query reporting clean) while the damage persists. A second pass must not lose it, and must not
    // re-report it as new either - the table is keyed by identity, so re-detection updates one row.
    let second = ch
        .check_partition_consistency()
        .await
        .expect("a second pass runs");
    assert_eq!(
        ch.partition_anomalies()
            .await
            .expect("still readable")
            .len(),
        1,
        "the record survives a second pass without being duplicated (second pass: {second:?})"
    );
}

/// The consistency check's cost is a function of the ingest rate, not of corpus size.
///
/// That claim is what makes running it continuously affordable, and it is the whole justification for the
/// `idx_ingested_at` skip index being a schema requirement. Asked directly the question is a full scan with a
/// `GROUP BY … HAVING uniq(toYYYYMM(timestamp_start)) > 1` over everything; the check instead reads only rows
/// ingested since its watermark, because a correction always arrives as a new row.
///
/// **Asserted as a scaling property rather than as wall-clock time**, deliberately. A timing ceiling against a
/// container on a shared laptop measures the host, so it would either be so loose it gates nothing or so tight
/// it fails at random - and the property that actually needs protecting is incrementality. A pass that
/// re-examined the corpus would still be fast on a small fixture and would fail here.
///
/// **The corpus is spread wider than `WINDOW_OVERLAP`, and that is load-bearing rather than incidental.** The
/// overlap deliberately re-reads backwards from the watermark to catch a clock-behind writer, so rows stamped
/// *at* the watermark are re-examined every pass - which means a fixture whose every row shares one
/// `ingested_at` is re-read in full forever, and a first version of this test asserted incrementality against
/// exactly that shape and failed. The mechanism was right and the fixture was wrong, which is worth recording
/// because the failure looked like the opposite. In production ingest is continuous, so what the overlap holds
/// is a fixed *duration* of arrivals - bounded by rate, which is the claim - not a fixed fraction of the
/// corpus.
#[tokio::test]
async fn the_consistency_check_examines_new_rows_not_the_corpus() {
    let Ok(url) = std::env::var(URL_ENV) else {
        eprintln!("clickhouse parity: skipped - set {URL_ENV} (or run `make test-clickhouse`)");
        return;
    };

    let ch = clickhouse_backend(&url, "sideseat_parity_checkcost").await;
    let now = Utc::now();
    let base_instant = Utc
        .with_ymd_and_hms(now.year(), now.month(), 5, 12, 0, 0)
        .unwrap();

    // Ingested well over `WINDOW_OVERLAP` before the row added later, so the overlap cannot reach back to it.
    let corpus = 200;
    let spans: Vec<NormalizedSpan> = (0..corpus)
        .map(|i| NormalizedSpan {
            project_id: Some(PROJECT.to_string()),
            trace_id: format!("cost-trace-{i}"),
            span_id: format!("cost-span-{i}"),
            span_name: "generation".to_string(),
            timestamp_start: base_instant,
            timestamp_end: Some(base_instant),
            ingested_at: Some(ts(0)),
            ..Default::default()
        })
        .collect();
    ch.insert_spans(spans).await.expect("seed the corpus");

    // The cold pass reads what exists, because its watermark is the epoch. Expected, and the reason a new
    // replica is not asked to do this at startup.
    let first = ch.check_partition_consistency().await.expect("first pass");
    assert_eq!(
        first.examined, corpus as u64,
        "the cold pass reads what exists, got {first:?}"
    );
    assert_eq!(
        first.anomalies, 0,
        "one partition each, so nothing to report"
    );

    // One new span, ingested 50 minutes after the corpus - beyond the overlap, so it moves the watermark past
    // every existing row.
    ch.insert_spans(vec![NormalizedSpan {
        project_id: Some(PROJECT.to_string()),
        trace_id: "cost-trace-new".to_string(),
        span_id: "cost-span-new".to_string(),
        span_name: "generation".to_string(),
        timestamp_start: base_instant,
        timestamp_end: Some(base_instant),
        ingested_at: Some(ts(3000)),
        ..Default::default()
    }])
    .await
    .expect("one more span");

    // This pass still re-reads the corpus, because the watermark is only just past it. What matters is the
    // pass *after* it.
    ch.check_partition_consistency()
        .await
        .expect("the pass that advances the watermark past the corpus");

    let third = ch.check_partition_consistency().await.expect("third pass");
    assert!(
        third.examined <= 1,
        "with the watermark past the corpus a pass examined {} of {corpus} rows - the window is not \
         incremental, so the check is a full scan on a schedule and the cost claim is false",
        third.examined
    );

    // **`examined` is result cardinality, not work**, and asserting it alone is not enough: a pass that scans
    // the whole corpus and groups it down to one identity satisfies every assertion above. So the rows
    // ClickHouse actually *read* are checked, from its own query log - which is the only place that number
    // exists, and the thing the `idx_ingested_at` skip index is there to reduce.
    let client = raw_client(&url, "sideseat_parity_checkcost");
    client
        .query("SYSTEM FLUSH LOGS")
        .execute()
        .await
        .expect("flush the query log");

    let read_rows: Vec<u64> = client
        .query(
            "SELECT read_rows FROM system.query_log \
             WHERE type = 'QueryFinish' AND query LIKE '%groupUniqArray(toString(toYYYYMM%' \
             ORDER BY event_time_microseconds DESC LIMIT 1",
        )
        .fetch_all()
        .await
        .expect("read the query log");

    assert_eq!(
        read_rows.len(),
        1,
        "the consistency query is not in the query log, so this cannot check what it read"
    );
    assert!(
        read_rows[0] < corpus as u64,
        "the last consistency pass read {} rows out of a {corpus}-row corpus. `examined` was small, so the \
         *answer* was incremental while the *work* was a full scan - which is what an unusable or \
         unmaterialised skip index looks like, and is the claim this test exists to protect",
        read_rows[0]
    );
}

/// A released metric row and a later correction of the same datapoint **both** survive, and that is stated.
///
/// V2 has no `datapoint_id`, so the v3 rebuild gives existing rows the column's default of `''` while any
/// post-upgrade delivery carries a real digest. The column is in the new sorting key, so the two keys differ and
/// `FINAL` returns both rows - the correction adds to the measurement it was meant to replace.
///
/// **The assertion is the defect**, deliberately, because the alternative treatments are all worse: the digest
/// covers OTLP attributes with their protobuf variants preserved, so nothing in SQL reproduces it and no
/// backfill exists; deleting the released rows is data loss no operator asked for; and hiding an empty-id row
/// when an identified one appears for the same `(metric, timestamp)` would suppress a genuine unattributed
/// aggregate as soon as one unrelated series arrived. Over-reporting is the side this codebase takes when the
/// fact is unavailable - and this test is what stops it being read as fixed.
///
/// It also asserts the count is **reported**, since a residual nobody is told about is indistinguishable from a
/// bug.
#[tokio::test]
async fn a_released_metric_row_and_its_correction_both_survive() {
    let Ok(url) = std::env::var(URL_ENV) else {
        eprintln!("clickhouse parity: skipped - set {URL_ENV} (or run `make test-clickhouse`)");
        return;
    };

    let database = "sideseat_parity_legacymetric";
    let service = clickhouse_backend(&url, database).await;
    let client = raw_client(&url, database).with_option(
        sideseat_adapter_clickhouse::schema::TENANT_MAINTENANCE_SETTING,
        "1",
    );

    // A row as released v2 wrote it: no identity. Written directly, because the current writer always stamps
    // one - which is the point: this shape can only arrive from an older build.
    client
        .query(
            "INSERT INTO otel_metrics (project_id, metric_name, metric_type, timestamp, value_double, \
             datapoint_id) VALUES (?, 'legacy.counter', 'sum', now64(6), 1, '')",
        )
        .bind(PROJECT)
        .execute()
        .await
        .expect("insert a pre-identity row");

    // A correction of that datapoint, as the current build writes it.
    let stored_timestamp: Vec<i64> = client
        .query(
            "SELECT toUnixTimestamp64Micro(timestamp) FROM otel_metrics WHERE datapoint_id = '' LIMIT 1",
        )
        .fetch_all()
        .await
        .expect("read the instant back");
    let instant = DateTime::from_timestamp_micros(stored_timestamp[0]).expect("a valid instant");

    service
        .insert_metrics(&[NormalizedMetric {
            project_id: Some(PROJECT.to_string()),
            metric_name: "legacy.counter".to_string(),
            metric_type: MetricType::Sum,
            aggregation_temporality: AggregationTemporality::Cumulative,
            timestamp: instant,
            datapoint_id: "digest-of-the-same-datapoint".to_string(),
            value_double: Some(2.0),
            ..Default::default()
        }])
        .await
        .expect("insert the correction");

    let rows: Vec<f64> = client
        .query(
            "SELECT coalesce(value_double, 0) FROM otel_metrics FINAL \
             WHERE metric_name = 'legacy.counter' ORDER BY datapoint_id",
        )
        .fetch_all()
        .await
        .expect("read both back");

    assert_eq!(
        rows.len(),
        2,
        "the stated residual: a pre-identity row and its correction are two rows, not one. If this is ever 1 \
         the residual has been closed and the comments describing it are out of date"
    );
    assert_eq!(
        rows.iter().sum::<f64>(),
        3.0,
        "and they are summed, so the measurement is double-counted - got {rows:?}"
    );

    // Reported, not merely true.
    let unidentified = service
        .report_unidentified_metric_rows()
        .await
        .expect("the count is available");
    assert_eq!(
        unidentified, 1,
        "the pre-identity row must be counted, or an operator has no way to know the exposure exists"
    );
}
