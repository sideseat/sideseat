use super::*;

/// What was removed, for a deletion whose reason cannot be recomputed from the data that remains.
///
/// The distinction the journal turns on. **Age retention writes nothing here**, deliberately: it is a
/// predicate, so a restored database recomputes exactly the same verdict from the timestamps it holds, and an
/// entry per aged-out record would be an unbounded write for a fact that is already derivable. What is *not*
/// derivable is a caller's request and a limit having been reached, and those are what the journal holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::IntoStaticStr, strum::EnumString)]
pub enum DeletionCause {
    /// A caller asked for it - a trace, session, project or organization deletion.
    #[strum(serialize = "requested")]
    Requested,
    /// A limit was reached: count retention (`max_spans`) or quota reclamation. Not reproducible, because it
    /// happened *because* a limit was hit, and a restored database is not at that limit.
    ///
    /// **Nothing writes this yet, and the reason is a granularity question the plan does not settle.** Count
    /// retention selects individual span identities, up to `RETENTION_BATCH_SIZE` (100 000) per batch, and the
    /// journal is permanent and counted against the project's quota - so an entry per evicted span is roughly
    /// 8 MB of permanent rows per full batch, forever. One entry per affected *trace* is bounded and cheap and
    /// answers the wrong question: it would tell the re-drive sweep that a span was deliberately evicted when
    /// only a sibling was, so the sweep would decline to re-drive a payload whose write actually failed - loss,
    /// in the direction this whole subsystem refuses. Omitting the entry is the other error: the sweep re-drives
    /// a payload whose records were evicted, the eviction happens again, and it repeats to the re-drive cap -
    /// bounded, reported, and recoverable.
    ///
    /// So the cautious error is taken until the mechanism that reads it exists. The variant is declared because
    /// the *stored* vocabulary has to be settled before rows are written under it, and the `CHECK` constraint in
    /// both schemas already admits it.
    #[strum(serialize = "pressure")]
    Pressure,
}

impl DeletionCause {
    /// The stored spelling. Declared per variant through `#[strum(serialize = ...)]` rather than derived
    /// from the variant name, so renaming a variant cannot silently orphan every row already written
    /// under the old name.
    pub fn as_str(&self) -> &'static str {
        self.into()
    }

    /// Parse a stored spelling. Named `from_stored` rather than being the `FromStr` impl because the
    /// contract differs: an unknown spelling is `None` rather than an error, since only a newer writer
    /// can produce one.
    pub fn from_stored(value: &str) -> Option<Self> {
        <Self as std::str::FromStr>::from_str(value).ok()
    }
}

/// The kind of thing a journal entry names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::IntoStaticStr, strum::EnumString)]
pub enum DeletionScope {
    #[strum(serialize = "trace")]
    Trace,
    #[strum(serialize = "session")]
    Session,
    #[strum(serialize = "project")]
    Project,
    #[strum(serialize = "organization")]
    Organization,
    /// One span, which only pressure eviction produces: nothing asks to delete a single span.
    #[strum(serialize = "span")]
    Span,
}

impl DeletionScope {
    /// The stored spelling - see [`DeletionCause::as_str`] for why it is declared per variant.
    pub fn as_str(&self) -> &'static str {
        self.into()
    }

    /// Parse a stored spelling - see [`DeletionCause::from_stored`] for the name.
    pub fn from_stored(value: &str) -> Option<Self> {
        <Self as std::str::FromStr>::from_str(value).ok()
    }
}

/// One deletion, as ids, kind and instant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeletionRecord {
    /// The project - or, for an [`DeletionScope::Organization`] entry, the organization.
    ///
    /// One column rather than two, because what it means for both is "the tenant this entry is charged to":
    /// the journal is inside the per-project storage quota, and an organization's own deletion has no project
    /// to charge. A second nullable column would have to be joined or coalesced at every read to answer the
    /// same question.
    pub project_id: ProjectId,
    pub cause: DeletionCause,
    pub scope: DeletionScope,
    /// The trace, session, project or organization id.
    pub target_id: String,
    /// Set only for [`DeletionScope::Span`], where the target is the span's trace.
    pub span_id: Option<String>,
    pub recorded_at: DateTime<Utc>,
}

impl DeletionRecord {
    pub fn logical_bytes_for<P: AsRef<str> + ?Sized>(
        project_id: &P,
        cause: DeletionCause,
        scope: DeletionScope,
        target_id: &str,
        span_id: Option<&str>,
    ) -> u64 {
        64u64
            .saturating_add(project_id.as_ref().len() as u64)
            .saturating_add(cause.as_str().len() as u64)
            .saturating_add(scope.as_str().len() as u64)
            .saturating_add(target_id.len() as u64)
            .saturating_add(span_id.map(str::len).unwrap_or_default() as u64)
    }

    /// Stable logical size charged to the project's storage budget.
    ///
    /// The fixed portion accounts for the sequence, instant, nullable tag and row framing; strings are charged
    /// by UTF-8 bytes so SQLite and PostgreSQL report the same value for identical records.
    pub fn logical_bytes(&self) -> u64 {
        Self::logical_bytes_for(
            &self.project_id,
            self.cause,
            self.scope,
            &self.target_id,
            self.span_id.as_deref(),
        )
    }
}

/// The append-only record of deletions a restore cannot recompute.
///
/// Two consumers, and each needs something the other does not:
///
/// - **A restore replays it forward before serving reads.** A snapshot predating a deletion predates its
///   tombstone too, so restoring the analytics store further back than the transactional one - which is what
///   different backup cadences produce - resurrects rows the caller was told were gone. Replaying the journal
///   removes them again.
/// - **The staged-payload re-drive sweep asks whether an absence was intended.** That sweep looks for staged
///   payloads whose records are absent and re-drives them; without the journal it cannot tell a failed write
///   from a deliberate deletion or a pressure eviction, so it recreates exactly what those removed, and the
///   eviction happens again, until the re-drive cap is exhausted.
///
/// **Never truncated by any sweep, and permanent.** Any retention here would be a bound on how late a stalled
/// writer may commit, which is the thing this whole family of records exists because nothing bounds. The cost
/// is stated rather than hidden: the journal is inside the storage quota, so a project that deletes enough can
/// be refused even after every telemetry byte is reclaimed. That is the correct direction - discarding the
/// record of a deletion to admit new writes trades a durable guarantee for throughput - and it is survivable
/// because the entries are ids and instants rather than payloads.
///
/// **Appended before the deletion it records, never after.** A crash between them must leave a record with no
/// deletion (harmless: the replay removes something already gone, and every step is idempotent) rather than a
/// deletion with no record (the resurrection this exists to prevent). Same ordering as the tombstone, for the
/// same reason.
#[async_trait]
pub trait DeletionJournal: Send + Sync {
    /// Append these deletions.
    ///
    /// A batch, because a trace deletion route removes many traces in one request and one round trip per trace
    /// would make the journal the cost of deleting. Atomic: a partial append is a deletion with no record for
    /// some of its targets, which is the state the ordering above exists to make impossible.
    ///
    /// **Prefer the combined methods below where one exists.** Appending on its own leaves a window in whichever
    /// direction the caller picked: journal first and a failed tombstone leaves a record for a deletion that did
    /// not happen, which a restore replays as a deletion the caller was told had failed; tombstone first and a
    /// failed append leaves a deletion the sweeps will perform anyway with no record, which a restore undoes.
    /// Both writes land in *this* store, so the window is avoidable rather than a trade.
    async fn append_deletions(&self, records: &[DeletionRecord]) -> Result<(), DataError>;

    /// Tombstone these traces and journal their deletion, in one transaction.
    ///
    /// The pair has to be atomic, and an ordering cannot substitute for that. A tombstone is not inert - the
    /// deletion sweeps act on it and remove the rows later - so with the tombstone first a failed append leaves a
    /// deletion that happens anyway and no record of it; with the append first a failed tombstone leaves a
    /// permanent record for a deletion the caller was told had failed, and a restore replays it. Both rows live
    /// in the transactional store, so there is no reason for either.
    async fn record_deleted_traces_journalled(
        &self,
        project_id: &ProjectId,
        trace_ids: &[String],
    ) -> Result<(), DataError>;

    /// Tombstone these sessions **and** the traces they resolved to, and journal all of it, in one transaction.
    ///
    /// Four writes rather than two, for the reason `record_deleted_sessions` exists: a session is deleted by
    /// deleting its traces, so both tombstones are needed, and the journal needs both scopes - the session entry
    /// is the durable fact a replay re-resolves, the trace entries are what remains when the session is no longer
    /// resolvable from restored data at all.
    async fn record_deleted_sessions_journalled(
        &self,
        project_id: &ProjectId,
        session_ids: &[String],
        trace_ids: &[String],
    ) -> Result<(), DataError>;

    /// Journal requested exact-span deletions and record their trace cleanup atomically.
    async fn record_deleted_spans_journalled(
        &self,
        project_id: &ProjectId,
        spans: &[(String, String)],
    ) -> Result<Vec<(String, i64)>, DataError>;

    /// Persist pressure-eviction journal rows and cleanup candidates in one transaction.
    async fn record_pressure_eviction(
        &self,
        project_id: &ProjectId,
        spans: &[(String, String)],
    ) -> Result<Vec<(String, i64)>, DataError>;

    /// Claim a project for deletion and journal it, in one transaction.
    ///
    /// Returns whether this caller won the claim. The journal entry is written **only when it did**, which the
    /// separate calls could not achieve: journalling before the claim wrote an entry for every losing caller -
    /// and since an organization cleanup re-runs while its projects' tombstones remain, that was an unbounded
    /// number of permanent, quota-counted records for one deletion.
    async fn claim_project_for_deletion_journalled(&self, id: &str) -> Result<bool, DataError>;

    /// Claim an organization for deletion and journal it, in one transaction. See the project twin.
    async fn claim_organization_for_deletion_journalled(&self, id: &str)
    -> Result<bool, DataError>;

    /// Entries after `after_sequence`, oldest first, at most `limit`.
    ///
    /// The sequence is the journal's own append order and is returned with each entry, so a replay that is
    /// interrupted resumes from what it has applied rather than from a timestamp - two entries can share a
    /// microsecond, and a clock is not an order (the analytics stores have no commit-ordered sequence at all,
    /// which is why this one is a column the transactional store assigns).
    ///
    /// Returns `(entries, highest_sequence_examined)`, and the second value is not a convenience. A row whose
    /// cause or scope this build does not understand is skipped rather than guessed at, so a page can yield
    /// *fewer* entries than it examined - and a page consisting entirely of such rows yields none at all, which
    /// against a cursor advanced only by returned entries is indistinguishable from the end of the journal. The
    /// replay would stop there and never reach the known deletions behind them. Advancing on what was
    /// **examined** makes progress a property of the page rather than of what happened to be interpretable -
    /// the same reasoning as the search cursor's last-*examined* position in §4.3 of the plan.
    ///
    /// `highest_sequence_examined` equals `after_sequence` when the page was empty, so a caller can loop until
    /// it stops advancing.
    async fn deletions_since(
        &self,
        after_sequence: i64,
        limit: usize,
    ) -> Result<(Vec<(i64, DeletionRecord)>, i64), DataError>;

    /// Whether the journal explains this record's absence.
    ///
    /// What the re-drive sweep asks before recreating a staged payload's records. Scoped by project and
    /// target, and a `Span` target is answered by an entry for the span **or** for its trace, because
    /// deleting the trace removes the span too.
    async fn deletion_is_journaled(
        &self,
        project_id: &ProjectId,
        scope: DeletionScope,
        target_id: &str,
        span_id: Option<&str>,
    ) -> Result<bool, DataError>;

    /// Span identities already covered by a span- or trace-scoped journal entry.
    async fn journaled_spans_among(
        &self,
        project_id: &ProjectId,
        spans: &[(String, String)],
    ) -> Result<HashSet<(String, String)>, DataError>;

    /// Exact span deletions recorded for these traces.
    ///
    /// A cleanup candidate stores one row per trace, while pressure/requested deletion records are exact span
    /// identities. Re-reading those identities lets cleanup re-apply the analytical deletion after a crash or a
    /// writer that committed in the cross-store window, before it reconciles the trace's surviving file
    /// references.
    async fn journaled_span_deletions_for_traces(
        &self,
        project_id: &ProjectId,
        trace_ids: &[String],
    ) -> Result<Vec<(String, String)>, DataError>;
}

/// Stable logical size of one retention-cleanup candidate.
pub fn retention_cleanup_logical_bytes<P: AsRef<str> + ?Sized>(
    project_id: &P,
    trace_id: &str,
) -> u64 {
    64u64
        .saturating_add(project_id.as_ref().len() as u64)
        .saturating_add(trace_id.len() as u64)
}

/// Durable metadata for payload bytes staged before OTLP acknowledgement.
///
/// The blob itself lives behind `FileStorage`; this registry is what makes queue loss discoverable and
/// records the bounded redrive state. Rows have no TTL and leave only through confirmation or a proven
/// deletion.
#[async_trait]
pub trait StagedPayloadStore: Send + Sync {
    async fn create_staged_payload(&self, payload: &StagedPayload) -> Result<(), DataError>;

    async fn get_staged_payload(&self, id: &str) -> Result<Option<StagedPayload>, DataError>;

    /// Oldest payloads that have not exhausted the redrive cap.
    async fn pending_staged_payloads(&self, limit: usize) -> Result<Vec<StagedPayload>, DataError>;

    /// Increment and return the durable attempt count.
    async fn increment_staged_redrive_attempts(&self, id: &str) -> Result<u32, DataError>;

    async fn mark_staged_unconfirmed(&self, id: &str) -> Result<(), DataError>;

    async fn delete_staged_payload(&self, id: &str) -> Result<(), DataError>;

    /// Remove registry rows for a project that does not exist at the transactional restore point.
    async fn delete_project_staged_payloads(
        &self,
        project_id: &ProjectId,
    ) -> Result<u64, DataError>;
}

/// Durable legal-hold state, maintenance fencing and quota admission.
///
/// Hold recording and retention batches share the same leased per-project mutex. The usage counter is
/// deliberately best-effort: admission increments it atomically, while reconciliation replaces it from
/// independently measured storage.
#[async_trait]
pub trait StorageGovernance: Send + Sync {
    async fn active_project_hold(
        &self,
        project_id: &ProjectId,
        now: DateTime<Utc>,
    ) -> Result<Option<ProjectHold>, DataError>;

    async fn list_active_project_holds(
        &self,
        now: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<ProjectHold>, DataError>;

    async fn set_project_hold(
        &self,
        project_id: &ProjectId,
        hold_until: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> Result<ProjectHold, DataError>;

    async fn clear_project_hold(&self, project_id: &ProjectId) -> Result<bool, DataError>;

    async fn acquire_project_maintenance(
        &self,
        project_id: &ProjectId,
        owner: &str,
        now: DateTime<Utc>,
        lease_until: DateTime<Utc>,
    ) -> Result<bool, DataError>;

    async fn release_project_maintenance(
        &self,
        project_id: &ProjectId,
        owner: &str,
    ) -> Result<(), DataError>;

    /// Reserve ordinary-write capacity. Returns the resulting measured usage, or `None` on refusal.
    async fn reserve_project_storage(
        &self,
        project_id: &ProjectId,
        additional_bytes: u64,
        ordinary_limit_bytes: u64,
        now: DateTime<Utc>,
    ) -> Result<Option<ProjectStorageUsage>, DataError>;

    async fn project_storage_usage(
        &self,
        project_id: &ProjectId,
    ) -> Result<ProjectStorageUsage, DataError>;

    async fn replace_project_storage_usage(
        &self,
        project_id: &ProjectId,
        logical_bytes: u64,
        now: DateTime<Utc>,
    ) -> Result<ProjectStorageUsage, DataError>;

    /// Held transactional bytes: staged payloads plus the permanent deletion journal.
    async fn held_transactional_bytes(&self, project_id: &ProjectId) -> Result<u64, DataError>;

    /// Projects participating in accounting, including projects whose measured usage is still zero.
    async fn storage_project_ids(&self, limit: usize) -> Result<Vec<ProjectId>, DataError>;
}

#[cfg(test)]
mod deletion_vocabulary_tests {
    use super::{DeletionCause, DeletionScope};

    /// The spellings the journal's rows are written under, and the `CHECK` constraints that admit them.
    ///
    /// Both schemas spell this vocabulary out - `CHECK(cause IN ('requested', 'pressure'))` and
    /// `CHECK(scope IN ('trace', 'session', 'project', 'organization', 'span'))` in
    /// `adapter-sqlite/src/schema.rs` and `adapter-postgres/src/schema.rs` - so a changed spelling is not
    /// a compile error here; it is a rejected insert there, and permanent restore evidence that never
    /// gets written. This table is the third copy, and the one a reader of these enums sees.
    #[test]
    fn every_stored_spelling_round_trips_and_is_unchanged() {
        let causes = [
            (DeletionCause::Requested, "requested"),
            (DeletionCause::Pressure, "pressure"),
        ];
        for (cause, spelling) in causes {
            assert_eq!(cause.as_str(), spelling);
            assert_eq!(DeletionCause::from_stored(spelling), Some(cause));
        }

        let scopes = [
            (DeletionScope::Trace, "trace"),
            (DeletionScope::Session, "session"),
            (DeletionScope::Project, "project"),
            (DeletionScope::Organization, "organization"),
            (DeletionScope::Span, "span"),
        ];
        for (scope, spelling) in scopes {
            assert_eq!(scope.as_str(), spelling);
            assert_eq!(DeletionScope::from_stored(spelling), Some(scope));
        }
    }

    /// An unknown spelling is `None`, not an error: only a newer writer can produce one, and a reader
    /// that refused would stop the sweep rather than skip the row it cannot interpret.
    #[test]
    fn an_unknown_spelling_is_none_rather_than_an_error() {
        assert_eq!(DeletionCause::from_stored("future-cause"), None);
        assert_eq!(DeletionScope::from_stored("future-scope"), None);
        assert_eq!(DeletionScope::from_stored("Trace"), None, "case matters");
        assert_eq!(DeletionCause::from_stored(""), None);
    }
}
