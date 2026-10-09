use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use super::*;
use crate::DuckdbService;
use sideseat_core::storage::AppStorage;

fn span(span_id: &str, ingested_us: i64) -> NormalizedSpan {
    NormalizedSpan {
        project_id: Some("p".into()),
        trace_id: "t".into(),
        span_id: span_id.into(),
        ingested_at: chrono::DateTime::from_timestamp_micros(ingested_us),
        ..Default::default()
    }
}

fn stored(rowid: i64, ingested_us: i64, superseded_us: Option<i64>) -> StoredRevision {
    StoredRevision {
        rowid,
        ingested_us,
        superseded_us,
    }
}

fn identity(span_id: &str) -> SpanIdentity {
    ("p".into(), "t".into(), span_id.into())
}

#[test]
fn a_new_identity_is_its_own_winner_and_updates_nothing() {
    let plan = plan(&[span("a", 10)], &HashMap::new());
    assert_eq!(plan.new, vec![None]);
    assert!(plan.updates.is_empty());
}

#[test]
fn a_correction_supersedes_the_stored_winner_at_its_own_instant() {
    let stored = HashMap::from([(identity("a"), vec![stored(7, 10, None)])]);
    let plan = plan(&[span("a", 20)], &stored);
    assert_eq!(plan.new, vec![None]);
    assert_eq!(plan.updates, vec![(7, Some(20))]);
}

#[test]
fn an_older_revision_is_superseded_at_once_and_leaves_the_winner_alone() {
    let stored = HashMap::from([(identity("a"), vec![stored(7, 30, None)])]);
    let plan = plan(&[span("a", 20)], &stored);
    assert_eq!(plan.new, vec![Some(30)]);
    assert!(plan.updates.is_empty());
}

#[test]
fn a_revision_between_two_stored_ones_moves_the_earlier_ones_instant() {
    let stored = HashMap::from([(
        identity("a"),
        vec![stored(3, 10, Some(30)), stored(9, 30, None)],
    )]);
    let plan = plan(&[span("a", 20)], &stored);
    assert_eq!(plan.new, vec![Some(30)]);
    assert_eq!(plan.updates, vec![(3, Some(20))]);
}

#[test]
fn equal_instants_are_ordered_by_row_so_the_later_write_wins() {
    let stored = HashMap::from([(identity("a"), vec![stored(4, 10, None)])]);
    let plan = plan(&[span("a", 10), span("a", 10)], &stored);
    assert_eq!(plan.new, vec![Some(10), None]);
    assert_eq!(plan.updates, vec![(4, Some(10))]);
}

async fn service() -> (tempfile::TempDir, DuckdbService) {
    let temp = tempfile::TempDir::new().expect("temp dir");
    std::fs::create_dir_all(temp.path().join("duckdb")).expect("duckdb dir");
    let storage = AppStorage::init_for_test(temp.path().to_path_buf());
    let service = DuckdbService::init(&storage, Arc::new(crate::TestClock))
        .await
        .expect("service");
    (temp, service)
}

/// A small deterministic generator, so a failing history can be replayed from its seed.
struct Lcg(u64);

impl Lcg {
    fn below(&mut self, bound: u64) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) % bound
    }
}

type Winners = BTreeSet<(String, String, i64)>;

fn winners(conn: &duckdb::Connection, sql: &str) -> Winners {
    let mut statement = conn.prepare(sql).expect("prepare");
    statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .expect("query")
        .collect::<Result<_, _>>()
        .expect("rows")
}

/// The marker names exactly the revisions the window over every revision ranks first - now, and as of every
/// watermark - whatever order revisions arrive in, with equal instants, several revisions of one identity in a
/// batch, and identities deleted whole between writes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_marker_names_the_windows_winner_for_any_history() {
    for seed in 1..=12_u64 {
        let (_temp, service) = service().await;
        let mut random = Lcg(seed);
        let mut instants = BTreeSet::new();
        for _ in 0..40 {
            let batch: Vec<NormalizedSpan> = (0..1 + random.below(6))
                .map(|_| {
                    // A handful of identities and coarse instants, so ties and out-of-order arrivals are common.
                    let at = 1_000 * (1 + random.below(12) as i64);
                    instants.insert(at);
                    span(&format!("s{}", random.below(5)), at)
                })
                .collect();
            service
                .write(|conn| crate::repositories::span::insert_batch(conn, &batch))
                .expect("write");
            if random.below(10) == 0 {
                let doomed = format!("s{}", random.below(5));
                service
                    .write(|conn| {
                        conn.execute(
                            "DELETE FROM otel_spans WHERE project_id = 'p' AND trace_id = 't' AND span_id = ?",
                            [&doomed],
                        )
                        .map(|_| ())
                        .map_err(Into::into)
                    })
                    .expect("delete");
            }
        }
        let conn = service.conn();
        let window = |watermark: Option<i64>| {
            let before = watermark
                .map(|us| format!("WHERE epoch_us(ingested_at) < {us}"))
                .unwrap_or_default();
            winners(
                &conn,
                &format!(
                    "SELECT trace_id, span_id, rowid FROM otel_spans {before} \
                     QUALIFY ROW_NUMBER() OVER (PARTITION BY project_id, trace_id, span_id \
                     ORDER BY ingested_at DESC, rowid DESC) = 1"
                ),
            )
        };
        assert_eq!(
            winners(
                &conn,
                &format!(
                    "SELECT trace_id, span_id, rowid FROM otel_spans WHERE {}",
                    sideseat_query_sql::winners::DUCKDB_WINNER
                )
            ),
            window(None),
            "seed {seed}: current winners"
        );
        for watermark in instants.iter().flat_map(|&at| [at, at + 1]) {
            let condition = sideseat_query_sql::winners::DUCKDB_WINNER_AS_OF
                .replace("?::BIGINT", &watermark.to_string());
            assert_eq!(
                winners(
                    &conn,
                    &format!("SELECT trace_id, span_id, rowid FROM otel_spans WHERE {condition}")
                ),
                window(Some(watermark)),
                "seed {seed}: winners as of {watermark}"
            );
        }
        let chains: BTreeMap<String, usize> = winners(
            &conn,
            "SELECT trace_id, span_id, rowid FROM otel_spans WHERE superseded_at IS NULL",
        )
        .into_iter()
        .fold(BTreeMap::new(), |mut counts, (_, span_id, _)| {
            *counts.entry(span_id).or_default() += 1;
            counts
        });
        assert!(
            chains.values().all(|&count| count == 1),
            "seed {seed}: one winner per identity, found {chains:?}"
        );
    }
}

/// A write reads what it can change, not the identity's history: after fifty corrections a revision that arrives
/// in order reads the winner alone, and one that lands between two stored revisions reads from the one before it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_write_reads_only_the_revisions_it_can_change() {
    let (_temp, service) = service().await;
    for at in 1..=50_i64 {
        service
            .write(|conn| crate::repositories::span::insert_batch(conn, &[span("s", at * 1_000)]))
            .expect("write");
    }
    let conn = service.conn();
    let read = |since_us: i64| {
        let mut instants: Vec<i64> =
            crate::repositories::keyed::span_revisions(&conn, &[identity("s")], since_us)
                .expect("revisions")
                .remove(&identity("s"))
                .unwrap_or_default()
                .into_iter()
                .map(|revision| revision.ingested_us)
                .collect();
        instants.sort_unstable();
        instants
    };
    assert_eq!(read(60_000), vec![50_000]);
    assert_eq!(
        read(25_500),
        (25..=50).map(|at| at * 1_000).collect::<Vec<_>>()
    );
    assert_eq!(read(i64::MIN).len(), 50);
}
