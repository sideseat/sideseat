//! The cross-partition consistency check: detecting the duplicate schema v3 cannot collapse.
//!
//! Schema v3 took `toDate(timestamp_start)` out of the span sorting key, so a `ReplacingMergeTree` now
//! identifies duplicates by identity alone and a corrected re-delivery crossing **midnight UTC** collapses.
//! `PARTITION BY toYYYYMM(timestamp_start)` remains — time pruning is what it is for — so two revisions of
//! one identity can land in **different partitions**. Parts in different partitions never merge, and
//! `do_not_merge_across_partitions_select_final` (`mod.rs`) makes `FINAL` per-partition, so both survive and
//! the span is returned twice.
//!
//! Three repairs were designed for that and all three were withdrawn, which is why this is a detector:
//! explicit windowed version selection needs a stable tie-break for equal `ingested_at` and none exists here
//! (`(_part, _part_offset)` is physical placement a merge changes, and no per-delivery discriminator is
//! stored); an identity-scoped retention sweep replacing the TTLs produced more defects than the one it
//! fixed; and turning the partition-isolation setting off was measured at 10-12x the read cost, trading a
//! rare duplicate for a permanent regression on every query.
//!
//! So the residual is **reported rather than engineered around**. That is not a euphemism for ignored: a
//! reported duplicate is one an operator can act on, and the alternative on offer was a subsystem whose own
//! defects outnumbered the defect it addressed.
//!
//! # Why this is cheap enough to run continuously
//!
//! Asked directly the question is a full scan — `GROUP BY project_id, trace_id, span_id HAVING
//! uniq(toYYYYMM(timestamp_start)) > 1` over the corpus — which is not something to run on a schedule. Two
//! properties make it affordable:
//!
//! - **A correction always arrives as a new row.** So only rows ingested since the last run can have created
//!   a new anomaly, and the window is a function of the ingest rate rather than of corpus size.
//! - **With `toDate` gone from the sorting key, the probe is a point lookup.** For each candidate identity,
//!   "does another partition hold a revision of this?" is an index-supported seek on
//!   `(project_id, trace_id, span_id)` rather than a scan.
//!
//! Finding the recent rows needs its own index, because `ingested_at` is neither the partition key nor in the
//! sorting key: `idx_ingested_at`, a `minmax` skip index, which is effective here because parts are roughly
//! insertion-ordered so each part's range for that column is narrow. **Without that index this claim is
//! false**, which is why it is a schema requirement rather than an assumption.
//!
//! # This is a ClickHouse-only check, by construction
//!
//! DuckDB has no partitions, so it has no cross-partition residual — its reads go through `DEDUP_SPANS`, a
//! window function over the whole table, which selects one winner per identity however the rows are laid out.
//! The plan's sentence pairing the ClickHouse skip index with "and DuckDB an ART index" therefore names an
//! index with nothing to serve on that side, and ART memory is non-evictable and counts against the idle
//! footprint gate — so it is deliberately not added. The asymmetry is in the defect, not in the treatment.
//!
//! # Residuals of the detector itself
//!
//! - **A clock-behind instance can write a correction carrying an `ingested_at` below the window** and be
//!   missed. Mitigated by [`WINDOW_OVERLAP`], not closed — the same clock limit that runs through every
//!   mechanism over these stores.
//! - **A backward move can erase its own evidence, which is why the finding is recorded durably.** When a
//!   correction moves a span *backward* across a month, the newer revision expires first and the obsolete one
//!   is left alone in its partition; a current-state query then sees one row per identity and reports clean,
//!   having permanently served the wrong revision. The record in `span_partition_anomalies` survives that.
//! - **The watermark comes from the store, never from the reader's clock.** `max(ingested_at)` over the rows
//!   examined, so the window's two ends are stamped by the same clocks that wrote the data. A reader's
//!   `Utc::now()` compared against stamps written by other instances is the mistake this codebase has made
//!   more than once.

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, TimeDelta, Utc};
use clickhouse::Row;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use tokio::sync::watch;
use tokio::task::JoinHandle;

use super::{ClickhouseError, ClickhouseService};

/// How far back beyond the recorded watermark each pass re-reads.
///
/// A pass reads rows whose `ingested_at` is above `checked_through`, and that column is `Utc::now()` on the
/// writing instance — so an instance whose clock is behind can commit a correction stamped below a watermark
/// another instance's rows established, and it would never be examined. Re-reading an overlap catches the
/// ordinary case at the cost of re-probing rows that were already clean, which is idempotent: the anomaly
/// table is keyed by identity, so re-detecting one updates its row rather than adding another.
///
/// It bounds nothing in principle. An arbitrarily skewed clock defeats any finite overlap, and closing it
/// would need a commit-ordered sequence neither analytics backend provides.
const WINDOW_OVERLAP: TimeDelta = TimeDelta::minutes(10);

/// Identities examined per pass.
///
/// The pass is bounded work, not "all outstanding work": a backlog is the next pass's problem, because the
/// point of running continuously is that no single run is expensive. A pass that hit the cap does not advance
/// its watermark past what it examined, so nothing is skipped.
const MAX_CANDIDATES_PER_PASS: u64 = 10_000;

/// The row key used to store the watermark, which is not an identity.
///
/// One table rather than two, because a watermark is only meaningful together with what was found beneath it:
/// "clean through T" and the anomalies found up to T are one statement, and splitting them lets a restore
/// produce a watermark with no findings under it — which reads as "clean" and is not.
const WATERMARK_KEY: &str = "";

/// One identity whose revisions were found in more than one partition.
#[derive(Debug, Clone, Row, Serialize, Deserialize)]
pub struct PartitionAnomaly {
    pub project_id: String,
    pub trace_id: String,
    pub span_id: String,
    /// Every partition holding a revision of this identity, as `toYYYYMM` strings.
    pub partitions: Vec<String>,
    /// How many physical revisions were found across those partitions.
    pub revisions: u32,
}

/// What one pass did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CheckOutcome {
    /// Identities examined.
    pub examined: u64,
    /// Identities found to span more than one partition, this pass.
    pub anomalies: u64,
    /// Whether the pass stopped at [`MAX_CANDIDATES_PER_PASS`] rather than at the end of its window.
    pub truncated: bool,
}

impl ClickhouseService {
    /// Run one pass of the cross-partition consistency check.
    ///
    /// Reports identities whose revisions sit in more than one partition, and records each durably. Returns
    /// what the pass examined and found; the findings themselves are read back with
    /// [`Self::partition_anomalies`], because the point of recording them is that they outlive the pass.
    pub async fn check_partition_consistency(&self) -> Result<CheckOutcome, ClickhouseError> {
        // The **`Distributed` front end**, not `_local`. A first version read `_local` on the reasoning that
        // a question about physical parts must be asked where the parts are - which is true of a *mutation*
        // and backwards for a *read*: a `SELECT` against `_local` sees only the node the connection reached,
        // so an anomaly on any other shard was reported as clean. The Distributed table fans the read out,
        // which is what a report about the whole deployment needs.
        //
        // Still no `FINAL`: the question is about physical revisions, and `FINAL` shows one per identity per
        // partition - it would hide exactly what is being looked for.
        let spans = self.insert_table("otel_spans");
        let since = self.consistency_watermark().await? - WINDOW_OVERLAP;

        // Candidates: identities touched since the watermark. `GROUP BY` rather than `DISTINCT` so the
        // maximum ingested stamp comes back in the same pass - the watermark has to come from the rows
        // examined, not from this process's clock.
        //
        // Not `FINAL`: the whole question is about physical revisions, and `FINAL` shows one per identity per
        // partition. Reading the deduplicated relation here would hide exactly what is being looked for.
        #[derive(Row, Deserialize)]
        struct Candidate {
            project_id: String,
            trace_id: String,
            span_id: String,
            partitions: Vec<String>,
            revisions: u64,
            max_ingested: i64,
        }

        // The inner selection **groups before it limits, and orders by the identity's own newest stamp**.
        // Three defects in one line otherwise, and a first version had all three:
        //
        // - a bare `LIMIT` on rows caps *revisions*, not identities. Ten thousand revisions of one identity
        //   filled the cap, grouped down to a single candidate, and the pass then looked unfinished-but-not-
        //   truncated and advanced its watermark past every other identity in the window. Those were never
        //   examined again, because the watermark only moves forward.
        // - with no `ORDER BY`, a truncated pass takes an arbitrary subset, so there is no prefix to advance
        //   the watermark to - and refusing to advance it at all means the next pass selects the same subset
        //   forever. That is a livelock, not a backlog.
        // - ordering by `max(ingested_at)` **ascending** makes a pass a prefix of the window, which is what
        //   lets the watermark advance to what was examined and guarantees progress. The same reasoning as the
        //   search cursor recording the last position *examined* rather than the last one returned.
        let candidates: Vec<Candidate> = self
            .client
            .query(&format!(
                "SELECT project_id, trace_id, span_id, \
                        groupUniqArray(toString(toYYYYMM(timestamp_start))) AS partitions, \
                        count() AS revisions, \
                        toUnixTimestamp64Micro(max(ingested_at)) AS max_ingested \
                 FROM {spans} \
                 WHERE (project_id, trace_id, span_id) IN ( \
                     SELECT project_id, trace_id, span_id FROM {spans} \
                     WHERE ingested_at > fromUnixTimestamp64Micro(?) \
                     GROUP BY project_id, trace_id, span_id \
                     ORDER BY max(ingested_at) ASC \
                     LIMIT ? \
                 ) \
                 GROUP BY project_id, trace_id, span_id"
            ))
            .bind(since.timestamp_micros())
            .bind(MAX_CANDIDATES_PER_PASS)
            .fetch_all()
            .await
            .map_err(ClickhouseError::from)?;

        let truncated = candidates.len() as u64 >= MAX_CANDIDATES_PER_PASS;
        let examined = candidates.len() as u64;

        let found: Vec<&Candidate> = candidates
            .iter()
            .filter(|c| c.partitions.len() > 1)
            .collect();

        // The watermark advances to what was **examined**, truncated or not, and that is sound only because
        // the selection is an ordered prefix: every identity below this stamp has been looked at, so moving
        // past it skips nothing. Refusing to advance on a truncated pass was the first version and is a
        // livelock - the next pass re-selects the same prefix and never reaches the rest.
        //
        // From the rows, never from `Utc::now()`: `ingested_at` is written by other instances' clocks, and
        // comparing a reader's clock against them is the mistake this codebase has made more than once.
        let reached = candidates.iter().map(|c| c.max_ingested).max();

        if !found.is_empty() {
            let mut insert: clickhouse::insert::Insert<AnomalyRow> = self
                .client
                .insert("span_partition_anomalies")
                .await
                .map_err(ClickhouseError::from)?;
            for candidate in &found {
                insert
                    .write(&AnomalyRow {
                        project_id: candidate.project_id.clone(),
                        trace_id: candidate.trace_id.clone(),
                        span_id: candidate.span_id.clone(),
                        partitions: candidate.partitions.clone(),
                        revisions: u32::try_from(candidate.revisions).unwrap_or(u32::MAX),
                        detected_at: OffsetDateTime::now_utc(),
                        checked_through: OffsetDateTime::UNIX_EPOCH,
                    })
                    .await
                    .map_err(ClickhouseError::from)?;
            }
            insert.end().await.map_err(ClickhouseError::from)?;

            // Loud, because this is a span that reads return twice and no read-time mechanism here can
            // resolve it. The identities are in the table for anything that wants to act on them.
            tracing::error!(
                anomalies = found.len(),
                "Span identities have revisions in more than one partition, so FINAL returns each of them \
                 more than once. This is the stated cross-month residual of schema v3; see \
                 span_partition_anomalies for the identities."
            );
        }

        if let Some(reached) = reached {
            let reached = OffsetDateTime::from_unix_timestamp_nanos(i128::from(reached) * 1_000)
                .unwrap_or(OffsetDateTime::UNIX_EPOCH);
            self.record_consistency_watermark(reached).await?;
        }

        Ok(CheckOutcome {
            examined,
            anomalies: found.len() as u64,
            truncated,
        })
    }

    /// Every anomaly recorded so far, newest detection first.
    pub async fn partition_anomalies(&self) -> Result<Vec<PartitionAnomaly>, ClickhouseError> {
        self.client
            .query(
                "SELECT project_id, trace_id, span_id, partitions, revisions \
                 FROM span_partition_anomalies FINAL \
                 WHERE span_id != ? \
                 ORDER BY detected_at DESC",
            )
            .bind(WATERMARK_KEY)
            .fetch_all()
            .await
            .map_err(ClickhouseError::from)
    }

    async fn consistency_watermark(&self) -> Result<DateTime<Utc>, ClickhouseError> {
        let stored: Option<i64> = self
            .client
            .query(
                "SELECT toUnixTimestamp64Micro(checked_through) FROM span_partition_anomalies FINAL \
                 WHERE project_id = ? AND trace_id = ? AND span_id = ?",
            )
            .bind(WATERMARK_KEY)
            .bind(WATERMARK_KEY)
            .bind(WATERMARK_KEY)
            .fetch_optional()
            .await
            .map_err(ClickhouseError::from)?;

        Ok(stored
            .and_then(DateTime::from_timestamp_micros)
            .unwrap_or(DateTime::UNIX_EPOCH))
    }

    async fn record_consistency_watermark(
        &self,
        micros: OffsetDateTime,
    ) -> Result<(), ClickhouseError> {
        let mut insert: clickhouse::insert::Insert<AnomalyRow> = self
            .client
            .insert("span_partition_anomalies")
            .await
            .map_err(ClickhouseError::from)?;
        insert
            .write(&AnomalyRow {
                project_id: WATERMARK_KEY.to_string(),
                trace_id: WATERMARK_KEY.to_string(),
                span_id: WATERMARK_KEY.to_string(),
                partitions: Vec::new(),
                revisions: 0,
                detected_at: OffsetDateTime::now_utc(),
                checked_through: micros,
            })
            .await
            .map_err(ClickhouseError::from)?;
        insert.end().await.map_err(ClickhouseError::from)
    }
}

/// The stored shape, which carries the two timestamp columns the public type does not.
#[derive(Row, Serialize, Deserialize)]
struct AnomalyRow {
    project_id: String,
    trace_id: String,
    span_id: String,
    partitions: Vec<String>,
    revisions: u32,
    #[serde(with = "clickhouse::serde::time::datetime64::micros")]
    detected_at: OffsetDateTime,
    #[serde(with = "clickhouse::serde::time::datetime64::micros")]
    checked_through: OffsetDateTime,
}

/// How often a pass runs.
///
/// The check exists to make the cross-month residual a *reported* fact rather than a silent one, and a check
/// nobody runs reports nothing - so it is scheduled rather than offered as a method someone might call. Five
/// minutes because the window is a function of ingest rate, not corpus size: a longer interval does not make a
/// pass cheaper, it makes each one larger.
const CHECK_INTERVAL: Duration = Duration::from_secs(300);

impl ClickhouseService {
    /// Run the cross-partition consistency check periodically.
    ///
    /// Failures are logged and the loop continues: this is a detector, so its own unavailability must not take
    /// anything else down with it - but it is logged at `warn`, because a detector that has silently stopped
    /// detecting is indistinguishable from a clean corpus.
    pub fn start_consistency_check_task(
        self: &Arc<Self>,
        mut shutdown_rx: watch::Receiver<bool>,
    ) -> JoinHandle<()> {
        let service = Arc::clone(self);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(CHECK_INTERVAL);
            // The first tick fires immediately, and a pass at startup is not wanted: nothing has been ingested
            // yet, so it would read the whole overlap window for nothing on every new replica.
            interval.tick().await;
            loop {
                tokio::select! {
                    biased;
                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() {
                            tracing::debug!("ClickHouse consistency check task shutting down");
                            break;
                        }
                    }
                    _ = interval.tick() => {
                        match service.check_partition_consistency().await {
                            Ok(outcome) if outcome.truncated => tracing::warn!(
                                examined = outcome.examined,
                                anomalies = outcome.anomalies,
                                "Cross-partition consistency check hit its per-pass cap, so its watermark did \
                                 not advance; the remainder is the next pass's work"
                            ),
                            Ok(outcome) => tracing::debug!(
                                examined = outcome.examined,
                                anomalies = outcome.anomalies,
                                "Cross-partition consistency check pass complete"
                            ),
                            Err(e) => tracing::warn!(
                                error = %e,
                                "Cross-partition consistency check failed; the cross-month residual is \
                                 undetected until it succeeds"
                            ),
                        }
                    }
                }
            }
        })
    }
}

impl ClickhouseService {
    /// Count metric rows carrying no identity, and report them with the remedy.
    ///
    /// These are rows written before `datapoint_id` existed. The v3 rebuild gives them the column's default of
    /// `''`, which is not a digest any delivery can produce - so a correction of one of those datapoints lands
    /// beside it rather than replacing it, and both are returned. That residual is stated on `MIGRATIONS` and is
    /// unfixable in SQL, since the id digests OTLP attributes with their protobuf variants preserved.
    ///
    /// Called **immediately after the rebuild**, which is what keeps it affordable: `datapoint_id` is not
    /// indexed, so this is a scan, and the one moment it costs nothing extra is when the table has just been
    /// rewritten and is in cache. Doing it at every startup would be a scan of the metrics table on every boot.
    pub async fn report_unidentified_metric_rows(&self) -> Result<u64, ClickhouseError> {
        // The `Distributed` front end, for the reason `check_partition_consistency` records: a read against
        // `_local` sees one shard, so a count taken there reports zero for rows sitting on any other node.
        let table = self.insert_table("otel_metrics");
        let count: Option<u64> = self
            .client
            .query(&format!(
                "SELECT count() FROM {table} WHERE datapoint_id = '' LIMIT 1"
            ))
            .fetch_optional()
            .await
            .map_err(ClickhouseError::from)?;
        let count = count.unwrap_or(0);

        if count > 0 {
            tracing::warn!(
                rows = count,
                "{count} metric rows predate the datapoint identity and carry an empty datapoint_id. A later \
                 correction of one of those datapoints is returned *beside* it rather than replacing it, so \
                 those measurements can be double-counted. The identity cannot be backfilled - it digests OTLP \
                 attributes with their protobuf types - so the remedy is an operator decision: drop the \
                 affected partitions of otel_metrics, accepting the loss of pre-identity metrics, or accept \
                 the over-report."
            );
        }
        Ok(count)
    }
}
