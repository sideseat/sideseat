//! The cross-partition consistency check: detecting the duplicate schema v3 cannot collapse.
//!
//! Schema v3 took `toDate(timestamp_start)` out of the span sorting key, so a `ReplacingMergeTree` now
//! identifies duplicates by identity alone and a corrected re-delivery crossing **midnight UTC** collapses.
//! `PARTITION BY toYYYYMM(timestamp_start)` remains — time pruning is what it is for — so two revisions of
//! one identity can land in **different partitions**. Parts in different partitions never merge, and
//! `do_not_merge_across_partitions_select_final` (`mod.rs`) makes `FINAL` per-partition, so both survive and
//! the span is returned twice.
//!
//! The store has no stable tie-break for equal `ingested_at`: `(_part, _part_offset)` is physical placement
//! that merges can change, and no per-delivery discriminator is stored. Cross-partition `FINAL` is also too
//! expensive for every query. The residual is therefore detected and recorded for operators rather than
//! resolved with a non-deterministic winner.
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
//! DuckDB has no partitions, so it has no cross-partition residual. Its reads go through `DEDUP_SPANS`, a
//! window function over the whole table that selects one winner per identity. An additional DuckDB ART index
//! would serve no detector query, while its non-evictable memory would count against the idle footprint gate.
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
//! - **The watermark comes from the store, never from the reader's clock.** It is
//!   `max(ingested_at)` over the rows examined, so both ends of the window use writer timestamps.

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
/// A backlog is the next pass's problem: the point of running continuously is that no single run is expensive.
///
/// **This bounds identities returned, not rows scanned**, and the difference is real rather than pedantic. One
/// recently-touched identity carrying five million revisions is a single candidate, and grouping it still reads
/// those five million rows - so a pass's *work* is bounded by the revision depth behind its window, which
/// nothing here caps. What the skip index buys is that the window is a slice of recent ingest rather than the
/// corpus; what it does not buy is a bound on how deep that slice is. Stated rather than implied, because a cap
/// that is described as bounding work and does not is worse than no cap at all.
const MAX_CANDIDATES_PER_PASS: u64 = 10_000;

/// Reserved row key for the watermark, which is not an identity.
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
        let client = self.maintenance_client();
        // Read through the `Distributed` front end. `_local` would inspect only the node reached by this
        // connection, while the report covers the whole deployment.
        //
        // Still no `FINAL`: the question is about physical revisions, and `FINAL` shows one per identity per
        // partition - it would hide exactly what is being looked for.
        let spans = self.insert_table("otel_spans");
        // Subtract the overlap only after a complete pass. A truncated pass must resume at its watermark;
        // subtracting the overlap would repeatedly select the same ordered prefix and never reach the backlog.
        let (watermark, behind) = self.consistency_watermark().await?;
        let since = if behind {
            watermark
        } else {
            watermark - WINDOW_OVERLAP
        };

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

        // `GLOBAL IN`, not `IN`. Both sides read the `Distributed` table, and ClickHouse's
        // `distributed_product_mode` defaults to `deny` - so a plain `IN` here is refused outright on a
        // multi-shard cluster with "Double-distributed IN/JOIN subqueries is denied", meaning the scheduled
        // detector fails on every pass instead of reporting anything. `GLOBAL IN` evaluates the subquery once
        // on the initiator and ships the result, which is both permitted and what the semantics need: the
        // candidate set must be the same on every shard, not each shard's local view.
        //
        // The inner selection groups before limiting so the cap counts identities rather than revisions.
        // Ordering identities by their newest ingest stamp ascending makes a truncated pass a stable prefix,
        // allowing the watermark to advance while preserving eventual progress through the backlog.
        let candidates: Vec<Candidate> = client
            .query(&format!(
                "SELECT project_id, trace_id, span_id, \
                        groupUniqArray(toString(toYYYYMM(timestamp_start))) AS partitions, \
                        count() AS revisions, \
                        toUnixTimestamp64Micro(max(ingested_at)) AS max_ingested \
                 FROM {spans} \
                 WHERE (project_id, trace_id, span_id) GLOBAL IN ( \
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

        // Advance to the newest stamp actually examined, even for a truncated pass. The ordered-prefix
        // selection guarantees every identity below that stamp was considered. Use row timestamps rather than
        // this reader's clock so the watermark stays in the writers' time domain.
        let reached = candidates.iter().map(|c| c.max_ingested).max();

        if !found.is_empty() {
            let mut insert: clickhouse::insert::Insert<AnomalyRow> = client
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
            self.record_consistency_watermark(reached, truncated)
                .await?;
        }

        Ok(CheckOutcome {
            examined,
            anomalies: found.len() as u64,
            truncated,
        })
    }

    /// Every anomaly recorded so far, newest detection first.
    pub async fn partition_anomalies(&self) -> Result<Vec<PartitionAnomaly>, ClickhouseError> {
        self.maintenance_client()
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

    /// The watermark, and whether the pass that set it was truncated.
    ///
    /// The second value is what stops the overlap turning a capped pass into a livelock, so it is stored rather
    /// than inferred: `revisions` on the watermark row carries it, a column that is otherwise meaningless
    /// there.
    async fn consistency_watermark(&self) -> Result<(DateTime<Utc>, bool), ClickhouseError> {
        let client = self.maintenance_client();
        let stored: Option<(i64, u32)> = client
            .query(
                "SELECT toUnixTimestamp64Micro(checked_through), revisions \
                 FROM span_partition_anomalies FINAL \
                 WHERE project_id = ? AND trace_id = ? AND span_id = ?",
            )
            .bind(WATERMARK_KEY)
            .bind(WATERMARK_KEY)
            .bind(WATERMARK_KEY)
            .fetch_optional()
            .await
            .map_err(ClickhouseError::from)?;

        Ok(match stored {
            Some((micros, behind)) => (
                DateTime::from_timestamp_micros(micros).unwrap_or(DateTime::UNIX_EPOCH),
                behind == 1,
            ),
            None => (DateTime::UNIX_EPOCH, false),
        })
    }

    async fn record_consistency_watermark(
        &self,
        micros: OffsetDateTime,
        behind: bool,
    ) -> Result<(), ClickhouseError> {
        let client = self.maintenance_client();
        let mut insert: clickhouse::insert::Insert<AnomalyRow> = client
            .insert("span_partition_anomalies")
            .await
            .map_err(ClickhouseError::from)?;
        insert
            .write(&AnomalyRow {
                project_id: WATERMARK_KEY.to_string(),
                trace_id: WATERMARK_KEY.to_string(),
                span_id: WATERMARK_KEY.to_string(),
                partitions: Vec::new(),
                // Not a revision count on this row: it carries whether the pass that set the watermark was
                // truncated, which the next pass needs in order to decide about the overlap.
                revisions: u32::from(behind),
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
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            // The first tick fires immediately, and a pass at startup is not wanted: nothing has been ingested
            // yet, so it would read the whole overlap window for nothing on every new replica.
            interval.tick().await;
            loop {
                tokio::select! {
                    biased;
                    changed = shutdown_rx.changed() => {
                        if changed.is_err() || *shutdown_rx.borrow() {
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
            .maintenance_client()
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
