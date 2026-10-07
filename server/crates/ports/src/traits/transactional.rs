use super::*;

/// Users, organizations, memberships and auth methods: who is asking.
#[async_trait]
pub trait IdentityStore: Send + Sync {
    /// Create a new user
    async fn create_user(
        &self,
        email: &str,
        display_name: Option<&str>,
    ) -> Result<UserRow, DataError>;

    /// Get a user by ID
    async fn get_user(&self, id: &str) -> Result<Option<UserRow>, DataError>;

    /// Get a user by email
    async fn get_user_by_email(&self, email: &str) -> Result<Option<UserRow>, DataError>;

    /// Update a user's display name
    async fn update_user(
        &self,
        id: &str,
        display_name: Option<&str>,
    ) -> Result<Option<UserRow>, DataError>;
    /// Create a new organization with owner membership atomically
    async fn create_organization_with_owner(
        &self,
        name: &str,
        slug: &str,
        owner_user_id: &str,
    ) -> Result<OrganizationRow, DataError>;

    /// Get an organization by ID
    async fn get_organization(&self, id: &str) -> Result<Option<OrganizationRow>, DataError>;

    /// Update an organization's name
    async fn update_organization(
        &self,
        id: &str,
        name: &str,
    ) -> Result<Option<OrganizationRow>, DataError>;

    /// List organizations for a user with their role
    async fn list_orgs_for_user(
        &self,
        user_id: &str,
        page: u32,
        limit: u32,
    ) -> Result<(Vec<OrgWithRole>, u64), DataError>;

    /// Delete an organization (cascades to projects, memberships, files)
    async fn delete_organization(&self, id: &str) -> Result<bool, DataError>;

    /// List project IDs for an organization (for cascade cleanup)
    async fn list_project_ids(&self, organization_id: &str) -> Result<Vec<String>, DataError>;
    /// Get a membership
    async fn get_membership(
        &self,
        organization_id: &str,
        user_id: &str,
    ) -> Result<Option<MembershipRow>, DataError>;

    /// Get a member with user info
    async fn get_member_with_user(
        &self,
        organization_id: &str,
        user_id: &str,
    ) -> Result<Option<MemberWithUser>, DataError>;

    /// Add a member to an organization
    async fn add_member(
        &self,
        organization_id: &str,
        user_id: &str,
        role: &str,
    ) -> Result<MembershipRow, DataError>;

    /// List members of an organization
    async fn list_members(
        &self,
        organization_id: &str,
        page: u32,
        limit: u32,
    ) -> Result<(Vec<MemberWithUser>, u64), DataError>;

    /// Update a member's role atomically with last-owner protection
    async fn update_role_atomic(
        &self,
        organization_id: &str,
        user_id: &str,
        new_role: &str,
    ) -> Result<LastOwnerResult<MembershipRow>, DataError>;

    /// Remove a member atomically with last-owner protection
    async fn remove_member_atomic(
        &self,
        organization_id: &str,
        user_id: &str,
    ) -> Result<LastOwnerResult<()>, DataError>;
    /// Create a new auth method
    #[allow(clippy::too_many_arguments)]
    async fn create_auth_method(
        &self,
        user_id: &str,
        method_type: &str,
        provider: Option<&str>,
        provider_id: Option<&str>,
        credential_hash: Option<&str>,
        metadata: Option<&str>,
    ) -> Result<AuthMethodRow, DataError>;

    /// Find an auth method by OAuth provider and provider ID
    async fn find_auth_by_oauth(
        &self,
        provider: &str,
        provider_id: &str,
    ) -> Result<Option<AuthMethodRow>, DataError>;

    /// List all auth methods for a user
    async fn list_auth_methods_for_user(
        &self,
        user_id: &str,
    ) -> Result<Vec<AuthMethodRow>, DataError>;

    /// Delete an auth method
    async fn delete_auth_method(&self, id: &str) -> Result<bool, DataError>;

    /// Get the bootstrap auth method for a user
    async fn get_bootstrap_method(&self, user_id: &str)
    -> Result<Option<AuthMethodRow>, DataError>;
}

/// Projects and their deletion fence.
#[async_trait]
pub trait ProjectStore: Send + Sync {
    /// Create a new project
    async fn create_project(
        &self,
        organization_id: &str,
        name: &str,
    ) -> Result<ProjectRow, DataError>;

    /// Get a project by ID
    async fn get_project(&self, id: &str) -> Result<Option<ProjectRow>, DataError>;

    /// Update a project's name
    async fn update_project(&self, id: &str, name: &str) -> Result<Option<ProjectRow>, DataError>;

    /// List projects for an organization
    async fn list_projects_for_org(
        &self,
        organization_id: &str,
        page: u32,
        limit: u32,
    ) -> Result<(Vec<ProjectRow>, u64), DataError>;

    /// List projects for a user (across all orgs they're a member of)
    async fn list_projects_for_user(
        &self,
        user_id: &str,
        page: u32,
        limit: u32,
    ) -> Result<(Vec<ProjectRow>, u64), DataError>;

    /// List all live projects for bounded background maintenance.
    async fn list_projects(
        &self,
        page: u32,
        limit: u32,
    ) -> Result<(Vec<ProjectRow>, u64), DataError>;

    /// Every live project or project represented by restored content ownership.
    ///
    /// This is broader than [`Self::list_projects`]: restore must also discover project-local rows whose
    /// parent project was outside the selected transactional backup. Durable lifecycle facts are excluded
    /// because repair does not consume them and therefore could not reach a fixed point.
    async fn restore_project_ids(&self, limit: usize) -> Result<Vec<ProjectId>, DataError>;

    /// Claim a project for deletion, if it exists and nobody else has claimed it.
    ///
    /// Takes the cache because a successful claim must drop every cached answer about the project at
    /// once: from that moment it is not live, and a cached "here it is" would outlive the fact by the
    /// cache's five minutes.
    async fn claim_project_for_deletion(&self, id: &str) -> Result<bool, DataError>;

    /// Whether this project accepts writes: a row exists and nothing has claimed it.
    ///
    /// A missing project is refused as firmly as a claimed one. Spans written for a project with no row
    /// are unreachable through every read path, so accepting them stores data nothing can ever show.
    async fn project_accepts_writes(&self, id: &str) -> Result<bool, DataError>;

    /// Record that these traces were deleted, so a late ingest cannot resurrect them.
    ///
    /// The trace deletion route removes the analytics rows and then reclaims the file bytes those rows
    /// referenced. An ingest already in flight for one of those traces commits *afterwards* - the file
    /// association and bytes were written before the analytics row, which is deliberate - and the
    /// result is a span row carrying a `#!B64!#` reference to content the deletion has taken, for a
    /// trace the caller was told 204 for. No elapsed time bounds that: a queued batch can be
    /// redelivered minutes later.
    ///
    /// So the deletion leaves a tombstone and the write path consults it. Written in the same request
    /// as the deletion, before the analytics delete, so no window exists where a trace is deleted and
    /// not yet tombstoned.
    async fn record_deleted_traces(
        &self,
        project_id: &ProjectId,
        trace_ids: &[String],
    ) -> Result<(), DataError>;

    /// Which of these traces are tombstoned, so their spans must not be written.
    ///
    /// Returns the subset that has been deleted. One query per batch rather than per span: a batch
    /// commonly carries a handful of traces, and this sits directly on the ingestion hot path.
    async fn deleted_traces_among(
        &self,
        project_id: &ProjectId,
        trace_ids: &[String],
    ) -> Result<std::collections::HashSet<String>, DataError>;

    /// Projects claimed for deletion longer ago than `older_than_secs`, so a cleanup that died part
    /// way through can be resumed.
    /// Abandoned project cleanups, with the tombstone value each was observed at - see
    /// [`TransactionalRepository::reclaim_stale_project`]. Capped, so a backlog cannot starve the leased
    /// trace and session sweeps that run after it in the same pass.
    async fn get_stale_claimed_projects(
        &self,
        older_than_secs: i64,
    ) -> Result<Vec<(String, i64)>, DataError>;

    /// Re-lease an abandoned project cleanup, if the tombstone is still the one observed.
    ///
    /// The lease *is* the tombstone's timestamp, pushed forward: `deleting_at` is both the fence and the age
    /// that makes a claim look abandoned, so refreshing it hands this project to one resumer without lifting
    /// the fence - which has to stay set until the cleanup finishes. Unleased, every replica resumed every
    /// stale project and duplicated all of its four-store cleanup.
    async fn reclaim_stale_project(
        &self,
        project_id: &ProjectId,
        observed_deleting_at: i64,
    ) -> Result<bool, DataError>;

    /// Record what a cleanup sweep observed for a project, and answer whether its tombstone may go.
    ///
    /// The barrier is repeated observation, not elapsed time: no wall-clock grace period bounds how long
    /// a writer that read the fence before the tombstone can take to commit.
    async fn record_project_sweep(
        &self,
        id: &str,
        was_clean: bool,
        required: i64,
        min_gap_secs: i64,
    ) -> Result<bool, DataError>;

    /// Claim deleted-project ids for a cleanup check, one per window.
    ///
    /// Returns the ids this call claimed - not every one recorded. Cleanup runs on every instance, and the
    /// records are permanent; without a window the cost would grow with instances times lifetime
    /// deletions. Marking the check in the same statement that returns it makes concurrent instances race
    /// for each id and only one wins per window.
    /// Returns `(project id, claim token)`. The token is what a report must present: a worker whose lease
    /// expired part way through its batch would otherwise overwrite the schedule and result of the worker
    /// that has since taken the id.
    async fn claim_deleted_projects_for_check(
        &self,
        lease_secs: i64,
        limit: i64,
    ) -> Result<Vec<(String, i64)>, DataError>;

    /// Record that these sessions were deleted, so a trace arriving later cannot recreate them.
    ///
    /// The trace tombstone is not enough on its own: a session is deleted by resolving it to trace ids and
    /// deleting those, and a trace of the same session that arrives *after* that resolution was never in
    /// the snapshot. The session id is the durable fact - the trace ids are one instant's view of it.
    async fn record_deleted_sessions(
        &self,
        project_id: &ProjectId,
        session_ids: &[String],
    ) -> Result<(), DataError>;

    /// Which of these sessions are tombstoned, so their spans must not be written.
    async fn deleted_sessions_among(
        &self,
        project_id: &ProjectId,
        session_ids: &[String],
    ) -> Result<std::collections::HashSet<String>, DataError>;

    /// Claim a batch of deleted *sessions* whose check is due, leased and exclusive.
    ///
    /// Same protocol as traces, for the same reason: the pre-write session check and the analytics write are
    /// in different stores, so a crash between the write and its compensating re-check leaves spans for a
    /// deleted session that only a sweep can collect.
    async fn claim_deleted_sessions_for_check(
        &self,
        lease_secs: i64,
        limit: i64,
    ) -> Result<Vec<(String, String, i64)>, DataError>;

    /// Record what a deleted session's check found, matched on the claim token.
    async fn record_deleted_session_check(
        &self,
        project_id: &ProjectId,
        session_id: &str,
        claim_token: i64,
        was_quiet: bool,
        base_gap_secs: i64,
        max_gap_secs: i64,
    ) -> Result<(), DataError>;

    /// Claim a batch of deleted *traces* whose check is due, leased and exclusive.
    ///
    /// Same discipline as the project claim, and needed for the same reason: the pre-write tombstone check
    /// and the analytics write are in different stores, so a crash between them leaves spans for a deleted
    /// trace that only a sweep can collect. Re-checking every record forever at a fixed rate would be
    /// unbounded lifetime work, hence the lease, the backoff and the cap.
    async fn claim_deleted_traces_for_check(
        &self,
        lease_secs: i64,
        limit: i64,
    ) -> Result<Vec<(String, String, i64)>, DataError>;

    /// Record what a deleted trace's check found, matched on the claim token.
    async fn record_deleted_trace_check(
        &self,
        project_id: &ProjectId,
        trace_id: &str,
        claim_token: i64,
        was_quiet: bool,
        base_gap_secs: i64,
        max_gap_secs: i64,
    ) -> Result<(), DataError>;

    /// Record what a deleted project's check found. Quiet pushes the next check further out; anything found
    /// brings it back to the base interval.
    async fn record_deleted_project_check(
        &self,
        project_id: &ProjectId,
        claim_token: i64,
        was_quiet: bool,
        base_gap_secs: i64,
        max_gap_secs: i64,
    ) -> Result<(), DataError>;

    /// Forget projects deleted longer ago than `retention_secs`. Returns how many were forgotten.
    async fn forget_deleted_projects(&self, retention_secs: i64) -> Result<u64, DataError>;

    /// Claim an organization for deletion, if it exists and nobody else has claimed it.
    async fn claim_organization_for_deletion(&self, id: &str) -> Result<bool, DataError>;

    /// Organizations claimed for deletion longer ago than `older_than_secs`.
    async fn get_stale_claimed_organizations(
        &self,
        older_than_secs: i64,
    ) -> Result<Vec<(String, i64)>, DataError>;

    /// Re-lease an abandoned organization cleanup - see [`TransactionalRepository::reclaim_stale_project`].
    async fn reclaim_stale_organization(
        &self,
        org_id: &str,
        observed_deleting_at: i64,
    ) -> Result<bool, DataError>;

    /// How many project rows an organization still has, tombstoned or not.
    async fn count_projects_of_organization(&self, org_id: &str) -> Result<i64, DataError>;

    /// Delete a project's row. Only correct once its data is gone: the row is what every other path
    /// finds the data by.
    async fn delete_project(&self, id: &str) -> Result<bool, DataError>;
}

/// The rows that name stored bytes, and the reference counting that protects them.
#[async_trait]
pub trait FileMetaStore: Send + Sync {
    /// A stable page of trace identities still named by transactional byte ownership.
    ///
    /// Restore repair must inspect both directions of a mismatched snapshot: analytics rows can have lost
    /// their ownership rows, and ownership rows can outlive analytics rows. The latter cannot be discovered
    /// by scanning analytics alone, so this pages the union of `trace_files` and `span_bodies` by trace id.
    async fn restore_association_trace_ids(
        &self,
        project_id: &ProjectId,
        after_trace_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<String>, DataError>;

    /// Upsert a file record (insert or increment ref_count)
    /// Returns the new ref_count value.
    async fn upsert_file(
        &self,
        project_id: &ProjectId,
        file_hash: &str,
        media_type: Option<&str>,
        size_bytes: i64,
        hash_algo: &str,
    ) -> Result<i64, DataError>;

    /// Get a file by project and hash
    async fn get_file(
        &self,
        project_id: &ProjectId,
        file_hash: &str,
    ) -> Result<Option<FileRow>, DataError>;

    /// Check if a file exists
    async fn file_exists(&self, project_id: &ProjectId, file_hash: &str)
    -> Result<bool, DataError>;

    /// Decrement ref_count atomically and return the new value
    /// Returns None if file doesn't exist, Some(new_ref_count) otherwise.
    async fn decrement_ref_count(
        &self,
        project_id: &ProjectId,
        file_hash: &str,
    ) -> Result<Option<i64>, DataError>;

    /// Delete a file metadata record
    async fn delete_file(&self, project_id: &ProjectId, file_hash: &str)
    -> Result<bool, DataError>;

    /// Delete all file records for a project
    async fn delete_project_files(&self, project_id: &ProjectId) -> Result<u64, DataError>;

    /// Associate a file with a trace, counting the reference only if the association is new.
    ///
    /// One operation because `ref_count` must equal the number of associations - that is what
    /// deletion decrements, and doing the two separately drifted in both directions.
    async fn associate_file(
        &self,
        trace_id: &str,
        project_id: &ProjectId,
        file_hash: &str,
        media_type: Option<&str>,
        size_bytes: i64,
        hash_algo: &str,
    ) -> Result<bool, DataError>;

    /// How many of these traces reference each file, so deletion can decrement by that many.
    async fn get_file_reference_counts_for_traces(
        &self,
        project_id: &ProjectId,
        trace_ids: &[String],
    ) -> Result<Vec<(String, i64)>, DataError>;

    /// Associate a trace with a file that already exists, without inventing metadata for it.
    ///
    /// Returns false when there is no such file, or it is claimed for deletion - both mean the caller
    /// must not commit a reference to it.
    async fn associate_existing_file(
        &self,
        trace_id: &str,
        project_id: &ProjectId,
        file_hash: &str,
    ) -> Result<bool, DataError>;

    /// Files claimed for deletion longer ago than `older_than_secs`, so an abandoned claim can be resumed.
    async fn get_stale_claimed_files(
        &self,
        older_than_secs: i64,
    ) -> Result<Vec<(String, String, i64)>, DataError>;

    /// Re-take an abandoned claim, but only if it is still exactly the one observed.
    ///
    /// The recovery path reads a snapshot and then deletes bytes; in between, the row can be released or
    /// deleted and recreated by an ingestion that associates the same content hash. Gating the byte deletion
    /// on this compare-and-set is what stops it removing content a committed span references.
    async fn reclaim_stale_file(
        &self,
        project_id: &ProjectId,
        file_hash: &str,
        observed_deleting_at: i64,
    ) -> Result<bool, DataError>;

    /// Claim a file for deletion, if nothing references it and nobody else has claimed it.
    ///
    /// The fence that closes the delete-then-recreate window: association refuses while a claim is set,
    /// because a claimed file's bytes may already be gone.
    async fn claim_file_for_deletion(
        &self,
        project_id: &ProjectId,
        file_hash: &str,
    ) -> Result<bool, DataError>;

    /// Give up a deletion claim, leaving the file in place.
    async fn release_deletion_claim(
        &self,
        project_id: &ProjectId,
        file_hash: &str,
    ) -> Result<(), DataError>;

    /// Put back a metadata row whose bytes could not be deleted, as an orphan for a later sweep.
    async fn restore_orphan_metadata(
        &self,
        project_id: &ProjectId,
        file_hash: &str,
        media_type: Option<&str>,
        size_bytes: i64,
        hash_algo: &str,
    ) -> Result<(), DataError>;

    /// Delete a file's metadata only if nothing references it, and say whether it was deleted.
    ///
    /// The condition belongs inside the statement: reading a count of zero and then deleting races with
    /// a concurrent association, and loses.
    async fn delete_file_if_unreferenced(
        &self,
        project_id: &ProjectId,
        file_hash: &str,
    ) -> Result<bool, DataError>;

    /// Recompute a file's reference count from the associations that exist, and return it.
    ///
    /// The count is a cached `COUNT(*)`; deriving it is the only form immune to two concurrent
    /// cleanups both subtracting the same references.
    async fn sync_ref_count(
        &self,
        project_id: &ProjectId,
        file_hash: &str,
    ) -> Result<Option<i64>, DataError>;

    /// Insert a trace-file association
    async fn insert_trace_file(
        &self,
        trace_id: &str,
        project_id: &ProjectId,
        file_hash: &str,
    ) -> Result<(), DataError>;

    /// Get file hashes for traces
    async fn get_file_hashes_for_traces(
        &self,
        project_id: &ProjectId,
        trace_ids: &[String],
    ) -> Result<Vec<String>, DataError>;

    /// Release one association this process created, after the write that would have justified it failed.
    ///
    /// A compensating action, not a deletion: the bytes are written before the analytics row that
    /// references them, so a batch whose analytics write fails has already created associations for spans
    /// that will never land - and those keep `ref_count` above zero, which is exactly what the orphan
    /// sweeper selects on, so nothing would ever reclaim them.
    ///
    /// Scoped to a single `(project, trace, hash)` because only the association this batch *created* may
    /// go. One that already existed belongs to an earlier committed batch, and releasing it would orphan
    /// that batch's file. Returns whether a row was removed.
    /// Confirm a batch's associations now that its analytics rows are committed.
    ///
    /// An association is created *provisional*, because files are written before the rows that name them.
    /// Confirming clears that marker, and the failure path deletes only rows still marked - which is what
    /// makes the release safe under concurrency rather than merely precise: two batches can carry the same
    /// association, and a read-then-release pair cannot tell "mine, unused" from "also the other batch's,
    /// now committed".
    async fn confirm_trace_file_associations(
        &self,
        associations: &[(String, String, String)],
    ) -> Result<u64, DataError>;

    async fn release_trace_file_association(
        &self,
        project_id: &ProjectId,
        trace_id: &str,
        file_hash: &str,
    ) -> Result<bool, DataError>;

    /// Delete trace-file associations for traces, returning the file hashes affected.
    ///
    /// The hashes come from the delete itself rather than from a prior read: an association added between a
    /// read and the delete is removed here but absent from the read, so its file's stored reference count is
    /// never recomputed - and the orphan sweeper selects on that count, so nothing would ever reclaim it.
    async fn delete_trace_files(
        &self,
        project_id: &ProjectId,
        trace_ids: &[String],
    ) -> Result<Vec<String>, DataError>;

    /// Record that these traces need file and favourite cleanup, **before** the spans are deleted.
    ///
    /// DuckDB commits the span deletion and the cleanup runs afterwards, asynchronously, with failures only
    /// logged. A crash or a transactional-store outage in between loses the only record of which traces needed
    /// cleaning - and their spans are already gone, so no later pass can rediscover them. Their associations,
    /// ref-counted bytes and favourites are orphaned permanently.
    ///
    /// Idempotent for the *row*, and it **bumps the token**, so a claim held by an earlier worker no longer
    /// matches: re-recording means there is new work behind the same identity, and a stale completion must not
    /// discard it.
    /// Returns each trace with the **token it now carries**, so the recording pass can complete on exactly the
    /// rows it wrote. Guessing a token would either fail to complete (leaving a record the sweep re-drives) or,
    /// worse, match a newer one.
    async fn record_retention_cleanup(
        &self,
        project_id: &ProjectId,
        trace_ids: &[String],
    ) -> Result<Vec<(String, i64)>, DataError>;

    /// Claim due cleanup candidates, leasing them so a concurrent instance takes different ones.
    ///
    /// The claim pushes `next_attempt_at` out **before** returning, which is what stops a slow reconciliation
    /// being re-claimed while it runs - the same discipline the deleted-project and deleted-trace sweeps use,
    /// and for the same reason: without it every replica drains the same rows.
    async fn claim_retention_cleanup(
        &self,
        limit: i64,
        lease_secs: i64,
    ) -> Result<Vec<(String, String, i64)>, DataError>;

    /// Drop candidates whose cleanup has completed, **only if still on the claimed token**.
    ///
    /// Deleting by identity alone let a stale worker remove a *newer* intent. A worker claims trace T, pauses
    /// past its lease; retention runs again for T, re-uses the row and deletes more spans; the paused worker
    /// then finishes its old work and deletes the row - so if the newer worker fails, the cleanup it recorded is
    /// gone and nothing can rediscover it. Comparing the token makes the completion refer to the work that was
    /// actually claimed.
    async fn complete_retention_cleanup(
        &self,
        project_id: &ProjectId,
        completed: &[(String, i64)],
    ) -> Result<(), DataError>;

    /// Restore an association a survivor scan released, as **durable**.
    ///
    /// The compensation path: a span committed between the scan and the release. `durable` rather than
    /// provisional, because the reference belongs to a *committed* span - restoring it as provisional lets a
    /// later failing batch's release delete it, since that release deletes a non-durable row with no writer left.
    async fn restore_durable_trace_file(
        &self,
        project_id: &ProjectId,
        trace_id: &str,
        file_hash: &str,
    ) -> Result<(), DataError>;

    /// Release a trace's associations **except** the ones its surviving spans still reference.
    ///
    /// The counterpart to [`Self::delete_trace_files`], and the distinction is the whole point: that one is
    /// for a trace that is *gone*, this one for a trace that has merely had some of its spans expired.
    /// Retention selects individual span identities, so handing it the trace-wide delete removed **every**
    /// association for a trace whose other spans were still live - leaving those spans pointing at bytes that
    /// had been reclaimed, which is precisely the dangling reference the write-files-before-rows ordering
    /// exists to prevent, produced by retention instead.
    ///
    /// Two conditions, and neither is optional:
    ///
    /// - `file_hash NOT IN keep`, so a file a survivor references is untouched.
    /// - `pending_writers = 0`, which is what protects a batch in flight. Referencing a file increments that
    ///   counter *before* the span row exists, so a concurrent ingestion is invisible to the survivor scan -
    ///   there is no span to find yet. Without this the reconciliation is a read-then-act race with exactly
    ///   the window it is meant to close.
    ///
    /// Returns the hashes actually removed, so the caller reconciles the set the statement produced rather
    /// than one it read beforehand - the same reason `delete_trace_files` uses `RETURNING`.
    async fn release_trace_files_except(
        &self,
        project_id: &ProjectId,
        trace_id: &str,
        keep: &[String],
    ) -> Result<Vec<String>, DataError>;

    /// Get total storage used by a project
    async fn get_project_storage_bytes(&self, project_id: &ProjectId) -> Result<i64, DataError>;

    /// Get all files with zero ref_count (for cleanup)
    async fn get_orphan_files(&self) -> Result<Vec<(String, String)>, DataError>;

    /// Get total file storage used by all projects in an organization
    async fn get_org_file_storage_bytes(&self, org_id: &str) -> Result<i64, DataError>;

    /// Get total file storage used across all orgs a user belongs to
    async fn get_user_file_storage_bytes(&self, user_id: &str) -> Result<i64, DataError>;
}

/// The retired content-body registry, kept only so it can be drained.
///
/// Span bodies used to be dual-written as content-addressed objects beside the analytics columns that already
/// held them. The analytics columns are the sole authority now; nothing writes or reads a body. These methods
/// empty what earlier versions left: associations first, then each object through the deletion claim, so a
/// crash at any point leaves either a claimed row the stale-claim pass finishes or an untouched one.
#[async_trait]
pub trait ContentBodyStore: Send + Sync {
    /// Delete up to `limit` body associations, of any project, and return how many went.
    ///
    /// No reader consults an association and no writer creates one, so removing them changes no answer; it
    /// turns every registered object into an orphan the claim protocol below can delete.
    async fn retire_span_body_associations(&self, limit: usize) -> Result<u64, DataError>;

    async fn get_orphan_content_bodies(
        &self,
        older_than: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<(ProjectId, String)>, DataError>;

    /// List deletion claims old enough that their worker may have crashed.
    async fn get_stale_claimed_content_bodies(
        &self,
        older_than: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<(ProjectId, String)>, DataError>;

    async fn claim_content_body_for_deletion(
        &self,
        project_id: &ProjectId,
        body_hash: &str,
    ) -> Result<bool, DataError>;

    async fn release_content_body_deletion_claim(
        &self,
        project_id: &ProjectId,
        body_hash: &str,
    ) -> Result<(), DataError>;

    async fn delete_claimed_content_body(
        &self,
        project_id: &ProjectId,
        body_hash: &str,
    ) -> Result<bool, DataError>;

    /// Project deletion: remove the project's registry rows and return the object hashes to delete.
    async fn delete_project_bodies(&self, project_id: &ProjectId)
    -> Result<Vec<String>, DataError>;
}

/// API keys, stored as a hash.
#[async_trait]
pub trait ApiKeyStore: Send + Sync {
    /// Create API key. Returns Err(Conflict) if limit (100) exceeded.
    #[allow(clippy::too_many_arguments)]
    async fn create_api_key(
        &self,
        org_id: &str,
        name: &str,
        key_hash: &str,
        key_prefix: &str,
        scope: ApiKeyScope,
        created_by: &str,
        expires_at: Option<i64>,
    ) -> Result<ApiKeyRow, DataError>;

    /// Get validation info by hash. Used for OTEL and API auth.
    async fn get_api_key_by_hash(
        &self,
        key_hash: &str,
    ) -> Result<Option<ApiKeyValidation>, DataError>;

    /// List all keys for organization (metadata only, ordered by created_at DESC).
    async fn list_api_keys(&self, org_id: &str) -> Result<Vec<ApiKeyRow>, DataError>;

    /// Delete key by ID.
    async fn delete_api_key(&self, id: &str, org_id: &str) -> Result<bool, DataError>;

    /// Update `last_used_at` when the threshold elapsed and invalidate the matching auth cache entry.
    async fn touch_api_key(
        &self,
        id: &str,
        key_hash: &str,
        threshold_secs: u64,
    ) -> Result<bool, DataError>;

    /// Delete all keys for organization (for org deletion cleanup).
    async fn delete_api_keys_for_org(&self, org_id: &str) -> Result<u64, DataError>;

    /// Get key hashes for organization (for cache invalidation on org delete).
    async fn get_api_key_hashes_for_org(&self, org_id: &str) -> Result<Vec<String>, DataError>;
}

/// Provider credentials and their per-project permissions.
#[async_trait]
pub trait CredentialStore: Send + Sync {
    /// List all credentials for an organization (metadata only, no secrets)
    async fn list_credentials(&self, org_id: &str) -> Result<Vec<CredentialRow>, DataError>;

    /// Get a single credential by id, scoped to org
    async fn get_credential(
        &self,
        id: &str,
        org_id: &str,
    ) -> Result<Option<CredentialRow>, DataError>;

    /// Create a new credential row (secret stored separately)
    #[allow(clippy::too_many_arguments)]
    async fn create_credential(
        &self,
        id: &str,
        org_id: &str,
        provider_key: &str,
        display_name: &str,
        endpoint_url: Option<&str>,
        extra_config: Option<&str>,
        key_preview: Option<&str>,
        created_by: Option<&str>,
    ) -> Result<CredentialRow, DataError>;

    /// Update credential metadata (display_name, endpoint_url, extra_config).
    /// Uses `Option<Option<&str>>` to distinguish absent (don't change) from
    /// `Some(None)` (clear field) and `Some(Some(v))` (set to value).
    async fn update_credential(
        &self,
        id: &str,
        org_id: &str,
        display_name: Option<&str>,
        endpoint_url: Option<Option<&str>>,
        extra_config: Option<Option<&str>>,
    ) -> Result<Option<CredentialRow>, DataError>;

    /// Delete a credential row by id, scoped to org. Returns true if deleted.
    async fn delete_credential(&self, id: &str, org_id: &str) -> Result<bool, DataError>;
    /// List permissions for a credential
    async fn list_credential_permissions(
        &self,
        credential_id: &str,
    ) -> Result<Vec<CredentialPermissionRow>, DataError>;

    /// Create a credential project permission
    async fn create_credential_permission(
        &self,
        id: &str,
        credential_id: &str,
        org_id: &str,
        project_id: Option<&ProjectId>,
        access: &str,
        created_by: Option<&str>,
    ) -> Result<CredentialPermissionRow, DataError>;

    /// Delete a credential permission by id. Returns true if deleted.
    async fn delete_credential_permission(
        &self,
        id: &str,
        credential_id: &str,
    ) -> Result<bool, DataError>;

    /// Get credential IDs accessible by a specific project (for filtering).
    /// Returns credentials that are not denied for this project and either:
    /// - have no allow rules (accessible by default), or
    /// - have an allow rule for this project or the org-level default
    async fn get_credentials_accessible_by_project(
        &self,
        org_id: &str,
        project_id: &ProjectId,
    ) -> Result<Vec<String>, DataError>;
}

/// Favourites, keyed on the entity they mark.
#[async_trait]
pub trait FavoriteStore: Send + Sync {
    /// Add a favorite
    /// For spans, secondary_id is the span_id (entity_id is trace_id)
    async fn add_favorite(
        &self,
        user_id: &str,
        entity_type: &str,
        entity_id: &str,
        secondary_id: Option<&str>,
        project_id: &ProjectId,
    ) -> Result<bool, DataError>;

    /// Remove a favorite
    /// For spans, secondary_id is the span_id (entity_id is trace_id)
    async fn remove_favorite(
        &self,
        user_id: &str,
        entity_type: &str,
        entity_id: &str,
        secondary_id: Option<&str>,
        project_id: &ProjectId,
    ) -> Result<bool, DataError>;

    /// Check if entities are favorited
    async fn check_favorites(
        &self,
        user_id: &str,
        entity_type: &str,
        entity_ids: &[String],
        project_id: &ProjectId,
    ) -> Result<Vec<String>, DataError>;

    /// Check if spans are favorited
    async fn check_span_favorites(
        &self,
        user_id: &str,
        span_ids: &[(String, String)],
        project_id: &ProjectId,
    ) -> Result<Vec<(String, String)>, DataError>;

    /// Count favorites for a user
    async fn count_favorites(
        &self,
        user_id: &str,
        project_id: &ProjectId,
    ) -> Result<i64, DataError>;

    /// List all favorite entity IDs for a user
    async fn list_favorite_ids(
        &self,
        user_id: &str,
        entity_type: &str,
        project_id: &ProjectId,
    ) -> Result<Vec<String>, DataError>;

    /// Delete favorites by entity (for cascade delete)
    async fn delete_favorites_by_entity(
        &self,
        entity_type: &str,
        entity_ids: &[String],
        project_id: &ProjectId,
    ) -> Result<u64, DataError>;
}
