/// A log record with no time of its own, carrying a user message for a span, as an exporter sends it: its
/// `timestamp` falls back to the receipt, so the same record received in another month is a later one.
fn timeless_message_log(received_at: DateTime<Utc>) -> NormalizedLog {
    use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
    use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue, KeyValueList, any_value};
    use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};

    let text = |value: &str| AnyValue {
        value: Some(any_value::Value::StringValue(value.to_string())),
    };
    let request = ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            scope_logs: vec![ScopeLogs {
                log_records: vec![LogRecord {
                    event_name: "gen_ai.user.message".to_string(),
                    body: Some(AnyValue {
                        value: Some(any_value::Value::KvlistValue(KeyValueList {
                            values: vec![KeyValue {
                                key: "content".to_string(),
                                value: Some(text("What is the capital of France?")),
                            }],
                        })),
                    }),
                    trace_id: vec![0xa1; 16],
                    span_id: vec![0xb2; 8],
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        }],
    };
    let mut logs = sideseat_ingestion::logs::extract_logs_batch(&request, received_at);
    assert_eq!(logs.len(), 1);
    let mut log = logs.remove(0);
    assert!(log.messages.is_some(), "the record carries a message");
    log.project_id = Some(PROJECT.to_string());
    log.ingested_at = Some(received_at);
    log
}

/// The span the timeless record's message belongs to.
fn span_of_the_timeless_log() -> NormalizedSpan {
    NormalizedSpan {
        project_id: Some(PROJECT.to_string()),
        trace_id: "a1".repeat(16),
        span_id: "b2".repeat(8),
        span_name: "generation".to_string(),
        timestamp_start: ts(0),
        ingested_at: Some(ts(0)),
        ..Default::default()
    }
}

/// How many messages the span's view joins from its log records.
async fn log_messages_of_the_span(store: &(impl MessageStore + ?Sized)) -> usize {
    let span = span_of_the_timeless_log();
    let rows = store
        .get_messages(&MessageQueryParams {
            project_id: ProjectId::from(PROJECT),
            trace_id: Some(span.trace_id.clone()),
            span_id: Some(span.span_id.clone()),
            ..Default::default()
        })
        .await
        .expect("the span's messages")
        .rows;
    let joined: Vec<serde_json::Value> = rows
        .iter()
        .flat_map(|row| {
            serde_json::from_str::<Vec<serde_json::Value>>(&row.log_messages_json)
                .expect("log messages JSON")
        })
        .collect();
    joined.len()
}

/// The same timeless record, received at the end of one month and again at the start of the next.
fn the_timeless_log_twice() -> [NormalizedLog; 2] {
    let first = Utc
        .with_ymd_and_hms(2026, 9, 30, 23, 0, 0)
        .single()
        .expect("instant");
    let again = Utc
        .with_ymd_and_hms(2026, 10, 1, 1, 0, 0)
        .single()
        .expect("instant");
    let [first, again] = [timeless_message_log(first), timeless_message_log(again)];
    assert_eq!(
        (&first.log_digest, first.ordinal),
        (&again.log_digest, again.ordinal),
        "one record, delivered twice"
    );
    assert_ne!(first.timestamp.month(), again.timestamp.month());
    [first, again]
}

/// A log record with no time of its own, delivered again in another month, is still one record on both backends:
/// its span's view joins its message once. On ClickHouse its `timestamp` - the receipt - chose the partition, and
/// the client resolves `FINAL` a partition at a time, so the second delivery was a second row and the message was
/// shown twice.
#[tokio::test]
async fn a_timeless_log_delivered_again_in_another_month_is_read_once_on_both_backends() {
    let Ok(url) = std::env::var(URL_ENV) else {
        eprintln!("clickhouse parity: skipped - set {URL_ENV} (or run `make test-clickhouse`)");
        return;
    };
    let (_temp, duck) = duckdb_backend().await;
    let ch = clickhouse_backend(&url, "sideseat_parity_timeless_logs").await;
    for repo in [&duck as &dyn AnalyticsRepository, &ch] {
        repo.insert_spans(vec![span_of_the_timeless_log()])
            .await
            .expect("span");
        for delivery in the_timeless_log_twice() {
            repo.insert_logs(std::slice::from_ref(&delivery))
                .await
                .expect("log");
        }
        assert_eq!(log_messages_of_the_span(repo).await, 1);
    }
}

/// The same on a cluster of two shards, through the `Distributed` front end the reads use.
#[tokio::test]
async fn a_timeless_log_delivered_again_in_another_month_is_read_once_on_a_two_shard_cluster() {
    let Ok(url) = std::env::var(TWO_SHARD_URL_ENV) else {
        eprintln!(
            "clickhouse two-shard: skipped - set {TWO_SHARD_URL_ENV} (or run \
             `make test-clickhouse-two-shard`)"
        );
        return;
    };
    let store = replicated_backend_at(&url, "sideseat_two_shard_timeless_logs").await;
    store
        .insert_spans(vec![span_of_the_timeless_log()])
        .await
        .expect("span");
    for delivery in the_timeless_log_twice() {
        store
            .insert_logs(std::slice::from_ref(&delivery))
            .await
            .expect("log");
    }
    assert_eq!(log_messages_of_the_span(&store).await, 1);
}

/// Whether a service of `config` refuses to start for a log table partitioned by `timestamp`, naming the layout.
async fn refuses_receipt_partitioned_logs(config: &ClickhouseConfig) -> bool {
    match ClickhouseService::init(
        config,
        std::sync::Arc::new(sideseat_server::runtime::clock::SystemClock),
    )
    .await
    {
        Ok(_) => false,
        Err(error) => {
            let message = error.to_string();
            assert!(
                message.contains("partitioned by `toYYYYMM(timestamp)`")
                    && message.contains("drop the configured database"),
                "the refusal names the layout and the reset: {message}"
            );
            true
        }
    }
}

/// A store created while the log records were partitioned by `timestamp` is at the supported version, and reads a
/// timeless record delivered in two months twice. It is refused at startup instead, with the reset named.
#[tokio::test]
async fn a_store_with_receipt_partitioned_logs_is_refused_at_startup() {
    let Ok(url) = std::env::var(URL_ENV) else {
        eprintln!("clickhouse parity: skipped - set {URL_ENV} (or run `make test-clickhouse`)");
        return;
    };
    let database = "sideseat_parity_log_layout";
    let _created = clickhouse_backend(&url, database).await;
    let client = raw_client(&url, database);
    for statement in [
        "CREATE TABLE otel_logs_months AS otel_logs ENGINE = ReplacingMergeTree(ingested_at) \
         PARTITION BY toYYYYMM(timestamp) ORDER BY (project_id, log_digest, ordinal)",
        "DROP TABLE otel_logs SYNC",
        "RENAME TABLE otel_logs_months TO otel_logs",
    ] {
        client.query(statement).execute().await.expect(statement);
    }
    assert!(
        refuses_receipt_partitioned_logs(&layout_config(&url, database, false)).await,
        "a store with receipt-partitioned logs started"
    );
}

/// The same refusal on a cluster, through the shard that created the store and through the other.
#[tokio::test]
async fn a_two_shard_store_with_receipt_partitioned_logs_is_refused_at_startup() {
    let Ok(url) = std::env::var(TWO_SHARD_URL_ENV) else {
        eprintln!(
            "clickhouse two-shard: skipped - set {TWO_SHARD_URL_ENV} (or run \
             `make test-clickhouse-two-shard`)"
        );
        return;
    };
    let database = "sideseat_two_shard_log_layout";
    let _created = replicated_backend_at(&url, database).await;
    let user = std::env::var(USER_ENV).ok();
    let password = std::env::var(PASSWORD_ENV).ok();
    let client = raw_client_at(&url, database, &user, &password);
    for statement in [
        format!(
            "CREATE TABLE otel_logs_local_months ON CLUSTER {REPLICATED_CLUSTER} AS otel_logs_local \
             ENGINE = ReplicatedReplacingMergeTree('/clickhouse/tables/{{shard}}/{database}/otel_logs_months', \
             '{{replica}}', ingested_at) PARTITION BY toYYYYMM(timestamp) \
             ORDER BY (project_id, log_digest, ordinal)"
        ),
        format!("DROP TABLE otel_logs_local ON CLUSTER {REPLICATED_CLUSTER} SYNC"),
        format!(
            "RENAME TABLE otel_logs_local_months TO otel_logs_local ON CLUSTER {REPLICATED_CLUSTER}"
        ),
    ] {
        client.query(&statement).execute().await.expect(&statement);
    }
    assert!(
        refuses_receipt_partitioned_logs(&layout_config(&url, database, true)).await,
        "a cluster store with receipt-partitioned logs started"
    );
    match std::env::var("SIDESEAT_TEST_CLICKHOUSE_TWO_SHARD_SECOND_URL") {
        Ok(second) => assert!(
            refuses_receipt_partitioned_logs(&layout_config(&second, database, true)).await,
            "the store started through the shard that did not create it"
        ),
        Err(_) => eprintln!(
            "clickhouse two-shard: the second shard's URL is unset, so only the first is checked"
        ),
    }
}
