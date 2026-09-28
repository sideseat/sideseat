use super::*;
use crate::DuckdbService;
use crate::repositories::query::DEDUP_SPANS;
use chrono::{DateTime, Utc};
use sideseat_core::storage::AppStorage;
use tempfile::TempDir;

fn test_now() -> DateTime<Utc> {
    DateTime::from_timestamp(1_700_000_000, 0).expect("valid fixture timestamp")
}

fn cleanup_by_time(
    conn: &Connection,
    minutes: u64,
    record_intent: CleanupRecorder<'_>,
) -> Result<(u64, HashMap<String, Vec<String>>), DuckdbError> {
    super::cleanup_by_time(conn, minutes, record_intent, test_now())
}

fn cleanup_by_count(
    conn: &Connection,
    max_spans: u64,
    record_intent: CleanupRecorder<'_>,
) -> Result<(u64, HashMap<String, Vec<String>>), DuckdbError> {
    super::cleanup_by_count(conn, max_spans, record_intent, test_now())
}

fn cleanup_by_count_within(
    conn: &Connection,
    max_spans: u64,
    identity_budget_for_cycle: i64,
    row_budget_for_cycle: u64,
    record_intent: CleanupRecorder<'_>,
) -> Result<(u64, HashMap<String, Vec<String>>), DuckdbError> {
    super::cleanup_by_count_within(
        conn,
        max_spans,
        identity_budget_for_cycle,
        row_budget_for_cycle,
        record_intent,
        test_now(),
    )
}

fn trim_project_to_limit(
    conn: &Connection,
    project_id: &str,
    overage: i64,
    batch_size: i64,
    row_budget: u64,
    record_intent: CleanupRecorder<'_>,
) -> Result<(u64, HashMap<String, Vec<String>>), DuckdbError> {
    super::trim_project_to_limit(
        conn,
        project_id,
        overage,
        batch_size,
        row_budget,
        record_intent,
        &super::ignore_pressure,
        test_now(),
    )
}

fn delete_oldest_spans_for_project(
    conn: &Connection,
    project_id: &str,
    limit: i64,
    row_budget: u64,
    allow_overshoot: bool,
    record_intent: CleanupRecorder<'_>,
) -> Result<BatchOutcome, DuckdbError> {
    super::delete_oldest_spans_for_project(
        conn,
        project_id,
        limit,
        row_budget,
        allow_overshoot,
        record_intent,
        &super::ignore_pressure,
        test_now(),
    )
}

fn cleanup_metrics_by_time(conn: &Connection, minutes: u64) -> Result<u64, DuckdbError> {
    super::cleanup_metrics_by_time(conn, minutes, test_now())
}

fn run_retention(
    conn: &Connection,
    config: &RetentionConfig,
    record_intent: CleanupRecorder<'_>,
) -> Result<RetentionResult, DuckdbError> {
    super::run_retention(conn, config, record_intent, test_now())
}

/// A recorder that records nothing, for the tests that are about deletion rather than about the intent.
///
/// Named rather than an inline closure at every call site, so `no_intent` reads as "this test does not
/// exercise the record" instead of as noise.
fn no_intent(_: &HashMap<String, Vec<String>>) -> Result<(), DuckdbError> {
    Ok(())
}

async fn create_test_service() -> (TempDir, DuckdbService) {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let duckdb_dir = temp_dir.path().join("duckdb");
    tokio::fs::create_dir_all(&duckdb_dir)
        .await
        .expect("Failed to create duckdb dir");
    let storage = AppStorage::init_for_test(temp_dir.path().to_path_buf());
    let service = DuckdbService::init(&storage, std::sync::Arc::new(crate::TestClock))
        .await
        .expect("Failed to init analytics service");
    (temp_dir, service)
}

fn insert_test_span(conn: &Connection, trace_id: &str, span_id: &str, timestamp: &str) {
    insert_span_for_project(conn, "default", trace_id, span_id, timestamp);
}

fn insert_span_for_project(
    conn: &Connection,
    project_id: &str,
    trace_id: &str,
    span_id: &str,
    timestamp: &str,
) {
    conn.execute(
        "INSERT INTO otel_spans (trace_id, span_id, span_name, timestamp_start, project_id)
             VALUES (?1, ?2, 'test', ?3, ?4)",
        [trace_id, span_id, timestamp, project_id],
    )
    .expect("Failed to insert test span");
}

/// A second delivery of an existing span. `otel_spans` is append-only, so this leaves both rows
/// and `ingested_at` decides which one reads see.
fn redeliver_span(
    conn: &Connection,
    project_id: &str,
    trace_id: &str,
    span_id: &str,
    timestamp: &str,
    ingested_at: &str,
) {
    conn.execute(
        "INSERT INTO otel_spans
                 (trace_id, span_id, span_name, timestamp_start, project_id, ingested_at)
             VALUES (?1, ?2, 'test', ?3, ?4, ?5)",
        [trace_id, span_id, timestamp, project_id, ingested_at],
    )
    .expect("Failed to re-deliver test span");
}

fn span_count(conn: &Connection, project_id: &str) -> i64 {
    conn.query_row(
        "SELECT COUNT(*) FROM otel_spans WHERE project_id = ?1",
        [project_id],
        |row| row.get(0),
    )
    .expect("Should query")
}

#[tokio::test]
async fn test_cleanup_by_time_empty_table() {
    let (_temp_dir, analytics) = create_test_service().await;
    let conn = analytics.conn();

    let (deleted, trace_ids) = cleanup_by_time(&conn, 60, &no_intent).expect("Should cleanup");
    assert_eq!(deleted, 0);
    assert!(trace_ids.is_empty());
}

#[tokio::test]
async fn test_cleanup_by_time_removes_old_spans() {
    let (_temp_dir, analytics) = create_test_service().await;
    let conn = analytics.conn();

    // Insert spans: 2 old, 1 recent
    insert_test_span(&conn, "trace1", "span1", "2020-01-01 00:00:00");
    insert_test_span(&conn, "trace2", "span2", "2020-01-02 00:00:00");
    let recent = Utc::now().format("%Y-%m-%d %H:%M:%S%.6f").to_string();
    insert_test_span(&conn, "trace3", "span3", &recent);

    // Cleanup spans older than 1 minute
    let (deleted, _trace_ids) = cleanup_by_time(&conn, 1, &no_intent).expect("Should cleanup");
    assert_eq!(deleted, 2);

    // Verify only recent span remains
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM otel_spans", [], |row| row.get(0))
        .expect("Should query");
    assert_eq!(count, 1);
}

#[tokio::test]
async fn test_cleanup_by_count_empty_table() {
    let (_temp_dir, analytics) = create_test_service().await;
    let conn = analytics.conn();

    let (deleted, trace_ids) = cleanup_by_count(&conn, 100, &no_intent).expect("Should cleanup");
    assert_eq!(deleted, 0);
    assert!(trace_ids.is_empty());
}

#[tokio::test]
async fn test_cleanup_by_count_under_limit() {
    let (_temp_dir, analytics) = create_test_service().await;
    let conn = analytics.conn();

    // Insert a few spans (under the limit)
    insert_test_span(&conn, "trace1", "span1", "2020-01-01 00:00:00");
    insert_test_span(&conn, "trace2", "span2", "2020-01-02 00:00:00");

    // 100 span limit - should not delete anything (only 2 spans)
    let (deleted, trace_ids) = cleanup_by_count(&conn, 100, &no_intent).expect("Should cleanup");
    assert_eq!(deleted, 0);
    assert!(trace_ids.is_empty());

    // Verify spans still exist
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM otel_spans", [], |row| row.get(0))
        .expect("Should query");
    assert_eq!(count, 2);
}

#[tokio::test]
async fn test_cleanup_by_count_exceeds_limit() {
    let (_temp_dir, analytics) = create_test_service().await;
    let conn = analytics.conn();

    // Insert 5 spans
    insert_test_span(&conn, "trace1", "span1", "2020-01-01 00:00:00");
    insert_test_span(&conn, "trace2", "span2", "2020-01-02 00:00:00");
    insert_test_span(&conn, "trace3", "span3", "2020-01-03 00:00:00");
    insert_test_span(&conn, "trace4", "span4", "2020-01-04 00:00:00");
    insert_test_span(&conn, "trace5", "span5", "2020-01-05 00:00:00");

    // Limit to 2 spans - should delete 3 oldest
    let (deleted, _trace_ids) = cleanup_by_count(&conn, 2, &no_intent).expect("Should cleanup");
    assert_eq!(deleted, 3);

    // Verify only 2 spans remain (the newest ones)
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM otel_spans", [], |row| row.get(0))
        .expect("Should query");
    assert_eq!(count, 2);

    // Verify the newest spans were kept
    let trace4_exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM otel_spans WHERE trace_id = 'trace4'",
            [],
            |row| row.get(0),
        )
        .expect("Should query");
    assert_eq!(trace4_exists, 1);

    let trace5_exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM otel_spans WHERE trace_id = 'trace5'",
            [],
            |row| row.get(0),
        )
        .expect("Should query");
    assert_eq!(trace5_exists, 1);
}

#[tokio::test]
async fn test_cleanup_preserves_recent_spans() {
    let (_temp_dir, analytics) = create_test_service().await;
    let conn = analytics.conn();

    // Insert only recent spans
    let now = Utc::now();
    let recent1 = now.format("%Y-%m-%d %H:%M:%S%.6f").to_string();
    let recent2 = (now - TimeDelta::seconds(30))
        .format("%Y-%m-%d %H:%M:%S%.6f")
        .to_string();

    insert_test_span(&conn, "trace1", "span1", &recent1);
    insert_test_span(&conn, "trace2", "span2", &recent2);

    // Cleanup with 1 minute retention - should preserve both
    let (deleted, trace_ids) = cleanup_by_time(&conn, 1, &no_intent).expect("Should cleanup");
    assert_eq!(deleted, 0);
    assert!(trace_ids.is_empty());

    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM otel_spans", [], |row| row.get(0))
        .expect("Should query");
    assert_eq!(count, 2);
}

#[tokio::test]
async fn test_cleanup_deletes_oldest_first() {
    let (_temp_dir, analytics) = create_test_service().await;
    let conn = analytics.conn();

    // Insert spans with different timestamps
    insert_test_span(&conn, "oldest", "span1", "2020-01-01 00:00:00");
    insert_test_span(&conn, "middle", "span2", "2020-06-01 00:00:00");
    insert_test_span(&conn, "newest", "span3", "2020-12-01 00:00:00");

    // Use the batch primitive directly to verify ordering
    let batch = delete_oldest_spans_for_project(&conn, "default", 1, u64::MAX, true, &no_intent)
        .expect("Should delete");
    assert_eq!(batch.rows, 1);

    // Verify oldest was deleted
    let oldest_exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM otel_spans WHERE trace_id = 'oldest'",
            [],
            |row| row.get(0),
        )
        .expect("Should query");
    assert_eq!(oldest_exists, 0);

    // Middle and newest should still exist
    let remaining: i64 = conn
        .query_row("SELECT COUNT(*) FROM otel_spans", [], |row| row.get(0))
        .expect("Should query");
    assert_eq!(remaining, 2);
}

#[tokio::test]
async fn test_cleanup_mixed_old_and_new_spans() {
    let (_temp_dir, analytics) = create_test_service().await;
    let conn = analytics.conn();

    // Insert mix of old and new spans
    insert_test_span(&conn, "old1", "span1", "2020-01-01 00:00:00");
    insert_test_span(&conn, "old2", "span2", "2020-01-02 00:00:00");
    insert_test_span(&conn, "old3", "span3", "2020-01-03 00:00:00");

    let now = Utc::now();
    insert_test_span(
        &conn,
        "new1",
        "span4",
        &now.format("%Y-%m-%d %H:%M:%S%.6f").to_string(),
    );
    insert_test_span(
        &conn,
        "new2",
        "span5",
        &(now - TimeDelta::seconds(10))
            .format("%Y-%m-%d %H:%M:%S%.6f")
            .to_string(),
    );

    // Cleanup old spans
    let (deleted, _trace_ids) = cleanup_by_time(&conn, 1, &no_intent).expect("Should cleanup");
    assert_eq!(deleted, 3);

    // Only new spans should remain
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM otel_spans", [], |row| row.get(0))
        .expect("Should query");
    assert_eq!(count, 2);
}

// ========================================================================
// TENANT ISOLATION AND REVISION AWARENESS
//
// Every test below fails against the pre-fix implementation. The existing cases above cannot:
// they use one project and one revision per span, which is exactly the shape in which both
// defects are invisible.
// ========================================================================

/// A trace id comes from the client, so two projects can present the same one - and a span id is
/// unique only within a trace. Expiring one tenant's span must not touch the other's.
#[tokio::test]
async fn expiring_one_project_leaves_a_colliding_span_of_another_project() {
    let (_temp_dir, analytics) = create_test_service().await;
    let conn = analytics.conn();

    // Same trace id AND same span id in two projects: the realistic collision, since both
    // values are client-supplied.
    insert_span_for_project(
        &conn,
        "alice",
        "shared-trace",
        "shared-span",
        "2020-01-01 00:00:00",
    );
    let recent = Utc::now().format("%Y-%m-%d %H:%M:%S%.6f").to_string();
    insert_span_for_project(&conn, "bob", "shared-trace", "shared-span", &recent);

    let (deleted, trace_ids) = cleanup_by_time(&conn, 1, &no_intent).expect("Should cleanup");
    assert_eq!(deleted, 1, "only alice's expired span should go");

    assert_eq!(
        span_count(&conn, "alice"),
        0,
        "alice's expired span is deleted"
    );
    assert_eq!(
        span_count(&conn, "bob"),
        1,
        "bob's recent span must survive: a batch keyed only by (trace_id, span_id) deleted it"
    );

    // And the cleanup list must attribute the trace to alice alone, or bob's files are reclaimed.
    assert_eq!(trace_ids.get("alice").map(Vec::len), Some(1));
    assert!(
        !trace_ids.contains_key("bob"),
        "bob's trace must not be handed to file cleanup"
    );
}

/// `max_spans` is a per-project limit. A shared budget let one busy tenant's overage delete a
/// quiet tenant's data, because the globally oldest rows are not necessarily the offender's.
#[tokio::test]
async fn max_spans_is_per_project_and_spends_only_the_offender() {
    let (_temp_dir, analytics) = create_test_service().await;
    let conn = analytics.conn();

    // The *innocent* project owns the globally oldest spans. Without that arrangement a global
    // algorithm passes by luck, deleting the offender's rows because they happen to be oldest.
    // At the limit, not over it, so the only reason to touch these is a shared budget.
    for i in 0..2 {
        insert_span_for_project(
            &conn,
            "quiet",
            &format!("q{i}"),
            &format!("qs{i}"),
            &format!("2019-01-0{} 00:00:00", i + 1),
        );
    }
    for i in 0..5 {
        insert_span_for_project(
            &conn,
            "noisy",
            &format!("n{i}"),
            &format!("ns{i}"),
            &format!("2021-01-0{} 00:00:00", i + 1),
        );
    }

    let (deleted, _) = cleanup_by_count(&conn, 2, &no_intent).expect("Should cleanup");

    assert_eq!(
        span_count(&conn, "noisy"),
        2,
        "the over-limit project must end at exactly max_spans"
    );
    assert_eq!(
        span_count(&conn, "quiet"),
        2,
        "the at-limit project is untouched even though it owns the globally oldest spans"
    );
    assert_eq!(deleted, 3, "only noisy's three excess spans");
}

/// One cycle's work is bounded across every project, not merely within each.
///
/// `MAX_COUNT_CLEANUP_BATCHES` bounds one project, so with no cycle-wide ceiling the total was that
/// bound times the number of over-limit projects - unbounded. That matters because the whole pass holds
/// the single DuckDB connection, which is the same one writes take, and accumulates every deleted trace
/// id in memory for the file sweep.
///
/// The remainder is not lost: the deferred projects are still over their limit, so the next cycle finds
/// them. This asserts both halves - the cycle stops, *and* a second cycle finishes the job - because a
/// budget that dropped the remainder would pass the first assertion alone.
#[tokio::test]
async fn count_retention_is_bounded_per_cycle_across_projects_not_only_per_project() {
    let (_temp_dir, analytics) = create_test_service().await;
    let conn = analytics.conn();

    // Three projects, each two identities over a limit of one: six identities of work in total.
    for project in ["a", "b", "c"] {
        for i in 0..3 {
            insert_span_for_project(
                &conn,
                project,
                &format!("{project}t{i}"),
                &format!("{project}s{i}"),
                &format!("2021-01-0{} 00:00:00", i + 1),
            );
        }
    }

    // A budget of three identities cannot cover all six, so at least one project must be deferred.
    let (deleted, _) =
        cleanup_by_count_within(&conn, 1, 3, u64::MAX, &no_intent).expect("Should cleanup");
    assert_eq!(
        deleted, 3,
        "the cycle stops at its budget rather than at the work available"
    );

    let remaining: i64 = ["a", "b", "c"].iter().map(|p| span_count(&conn, p)).sum();
    assert_eq!(
        remaining, 6,
        "nine identities less the three the budget allowed"
    );
    assert!(
        ["a", "b", "c"].iter().any(|p| span_count(&conn, p) > 1),
        "at least one project is deferred, still over its limit"
    );

    // The remainder is next cycle's work, not lost work.
    let (deleted_again, _) =
        cleanup_by_count_within(&conn, 1, 3, u64::MAX, &no_intent).expect("Should cleanup");
    assert_eq!(
        deleted_again, 3,
        "the second cycle takes the deferred remainder"
    );
    for project in ["a", "b", "c"] {
        assert_eq!(
            span_count(&conn, project),
            1,
            "{project} reaches its limit once the cycles have run"
        );
    }
}

/// The trim is bounded by **rows** as well as identities, because those are different amounts of work.
///
/// Selection is by winning identity but the delete removes every revision of each selected identity. So an
/// identity budget alone is not a bound on work: a project one identity over its limit whose oldest identity
/// carries a large number of revisions is charged `1` against that budget and does that many rows of work,
/// holding the single DuckDB connection - the same one writes take - for the duration. The cycle reported
/// itself bounded and was not.
///
/// The fixture makes the two numbers diverge, which is exactly what the identity-budget test does not: every
/// identity carries four revisions, so five identities are twenty rows behind five winners.
///
/// **What is asserted is the bound the mechanism actually delivers**, not a stronger one. The row budget is
/// checked between batches and the batch's identity limit is scaled by the revision ratio measured from the
/// previous batch - so the *first* batch is unbounded (no ratio exists yet) and the overshoot is at most one
/// batch. Asserting an exact row count would be asserting that an identity can be split, which it cannot:
/// dropping some revisions leaves the identity in place and makes no progress, dropping the winner promotes
/// an obsolete revision. So the assertion is "it stopped early", with the residual named.
#[tokio::test]
async fn count_retention_is_bounded_by_rows_not_only_by_identities() {
    let (_temp_dir, analytics) = create_test_service().await;
    let conn = analytics.conn();

    // Five identities, four revisions each: 20 physical rows behind 5 winners.
    for i in 0..5 {
        let ts = format!("2020-01-0{} 00:00:00", i + 1);
        for rev in 0..4 {
            redeliver_span(
                &conn,
                "default",
                &format!("t{i}"),
                &format!("s{i}"),
                &ts,
                &format!("2020-0{}-01 00:00:00", rev + 1),
            );
        }
    }

    // Batch size 1, so batches are per identity and the ratio is learned after the first. A row budget of 5
    // then permits the first identity (4 rows), finds 4 < 5 and takes a second (8 rows), and stops - rather
    // than taking all four identities and 16 rows, which is what an identity-only budget allows.
    let (deleted, _) =
        trim_project_to_limit(&conn, "default", 4, 1, 5, &no_intent).expect("Should trim");

    assert!(
        deleted < 16,
        "the trim deleted every revision of every over-limit identity ({deleted} rows) - the row budget \
             is not bounding anything, so one identity's revision depth is unbounded work inside a cycle that \
             reports itself bounded"
    );
    assert!(
        deleted >= 4,
        "it must still make progress, got {deleted} rows"
    );

    let winners: i64 = conn
        .query_row(
            &format!("SELECT COUNT(*) FROM {DEDUP_SPANS} WHERE project_id = 'default'"),
            [],
            |row| row.get(0),
        )
        .expect("Should query");
    assert!(
        winners > 1,
        "identities remain, still over the limit, and are the next cycle's work - got {winners}"
    );
}

/// The cleanup intent is recorded **before** the spans are deleted, which is what survives a crash.
///
/// The recorder observes both selected identities and the still-present source rows, proving the durable
/// recovery record precedes the destructive step.
#[tokio::test]
async fn the_cleanup_intent_is_recorded_before_the_spans_are_deleted() {
    let (_temp_dir, analytics) = create_test_service().await;
    let conn = analytics.conn();

    insert_test_span(&conn, "t1", "s1", "2020-01-01 00:00:00");
    insert_test_span(&conn, "t2", "s2", "2020-01-02 00:00:00");

    // What the recorder saw, and how many spans still existed when it saw it.
    let observed: std::sync::Mutex<Vec<(String, usize, i64)>> = std::sync::Mutex::new(Vec::new());
    let recorder = |by_project: &HashMap<String, Vec<String>>| {
        let remaining: i64 = conn
            .query_row("SELECT COUNT(*) FROM otel_spans", [], |row| row.get(0))
            .expect("count spans");
        let mut seen = observed.lock().unwrap();
        for (project_id, trace_ids) in by_project {
            seen.push((project_id.clone(), trace_ids.len(), remaining));
        }
        Ok(())
    };

    let (deleted, trace_ids) = cleanup_by_time(&conn, 1, &recorder).expect("Should cleanup");
    assert_eq!(deleted, 2, "both expired spans are deleted");

    let seen = observed.into_inner().unwrap();
    assert_eq!(seen.len(), 1, "one project was recorded, got {seen:?}");
    assert_eq!(seen[0].0, "default");
    assert_eq!(seen[0].1, 2, "both traces were handed to the recorder");
    assert_eq!(
        seen[0].2, 2,
        "the recorder ran with the spans still present, so the record is durable before the delete. It \
             saw {} spans, which means recording happens after the deletion and a crash in between loses the \
             cleanup entirely",
        seen[0].2
    );

    // And what it was handed is what the caller gets, so the record and the work cannot disagree.
    assert_eq!(trace_ids.get("default").map(|t| t.len()), Some(2));
}

/// A recorder that fails stops the deletion, because deleting without a record is the loss.
///
/// The two failure directions are not symmetric and this is the one that matters: if the record commits and
/// the delete does not, the cleanup is a harmless no-op; if the delete commits and the record does not, the
/// spans are gone and nothing knows their files are owed. So a failure here must abort rather than proceed.
#[tokio::test]
async fn a_failed_intent_record_leaves_the_spans_in_place() {
    let (_temp_dir, analytics) = create_test_service().await;
    let conn = analytics.conn();

    insert_test_span(&conn, "t1", "s1", "2020-01-01 00:00:00");

    let failing = |_: &HashMap<String, Vec<String>>| {
        Err(DuckdbError::Io(std::io::Error::other(
            "the transactional store is unavailable",
        )))
    };
    let result = cleanup_by_time(&conn, 1, &failing);
    assert!(result.is_err(), "a failed record must fail the batch");

    let remaining: i64 = conn
        .query_row("SELECT COUNT(*) FROM otel_spans", [], |row| row.get(0))
        .expect("count spans");
    assert_eq!(
        remaining, 1,
        "the span was deleted although its cleanup could not be recorded, so its files are orphaned with \
             nothing able to find them"
    );
}

/// The row bound holds when revision depth is **not uniform**, which is when an average lies.
///
/// The bound was first implemented by scaling the next batch's identity limit from the previous batch's
/// average revisions-per-identity. That is not a bound: an average describes what has happened, not what
/// the next batch contains. A batch of one-revision identities followed by a batch of hundred-revision ones
/// overshoots by two orders of magnitude, and the earlier test could not see it because every identity in
/// its fixture carried the same depth.
///
/// This fixture is deliberately skewed - shallow identities first, then a deep one - so an average taken
/// over the shallow ones is wrong about the deep one by 20x. The bound is now computed inside the delete
/// statement as a running revision total, so it does not depend on any prediction.
#[tokio::test]
async fn the_row_bound_holds_when_revision_depth_is_not_uniform() {
    let (_temp_dir, analytics) = create_test_service().await;
    let conn = analytics.conn();

    // Four shallow identities (one revision each), then one carrying twenty.
    for i in 0..4 {
        insert_span_for_project(
            &conn,
            "default",
            &format!("shallow{i}"),
            &format!("s{i}"),
            &format!("2020-01-0{} 00:00:00", i + 1),
        );
    }
    for rev in 0..20 {
        redeliver_span(
            &conn,
            "default",
            "deep",
            "deep-span",
            "2020-01-05 00:00:00",
            &format!("2020-01-{:02} 00:00:00", rev + 1),
        );
    }

    // Every identity is over a limit of zero, so nothing but the budget decides where it stops. A budget of
    // six covers the four shallow identities (4 rows) and must **not** reach the deep one, which would take
    // the total to 24.
    let (deleted, _) =
        trim_project_to_limit(&conn, "default", 5, 100, 6, &no_intent).expect("Should trim");

    assert!(
        deleted <= 6,
        "the trim deleted {deleted} rows against a budget of 6 - the bound is predicted from an average \
             rather than computed, so a batch of deep identities blows through it"
    );
    assert!(
        deleted >= 4,
        "it must still make progress on what fits, got {deleted}"
    );

    // The deep identity survives to the next cycle rather than being dropped.
    let deep_rows: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM otel_spans WHERE project_id = 'default' AND trace_id = 'deep'",
            [],
            |row| row.get(0),
        )
        .expect("Should query");
    assert_eq!(
        deep_rows, 20,
        "the identity that did not fit the budget is left intact for the next cycle"
    );
}

/// The overshoot escape fires **once per trim**, not once per batch.
///
/// `allow_overshoot` exists so a deep identity larger than the whole budget still makes progress, and it is
/// passed only when nothing has been deleted yet. Passed on every batch it re-opens the hole it plugs: a
/// batch fills the budget with shallow identities, the next batch finds a deep one at rank 1 and takes it
/// unconditionally, and the trim overshoots by that identity's entire depth *after* having already made
/// progress.
///
/// The fixture needs both a **multi-batch** trim and **skewed** depth, which is what the two earlier tests
/// each half-had: shallow identities first so batch one succeeds within budget, then a deep one so batch
/// two's rank-1 escape would be visible.
#[tokio::test]
async fn the_overshoot_escape_applies_once_per_trim_not_once_per_batch() {
    let (_temp_dir, analytics) = create_test_service().await;
    let conn = analytics.conn();

    // Four shallow identities (one row each), then one carrying twenty.
    for i in 0..4 {
        insert_span_for_project(
            &conn,
            "default",
            &format!("shallow{i}"),
            &format!("s{i}"),
            &format!("2020-01-0{} 00:00:00", i + 1),
        );
    }
    for rev in 0..20 {
        redeliver_span(
            &conn,
            "default",
            "deep",
            "deep-span",
            "2020-01-05 00:00:00",
            &format!("2020-01-{:02} 00:00:00", rev + 1),
        );
    }

    // Batch size 2, so the trim takes two batches to reach the deep identity: batch one takes two shallow
    // identities (2 rows), batch two takes the other two (4 rows total), batch three reaches the deep one.
    // With a budget of 6 the deep identity does not fit, and because progress has already been made the
    // escape must not apply.
    let (deleted, _) =
        trim_project_to_limit(&conn, "default", 5, 2, 6, &no_intent).expect("Should trim");

    assert!(
        deleted <= 6,
        "the trim deleted {deleted} rows against a budget of 6 - the rank-1 escape fired on a later batch, \
             so a deep identity is taken unconditionally even once the trim has made progress"
    );

    let deep_rows: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM otel_spans WHERE project_id = 'default' AND trace_id = 'deep'",
            [],
            |row| row.get(0),
        )
        .expect("Should query");
    assert_eq!(
        deep_rows, 20,
        "the deep identity is left for the next cycle rather than taken past the budget"
    );
}

/// A single identity larger than the whole budget is still deleted, because it cannot be split.
///
/// This is the stated residual, asserted rather than described: with a budget of one row and an identity
/// carrying twenty, the sweep must take all twenty. Dropping some of an identity's revisions would leave
/// the identity in place and make no progress; dropping its winner would promote an obsolete revision,
/// which is corruption rather than slow retention. So the batch overshoots by exactly one identity, and
/// a mechanism that instead made *no* progress here would be worse.
#[tokio::test]
async fn one_identity_larger_than_the_budget_is_still_deleted() {
    let (_temp_dir, analytics) = create_test_service().await;
    let conn = analytics.conn();

    for rev in 0..20 {
        redeliver_span(
            &conn,
            "default",
            "deep",
            "deep-span",
            "2020-01-05 00:00:00",
            &format!("2020-01-{:02} 00:00:00", rev + 1),
        );
    }

    let (deleted, _) =
        trim_project_to_limit(&conn, "default", 1, 100, 1, &no_intent).expect("Should trim");

    assert_eq!(
        deleted, 20,
        "an identity cannot be split, so a budget of 1 still takes its 20 revisions - the alternative is \
             a sweep that never makes progress on a deep identity"
    );
}

/// Retention selects from the deduplicated relation. Selecting raw rows let an expired *old*
/// revision nominate an identity whose winning correction is recent - and the delete, which
/// removes every revision of an identity, took the correction with it.
#[tokio::test]
async fn an_expired_revision_does_not_delete_its_recent_correction() {
    let (_temp_dir, analytics) = create_test_service().await;
    let conn = analytics.conn();

    let now = Utc::now();
    let recent = now.format("%Y-%m-%d %H:%M:%S%.6f").to_string();

    // First delivery is expired; the correction moves the span into the retention window.
    redeliver_span(
        &conn,
        "default",
        "trace1",
        "span1",
        "2020-01-01 00:00:00",
        "2020-01-01 00:00:00",
    );
    redeliver_span(&conn, "default", "trace1", "span1", &recent, &recent);

    let (deleted, trace_ids) = cleanup_by_time(&conn, 1, &no_intent).expect("Should cleanup");

    assert_eq!(
        deleted, 0,
        "the winning revision is recent, so nothing is expired"
    );
    assert_eq!(
        span_count(&conn, "default"),
        2,
        "both revisions survive: deleting the identity would have destroyed the correction"
    );
    assert!(trace_ids.is_empty());
}

/// Counting raw rows double-counts a corrected span, so a project one identity over the limit
/// looked two over, both were deleted, and the sweep finished below the limit.
#[tokio::test]
async fn count_retention_counts_identities_not_revisions() {
    let (_temp_dir, analytics) = create_test_service().await;
    let conn = analytics.conn();

    // Two identities, one of which has been re-delivered: three physical rows, two winners.
    insert_span_for_project(&conn, "default", "t1", "s1", "2020-01-01 00:00:00");
    redeliver_span(
        &conn,
        "default",
        "t1",
        "s1",
        "2020-01-01 00:00:00",
        "2020-06-01 00:00:00",
    );
    insert_span_for_project(&conn, "default", "t2", "s2", "2020-01-02 00:00:00");

    // A limit of 2 is satisfied by two winners: nothing should be deleted.
    let (deleted, trace_ids) = cleanup_by_count(&conn, 2, &no_intent).expect("Should cleanup");
    assert_eq!(
        deleted, 0,
        "two winning identities are within a limit of two, whatever the revision count"
    );
    assert_eq!(span_count(&conn, "default"), 3);
    assert!(trace_ids.is_empty());
}
