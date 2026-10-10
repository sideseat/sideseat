use super::*;

fn span(project: &str, trace: &str, id: &str, session: Option<&str>) -> NormalizedSpan {
    at(project, trace, id, session, 0)
}

/// As [`span`], with an explicit start offset so a trace's earliest span is unambiguous.
fn at(
    project: &str,
    trace: &str,
    id: &str,
    session: Option<&str>,
    offset_secs: i64,
) -> NormalizedSpan {
    NormalizedSpan {
        project_id: Some(project.to_string()),
        trace_id: trace.to_string(),
        span_id: id.to_string(),
        session_id: session.map(str::to_string),
        timestamp_start: chrono::DateTime::<chrono::Utc>::UNIX_EPOCH
            + chrono::Duration::seconds(offset_secs),
        ..Default::default()
    }
}

/// A pipeline over a temporary DuckDB, with files off - enough to ask the store a question.
async fn pipeline_over_a_temp_store() -> (
    tempfile::TempDir,
    Arc<dyn AnalyticsRepository + Send + Sync>,
    Arc<dyn sideseat_ports::traits::TransactionalRepository + Send + Sync>,
    TracePipeline,
) {
    pipeline_over_a_temp_store_with(false).await
}

/// As [`pipeline_over_a_temp_store`], with file storage on or off.
pub(super) async fn pipeline_over_a_temp_store_with(
    files_enabled: bool,
) -> (
    tempfile::TempDir,
    Arc<dyn AnalyticsRepository + Send + Sync>,
    Arc<dyn sideseat_ports::traits::TransactionalRepository + Send + Sync>,
    TracePipeline,
) {
    let store = temp_pipeline(files_enabled).await;
    (store.temp, store.analytics, store.database, store.pipeline)
}

/// The same, with the DuckDB service behind the analytics port, for a test that must reach the store in a way no
/// port offers - losing a record its rows still name.
pub(super) async fn pipeline_over_a_temp_duckdb(
    files_enabled: bool,
) -> (
    tempfile::TempDir,
    Arc<sideseat_adapter_duckdb::DuckdbService>,
    Arc<dyn AnalyticsRepository + Send + Sync>,
    Arc<dyn sideseat_ports::traits::TransactionalRepository + Send + Sync>,
    TracePipeline,
) {
    let store = temp_pipeline(files_enabled).await;
    (
        store.temp,
        store.duckdb,
        store.analytics,
        store.database,
        store.pipeline,
    )
}

/// A pipeline over temporary DuckDB and SQLite stores, with the services behind its ports.
pub(super) struct TempPipeline {
    pub temp: tempfile::TempDir,
    pub duckdb: Arc<sideseat_adapter_duckdb::DuckdbService>,
    pub analytics: Arc<dyn AnalyticsRepository + Send + Sync>,
    pub database: Arc<dyn sideseat_ports::traits::TransactionalRepository + Send + Sync>,
    pub pipeline: TracePipeline,
}

pub(super) async fn temp_pipeline(files_enabled: bool) -> TempPipeline {
    use chrono::{TimeZone, Utc};
    use sideseat_adapter_blob_storage::FilesystemStorage;
    use sideseat_adapter_cache::CacheService;
    use sideseat_adapter_duckdb::{DuckdbRepository, DuckdbService};
    use sideseat_adapter_sqlite::{SqliteRepository, SqliteService};
    use sideseat_core::config::{
        CacheBackendType, CacheConfig, EvictionPolicy, FilesConfig, StorageBackend,
    };
    use sideseat_core::storage::AppStorage;
    use sideseat_domain::pricing::PricingService;
    use sideseat_messaging::TopicService;
    use sideseat_ports::clock::Clock;
    use sideseat_ports::traits::TransactionalRepository;

    #[derive(Debug)]
    struct TestClock;

    impl Clock for TestClock {
        fn now(&self) -> chrono::DateTime<Utc> {
            Utc.timestamp_opt(1_700_000_000, 0).single().unwrap()
        }
    }

    let temp = tempfile::TempDir::new().expect("temp dir");
    let storage = AppStorage::init_for_test(temp.path().to_path_buf());
    tokio::fs::create_dir_all(temp.path().join("duckdb"))
        .await
        .expect("duckdb dir");
    let duckdb = Arc::new(
        DuckdbService::init(&storage, Arc::new(TestClock))
            .await
            .expect("duckdb"),
    );
    let analytics: Arc<dyn AnalyticsRepository + Send + Sync> =
        Arc::new(DuckdbRepository(Arc::clone(&duckdb)));
    let sqlite = Arc::new(
        SqliteService::init(&storage, Arc::new(TestClock))
            .await
            .expect("sqlite"),
    );
    let database: Arc<dyn TransactionalRepository + Send + Sync> =
        Arc::new(SqliteRepository(Arc::clone(&sqlite)));
    let cache = Arc::new(
        CacheService::new(&CacheConfig {
            backend: CacheBackendType::Memory,
            max_entries: 16,
            eviction_policy: EvictionPolicy::TinyLfu,
            redis_url: None,
        })
        .await
        .expect("memory cache"),
    );
    let file_config = FilesConfig {
        enabled: files_enabled,
        storage: StorageBackend::Filesystem,
        quota_bytes: 0,
        filesystem_path: Some(temp.path().join("files").display().to_string()),
        s3: None,
    };
    let temp_files = storage.subdir(sideseat_core::storage::DataSubdir::FilesTemp);
    tokio::fs::create_dir_all(&temp_files)
        .await
        .expect("files temp dir");
    let files = Arc::new(
        FileService::new(
            file_config,
            storage.subdir(sideseat_core::storage::DataSubdir::FilesTemp),
            Arc::new(FilesystemStorage::new(temp.path().join("files"))),
            Arc::clone(&database),
            cache,
        )
        .await
        .expect("file service"),
    );
    let pipeline = TracePipeline::new(
        Arc::clone(&analytics),
        Arc::new(PricingService::init_for_test().expect("offline pricing")),
        Arc::new(TopicService::new(sideseat_adapter_topics::memory_backend())),
        Arc::clone(&files),
        Arc::new(StagingService::new(
            Arc::clone(files.storage()),
            Arc::clone(&database),
            Arc::clone(&analytics),
            Arc::new(TestClock),
            sideseat_core::config::RetentionConfig::default(),
            5,
        )),
    );
    TempPipeline {
        temp,
        duckdb,
        analytics,
        database,
        pipeline,
    }
}

#[tokio::test]
async fn pipeline_stops_when_the_shutdown_sender_is_dropped() {
    let (_temp, _analytics, _database, pipeline) = pipeline_over_a_temp_store().await;
    let topic = pipeline
        .topics
        .stream_topic::<StagedPayloadRef>("shutdown-test", StagedPayloadRef::partition_key);
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let handle = Arc::new(pipeline).start(topic, shutdown_rx);

    drop(shutdown_tx);

    tokio::time::timeout(Duration::from_secs(1), handle)
        .await
        .expect("pipeline should stop when its shutdown channel closes")
        .expect("pipeline task should not panic");
}

/// A row matching the redelivery is not enough to skip it: this one names no raw record, so both revisions are
/// written - which re-creates the authority - rather than the matching one being dropped as already stored.
/// Dropping an exact redelivery the raw authority does hold is `raw_authority_tests`'.
#[tokio::test]
async fn a_redelivery_matching_only_unbacked_rows_is_kept() {
    let (_temp, analytics, _database, pipeline) = pipeline_over_a_temp_store().await;
    let mut stored = at("p", "trace", "span", None, 0);
    stored.content_digest = "same".to_string();
    analytics
        .insert_spans(vec![stored.clone()])
        .await
        .expect("store the winner");

    let mut correction = stored.clone();
    correction.content_digest = "changed".to_string();
    let mut incoming = vec![stored, correction];
    assert_eq!(pipeline.drop_exact_redeliveries(&mut incoming).await, 0);
    assert_eq!(incoming.len(), 2);
}

/// An SSE event names the session **a read will return**, not the one this batch happens to know.
///
/// A subscriber filtered by session compares `event.session_id`. The canonical session is the one on the
/// trace's earliest span, so a batch carrying only a later span - the ordinary shape of a streaming
/// exporter - resolved the trace to its own span's session and announced the span under a session no read
/// places it in. The store is asked after the write, when it can answer.
#[tokio::test]
async fn an_sse_event_carries_the_session_a_read_will_return() {
    let (_temp, analytics, _database, pipeline) = pipeline_over_a_temp_store().await;

    // The trace begins in session A, already stored.
    analytics
        .as_ref()
        .insert_spans(vec![at("p", "t1", "root", Some("session-a"), 0)])
        .await
        .expect("store the root");

    // A later batch carries only a child, which names a different session.
    let batch = vec![NormalizedSpan {
        parent_span_id: Some("root".to_string()),
        ..at("p", "t1", "child", Some("session-b"), 5)
    }];
    let mut events = sse_events_for(&batch);
    assert_eq!(
        events[0].session_id.as_deref(),
        Some("session-b"),
        "the batch on its own can only say what its own spans say"
    );

    analytics
        .as_ref()
        .insert_spans(batch)
        .await
        .expect("store the child");
    pipeline.stamp_stored_sessions(&mut events).await;

    assert_eq!(
        events[0].session_id.as_deref(),
        Some("session-a"),
        "the store holds the trace's earliest span, so its session is the one a read returns"
    );
}

/// The store's answer is taken whole: "this trace has no session" is a fact, not a gap.
///
/// Only overwriting the traces the store named left the batch's value standing for the rest, so a batch
/// that corrected a span by *removing* its session still announced the old one to a session page the
/// trace no longer belongs to.
#[tokio::test]
async fn an_sse_event_loses_a_session_the_store_no_longer_reports() {
    let (_temp, analytics, _database, pipeline) = pipeline_over_a_temp_store().await;

    // One span, delivered with a session and then corrected without one. Storage keeps the latest.
    let with_session = at("p", "t1", "only", Some("session-a"), 0);
    let corrected = at("p", "t1", "only", None, 0);
    analytics
        .as_ref()
        .insert_spans(vec![with_session.clone()])
        .await
        .expect("first delivery");
    analytics
        .as_ref()
        .insert_spans(vec![corrected])
        .await
        .expect("correction");

    // The event was built from the delivery that carried the session.
    let mut events = sse_events_for(&[with_session]);
    assert_eq!(
        events[0].session_id.as_deref(),
        Some("session-a"),
        "premise"
    );

    pipeline.stamp_stored_sessions(&mut events).await;
    assert_eq!(
        events[0].session_id, None,
        "the stored trace has no session, so neither does the event"
    );
}

#[tokio::test]
async fn queued_span_covered_by_exact_deletion_journal_is_dropped_before_write() {
    let (_temp, _analytics, database, pipeline) = pipeline_over_a_temp_store().await;
    let project_id = ProjectId::from("p");
    database
        .record_deleted_spans_journalled(
            &project_id,
            &[("trace".to_string(), "deleted".to_string())],
        )
        .await
        .unwrap();

    let mut batch = vec![
        at("p", "trace", "deleted", None, 0),
        at("p", "trace", "live", None, 1),
    ];
    assert_eq!(
        pipeline
            .drop_spans_for_journalled_deletions(&mut batch)
            .await,
        Ok(1)
    );
    assert_eq!(
        batch
            .iter()
            .map(|span| span.span_id.as_str())
            .collect::<Vec<_>>(),
        vec!["live"],
        "a sibling not named by the exact deletion remains ingestible"
    );
}

#[tokio::test]
async fn span_committed_after_pressure_delete_is_removed_by_post_write_compensation() {
    let (_temp, analytics, database, pipeline) = pipeline_over_a_temp_store().await;
    let project_id = ProjectId::from("p");
    database
        .record_pressure_eviction(&project_id, &[("trace".to_string(), "evicted".to_string())])
        .await
        .unwrap();

    analytics
        .insert_spans(vec![
            at("p", "trace", "evicted", None, 0),
            at("p", "trace", "live", None, 1),
        ])
        .await
        .unwrap();
    let written = vec![
        ("p".to_string(), "trace".to_string(), "evicted".to_string()),
        ("p".to_string(), "trace".to_string(), "live".to_string()),
    ];
    let removed = pipeline
        .collect_spans_written_for_deleted_traces(&written, &mut Vec::new())
        .await;
    assert_eq!(
        removed,
        HashSet::from([("p".to_string(), "trace".to_string(), "evicted".to_string(),)])
    );
    assert!(
        analytics
            .get_span(&project_id, "trace", "evicted")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        analytics
            .get_span(&project_id, "trace", "live")
            .await
            .unwrap()
            .is_some()
    );
}

/// A later delivery that *removes* a span's session wins, as it does in storage.
///
/// Storage keeps one row per span - the latest delivery - so a span re-sent without a session no longer
/// carries one. The resolver visited every delivery, so the earlier session survived and a trace whose
/// session had been removed was still resolved to it. Two passes now: latest delivery per span, then the
/// earliest surviving span that has a session, which is exactly `arg_min(...)` after
/// `WHERE session_id IS NOT NULL`.
#[test]
fn a_delivery_that_removes_a_session_wins_over_the_one_that_set_it() {
    let spans = vec![
        at("p", "t1", "root", Some("session-a"), 0),
        // The same span again, corrected to carry no session.
        at("p", "t1", "root", None, 0),
    ];
    assert!(
        !canonical_session_of_traces(&spans).contains_key(&("p".to_string(), "t1".to_string())),
        "the latest delivery of that span has no session, so the trace has none"
    );

    // And the reverse order is not symmetric - it is the *last* delivery that counts, not the one with
    // a session.
    let reinstated = vec![
        at("p", "t1", "root", None, 0),
        at("p", "t1", "root", Some("session-a"), 0),
    ];
    assert_eq!(
        canonical_session_of_traces(&reinstated)
            .get(&("p".to_string(), "t1".to_string()))
            .map(String::as_str),
        Some("session-a")
    );
}

/// A sessionless span does not decide which span is the trace's earliest.
///
/// The rule is "the session on the earliest span *that has one*" - `arg_min` runs after
/// `WHERE session_id IS NOT NULL`. Letting sessionless spans compete made the answer depend on spans
/// that say nothing about membership: a child sorting before the root by span id emptied the trace's
/// session entirely.
#[test]
fn a_sessionless_span_does_not_decide_the_trace_s_session() {
    let spans = vec![
        at("p", "t1", "aaa-child", None, 0),
        at("p", "t1", "root", Some("session-a"), 0),
    ];
    assert_eq!(
        canonical_session_of_traces(&spans)
            .get(&("p".to_string(), "t1".to_string()))
            .map(String::as_str),
        Some("session-a")
    );
}

/// A live event names the trace's session, so a session subscription sees the whole trace.
///
/// A subscriber filtered by session compares `event.session_id`, and a span carries one only if it is the
/// span that knew it - the root, for every framework here. So a child span's event was discarded by the
/// session page it belonged to, and a span naming a different session sent an event to a page that
/// session's trace does not appear on.
#[test]
fn a_live_event_carries_its_trace_s_canonical_session() {
    let spans = vec![
        at("p", "t1", "root", Some("session-a"), 0),
        at("p", "t1", "child", None, 1),
        at("p", "t1", "stray", Some("session-b"), 2),
    ];

    let events = sse_events_for(&spans);
    let sessions: Vec<Option<&str>> = events.iter().map(|e| e.session_id.as_deref()).collect();
    assert_eq!(
        sessions,
        vec![Some("session-a"), Some("session-a"), Some("session-a")],
        "every span of the trace must reach the session page the trace is displayed on"
    );
}

/// A batch of orphan children names no session, and the event says so rather than guessing.
#[test]
fn a_batch_that_names_no_session_leaves_the_event_unstamped() {
    let spans = vec![at("p", "t1", "child", None, 0)];
    let events = sse_events_for(&spans);
    assert_eq!(events[0].session_id, None);
}

/// Deleting one session must not drop a trace that belongs to another.
///
/// A trace whose earliest span names session A and whose child names B is displayed, read, filtered and
/// deleted as belonging to **A** everywhere else. The ingestion fence asked whether *any* span named a
/// deleted session, so deleting B caused a redelivery of that trace to be tombstoned and dropped in
/// full - and the tombstone then had the sweep delete the rows already stored under A. Data loss, from a
/// deletion of a session the trace was never canonically in.
#[test]
fn deleting_a_session_a_trace_is_not_canonically_in_leaves_it_alone() {
    let spans = vec![
        at("p", "t1", "root", Some("session-a"), 0),
        at("p", "t1", "child", Some("session-b"), 1),
    ];

    // Deleting the session named only by the later span touches nothing.
    let deleted_b = HashSet::from([("p".to_string(), "session-b".to_string())]);
    assert!(
        traces_of_sessions(&spans, &deleted_b).is_empty(),
        "the trace's canonical session is A, so deleting B must not resolve to it"
    );

    // Deleting the canonical session takes the whole trace, as before.
    let deleted_a = HashSet::from([("p".to_string(), "session-a".to_string())]);
    assert_eq!(
        traces_of_sessions(&spans, &deleted_a),
        HashSet::from([("p".to_string(), "t1".to_string())]),
    );

    // And the resolver answers by earliest span, not by batch order: reversing the input must not
    // change which session the trace is in.
    let reversed: Vec<NormalizedSpan> = spans.iter().rev().cloned().collect();
    assert_eq!(
        canonical_session_of_traces(&spans),
        canonical_session_of_traces(&reversed),
        "the canonical session must not depend on the order spans arrive in"
    );
    assert_eq!(
        canonical_session_of_traces(&spans)
            .get(&("p".to_string(), "t1".to_string()))
            .map(String::as_str),
        Some("session-a")
    );
}

/// A deleted session takes its trace's *whole* set of spans, not only the ones naming it.
///
/// The shape that matters, and the one every framework produces: the session id sits on the root and on
/// nothing else. Matching per span therefore dropped the root and kept the children, which recreated a
/// deleted session's trace minus its head.
#[test]
fn a_deleted_session_takes_the_children_that_do_not_name_it() {
    let spans = vec![
        span("p", "t1", "root", Some("s1")),
        span("p", "t1", "child", None),
        span("p", "t1", "grandchild", None),
        span("p", "t2", "other-root", Some("s2")),
        span("p", "t2", "other-child", None),
    ];
    let deleted = HashSet::from([("p".to_string(), "s1".to_string())]);

    let traces = traces_of_sessions(&spans, &deleted);
    assert_eq!(
        traces,
        HashSet::from([("p".to_string(), "t1".to_string())]),
        "the deleted session must resolve to its whole trace"
    );

    let mut kept = spans.clone();
    kept.retain(|s| {
        !traces.contains(&(
            s.project_id
                .as_deref()
                .unwrap_or(DEFAULT_PROJECT_ID)
                .to_string(),
            s.trace_id.clone(),
        ))
    });
    let ids: Vec<&str> = kept.iter().map(|s| s.span_id.as_str()).collect();
    assert_eq!(
        ids,
        vec!["other-root", "other-child"],
        "no span of a deleted session's trace may survive, and no other trace may be touched"
    );
}

/// A session id is scoped to its project, so the same id elsewhere is a different session.
#[test]
fn a_session_id_in_another_project_is_a_different_session() {
    let spans = vec![
        span("p", "t1", "root", Some("shared")),
        span("q", "t2", "root", Some("shared")),
    ];
    let deleted = HashSet::from([("p".to_string(), "shared".to_string())]);
    assert_eq!(
        traces_of_sessions(&spans, &deleted),
        HashSet::from([("p".to_string(), "t1".to_string())]),
        "a deletion in one project must not reach another project's session of the same name"
    );
}
