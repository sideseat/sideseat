//! The post-write session check takes a trace only on evidence of its beginning.
//!
//! A batch of child spans says nothing about which session their trace is in: the canonical session is the one on
//! the trace's earliest span, and a child is not it. The pre-write fence already refuses to fence on such a batch;
//! the post-write check tombstoned on it, from the store's answer - which, right after the write, is that same
//! batch. Two instances ingesting one trace then lost acknowledged data: one writes a child naming a deleted
//! session D and reads "T is in D"; the other writes T's root naming a live session L, passes every check and
//! answers 200; the first then tombstones T on its stale answer, and the trace sweep deletes the root.

use super::fences_early::traces_begun;
use super::pipeline_tests::temp_pipeline;
use super::*;

fn span(
    trace: &str,
    id: &str,
    parent: Option<&str>,
    session: &str,
    offset_secs: i64,
) -> NormalizedSpan {
    NormalizedSpan {
        project_id: Some("p".to_string()),
        trace_id: trace.to_string(),
        span_id: id.to_string(),
        parent_span_id: parent.map(str::to_string),
        session_id: Some(session.to_string()),
        timestamp_start: chrono::DateTime::<chrono::Utc>::UNIX_EPOCH
            + chrono::Duration::seconds(offset_secs),
        ..Default::default()
    }
}

fn written(trace: &str, id: &str) -> Vec<(String, String, String)> {
    vec![("p".to_string(), trace.to_string(), id.to_string())]
}

/// A batch carrying only a child of a trace, naming a deleted session, does not tombstone the trace: a root under a
/// live session, written by another instance in the meantime and already acknowledged, would be deleted with it.
#[tokio::test]
async fn a_child_naming_a_deleted_session_does_not_take_its_trace() {
    let store = temp_pipeline(false).await;
    let project = ProjectId::from("p");
    store
        .database
        .record_deleted_sessions(&project, &["session-d".to_string()])
        .await
        .unwrap();

    // Instance A writes a child of T naming the deleted session; the store knows nothing else of T.
    store
        .analytics
        .insert_spans(vec![span("t", "child", Some("root"), "session-d", 5)])
        .await
        .unwrap();
    let tombstoned = store
        .pipeline
        .tombstone_traces_of_deleted_sessions(&written("t", "child"), &HashSet::new())
        .await;

    assert!(
        tombstoned.is_empty(),
        "a child is no evidence of its trace's session: {tombstoned:?}"
    );
    assert!(
        store
            .database
            .deleted_traces_among(&project, &["t".to_string()])
            .await
            .unwrap()
            .is_empty(),
        "no tombstone for T"
    );

    // Instance B's root names live session L and is earlier: T is L's, and B's acknowledged root stays.
    store
        .analytics
        .insert_spans(vec![span("t", "root", None, "session-l", 0)])
        .await
        .unwrap();
    let removed = store
        .pipeline
        .collect_spans_written_for_deleted_traces(&written("t", "root"), &mut Vec::new())
        .await;
    assert!(removed.is_empty());
    assert!(
        store
            .analytics
            .get_span(&project, "t", "root")
            .await
            .unwrap()
            .is_some()
    );
}

/// The case the check exists for still holds: a batch that carries a trace's root, naming a session deleted while
/// it was written, has its trace tombstoned.
#[tokio::test]
async fn a_root_naming_a_deleted_session_takes_its_trace() {
    let store = temp_pipeline(false).await;
    let project = ProjectId::from("p");
    store
        .database
        .record_deleted_sessions(&project, &["session-d".to_string()])
        .await
        .unwrap();
    store
        .analytics
        .insert_spans(vec![
            span("t", "root", None, "session-d", 0),
            span("t", "child", Some("root"), "session-d", 1),
        ])
        .await
        .unwrap();
    let mut both = written("t", "root");
    both.extend(written("t", "child"));
    let tombstoned = store
        .pipeline
        .tombstone_traces_of_deleted_sessions(
            &both,
            &HashSet::from([("p".to_string(), "t".to_string())]),
        )
        .await;
    assert_eq!(
        tombstoned,
        HashSet::from([("p".to_string(), "t".to_string())])
    );
}

/// Through a whole batch, where the post-write check decides - the pre-write fence lets both exports through: an
/// export of a child naming a deleted session leaves its trace untombstoned (no root, a trace the store does not
/// know); an export of a root naming it, for a trace the store holds only a later span of under a live session (so
/// the fence keeps it), makes the deleted session the trace's and takes the trace.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_batch_tombstones_only_the_traces_it_began() {
    use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue, any_value};
    use opentelemetry_proto::tonic::resource::v1::Resource;
    use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, Span};

    fn otlp(trace: u8, id: u8, parent: Option<u8>) -> ExportTraceServiceRequest {
        let span = Span {
            trace_id: vec![trace; 16],
            span_id: vec![id; 8],
            parent_span_id: parent.map(|p| vec![p; 8]).unwrap_or_default(),
            name: "step".into(),
            start_time_unix_nano: 1_700_000_000_000_000_000 + u64::from(id),
            end_time_unix_nano: 1_700_000_000_000_000_100 + u64::from(id),
            attributes: vec![KeyValue {
                key: "session.id".into(),
                value: Some(AnyValue {
                    value: Some(any_value::Value::StringValue("session-d".into())),
                }),
            }],
            ..Default::default()
        };
        ExportTraceServiceRequest {
            resource_spans: vec![ResourceSpans {
                resource: Some(Resource::default()),
                scope_spans: vec![ScopeSpans {
                    spans: vec![span],
                    ..Default::default()
                }],
                ..Default::default()
            }],
        }
    }
    let store = temp_pipeline(false).await;
    let project = ProjectId::from(DEFAULT_PROJECT_ID);
    // Deleted before either export: what the post-write check exists for is a deletion landing during the write,
    // and the pre-write fence leaves a trace it has no evidence for to it.
    store
        .database
        .record_deleted_sessions(&project, &["session-d".to_string()])
        .await
        .unwrap();
    let tombstoned = |trace: u8| {
        let database = Arc::clone(&store.database);
        let project = project.clone();
        async move {
            let id = hex::encode([trace; 16]);
            !database
                .deleted_traces_among(&project, std::slice::from_ref(&id))
                .await
                .unwrap()
                .is_empty()
        }
    };

    store
        .pipeline
        .run_batch_outcomes_for_test(&[otlp(1, 2, Some(1))])
        .await;
    assert!(
        !tombstoned(1).await,
        "a child is no evidence of its trace's session"
    );

    store
        .analytics
        .insert_spans(vec![NormalizedSpan {
            project_id: Some(DEFAULT_PROJECT_ID.to_string()),
            trace_id: hex::encode([2u8; 16]),
            span_id: hex::encode([9u8; 8]),
            parent_span_id: Some(hex::encode([1u8; 8])),
            session_id: Some("session-l".to_string()),
            timestamp_start: chrono::DateTime::from_timestamp(1_800_000_000, 0).unwrap(),
            ..Default::default()
        }])
        .await
        .unwrap();
    store
        .pipeline
        .run_batch_outcomes_for_test(&[otlp(2, 1, None)])
        .await;
    assert!(
        tombstoned(2).await,
        "the root it wrote names the deleted session, so the trace is that session's"
    );
}

/// What a batch began is the traces of its parentless spans - no parent or an empty one - per project.
#[test]
fn a_trace_is_begun_by_a_span_without_a_parent() {
    let mut empty_parent = span("t2", "root", None, "s", 0);
    empty_parent.parent_span_id = Some(String::new());
    let mut elsewhere = span("t1", "root", None, "s", 0);
    elsewhere.project_id = Some("q".to_string());
    let spans = vec![
        span("t1", "child", Some("root"), "s", 1),
        empty_parent,
        elsewhere,
        span("t3", "root", None, "s", 0),
    ];
    assert_eq!(
        traces_begun(&spans),
        HashSet::from([
            ("p".to_string(), "t2".to_string()),
            ("q".to_string(), "t1".to_string()),
            ("p".to_string(), "t3".to_string()),
        ])
    );
}

/// As [`span`], received at `received` seconds since the epoch.
fn received(mut span: NormalizedSpan, received: i64) -> NormalizedSpan {
    span.ingested_at = Some(chrono::DateTime::from_timestamp(received, 0).unwrap());
    span
}

/// One pass of the session sweep with `grace`. A session just deleted is due at once; one checked is not again for a
/// while, so each test sweeps once.
async fn sweep(store: &super::pipeline_tests::TempPipeline, grace: std::time::Duration) {
    sideseat_domain::cleanup::advance_pending_deletions(
        &store.database,
        &store.analytics,
        &store.pipeline.file_service,
        0,
        grace,
    )
    .await
    .unwrap();
}

async fn tombstoned(store: &super::pipeline_tests::TempPipeline, trace: &str) -> bool {
    !store
        .database
        .deleted_traces_among(&ProjectId::from("p"), &[trace.to_string()])
        .await
        .unwrap()
        .is_empty()
}

const WEEK: std::time::Duration = std::time::Duration::from_secs(7 * 24 * 60 * 60);

/// The session sweep holds a rootless trace of a deleted session for its root, until the grace passes; one whose
/// root is stored and names the session goes at once.
#[tokio::test]
async fn the_session_sweep_holds_a_rootless_trace_for_its_root() {
    let store = temp_pipeline(false).await;
    let project = ProjectId::from("p");
    store
        .database
        .record_deleted_sessions(&project, &["session-d".to_string()])
        .await
        .unwrap();
    let now = chrono::Utc::now().timestamp();
    store
        .analytics
        .insert_spans(vec![
            received(span("waiting", "child", Some("root"), "session-d", 5), now),
            received(span("rooted", "root", None, "session-d", 0), now),
        ])
        .await
        .unwrap();

    sweep(&store, WEEK).await;
    assert!(
        tombstoned(&store, "rooted").await,
        "its root decides: it goes now"
    );
    assert!(!tombstoned(&store, "waiting").await, "held for its root");
    assert!(
        store
            .analytics
            .get_span(&project, "waiting", "child")
            .await
            .unwrap()
            .is_some(),
        "readable while it waits"
    );
}

/// Once the grace has passed, a rootless trace of a deleted session goes; one whose root arrived in time naming a
/// live session is that session's, and stays whole.
#[tokio::test]
async fn after_the_grace_a_rootless_trace_goes_and_a_rooted_one_follows_its_root() {
    let store = temp_pipeline(false).await;
    let project = ProjectId::from("p");
    store
        .database
        .record_deleted_sessions(&project, &["session-d".to_string()])
        .await
        .unwrap();
    let week_ago = chrono::Utc::now().timestamp() - 8 * 24 * 60 * 60;
    store
        .analytics
        .insert_spans(vec![
            received(
                span("waiting", "child", Some("root"), "session-d", 5),
                week_ago,
            ),
            received(
                span("moved", "child", Some("root"), "session-d", 5),
                week_ago,
            ),
            received(span("moved", "root", None, "session-l", 0), week_ago),
        ])
        .await
        .unwrap();

    sweep(&store, WEEK).await;
    assert!(
        tombstoned(&store, "waiting").await,
        "the grace has passed with no root"
    );
    assert!(
        !tombstoned(&store, "moved").await,
        "its root named a live session"
    );
    assert!(
        store
            .analytics
            .get_span(&project, "moved", "child")
            .await
            .unwrap()
            .is_some(),
        "kept whole, the child that named the deleted session included"
    );
}

/// The pre-write fence takes a trace only on its root: a child naming a deleted session, for a trace the store
/// holds no root of, is written and held for the sweep; for a trace whose stored root names the deleted session it
/// is fenced.
#[tokio::test]
async fn the_fence_takes_a_trace_on_its_root_only() {
    let store = temp_pipeline(false).await;
    let project = ProjectId::from("p");
    store
        .database
        .record_deleted_sessions(&project, &["session-d".to_string()])
        .await
        .unwrap();
    store
        .analytics
        .insert_spans(vec![
            span("rootless", "first", Some("root"), "session-d", 5),
            span("rooted", "root", None, "session-d", 0),
        ])
        .await
        .unwrap();

    let mut batch = vec![
        span("rootless", "second", Some("root"), "session-d", 6),
        span("rooted", "child", Some("root"), "session-d", 6),
    ];
    store
        .pipeline
        .drop_spans_for_deleted_sessions(&mut batch)
        .await
        .unwrap();
    assert_eq!(
        batch
            .iter()
            .map(|s| s.trace_id.as_str())
            .collect::<Vec<_>>(),
        ["rootless"],
        "only the trace whose stored root names the deleted session is fenced"
    );
    assert!(tombstoned(&store, "rooted").await);
    assert!(!tombstoned(&store, "rootless").await);
}

/// A root naming no session decides nothing: the trace is held for the grace as a rootless one is, since a span
/// that names a live session can still arrive and decide it.
#[tokio::test]
async fn a_root_naming_no_session_does_not_decide_its_trace() {
    let store = temp_pipeline(false).await;
    let project = ProjectId::from("p");
    store
        .database
        .record_deleted_sessions(&project, &["session-d".to_string()])
        .await
        .unwrap();
    let now = chrono::Utc::now().timestamp();
    let mut sessionless_root = received(span("t", "root", None, "unused", 0), now);
    sessionless_root.session_id = None;
    store
        .analytics
        .insert_spans(vec![
            sessionless_root,
            received(span("t", "child", Some("root"), "session-d", 5), now),
        ])
        .await
        .unwrap();

    sweep(&store, WEEK).await;
    assert!(
        !tombstoned(&store, "t").await,
        "held: nothing decides it yet"
    );
}

/// A root corrected later in the same batch to name no session decides nothing: the trace is not fenced.
#[tokio::test]
async fn a_root_corrected_to_no_session_in_its_batch_decides_nothing() {
    let store = temp_pipeline(false).await;
    let project = ProjectId::from("p");
    store
        .database
        .record_deleted_sessions(&project, &["session-d".to_string()])
        .await
        .unwrap();
    let mut corrected = span("t", "root", None, "unused", 0);
    corrected.session_id = None;
    let mut batch = vec![
        span("t", "root", None, "session-d", 0),
        corrected,
        span("t", "child", Some("root"), "session-d", 5),
    ];
    store
        .pipeline
        .drop_spans_for_deleted_sessions(&mut batch)
        .await
        .unwrap();
    assert_eq!(batch.len(), 3, "held for its root, not fenced");
    assert!(!tombstoned(&store, "t").await);
}

/// A stale redelivery of a root a stored correction superseded does not decide the trace: the fence lets it through
/// for the post-write check, which reads the winning revision.
#[tokio::test]
async fn a_stale_root_redelivery_does_not_decide_a_corrected_trace() {
    let store = temp_pipeline(false).await;
    let project = ProjectId::from("p");
    let mut cleared = received(span("t", "root", None, "unused", 0), 200);
    cleared.session_id = None;
    store
        .analytics
        .insert_spans(vec![
            received(span("t", "root", None, "session-d", 0), 100),
            received(span("t", "child", Some("root"), "session-l", 5), 150),
        ])
        .await
        .unwrap();
    store.analytics.insert_spans(vec![cleared]).await.unwrap();
    store
        .database
        .record_deleted_sessions(&project, &["session-d".to_string()])
        .await
        .unwrap();

    let mut batch = vec![received(span("t", "root", None, "session-d", 0), 100)];
    store
        .pipeline
        .drop_spans_for_deleted_sessions(&mut batch)
        .await
        .unwrap();
    assert_eq!(
        batch.len(),
        1,
        "not fenced on a root the store has superseded"
    );
    assert!(!tombstoned(&store, "t").await);
}
