//! Media a signal's records carry, stored once per project by content hash: the I/O phase every signal shares.
//!
//! Extraction is each signal's own (a span's messages, a log record's bytes values); what happens to the bytes
//! after is the same for both - a temp write, metadata and a provisional `(project, trace, hash)` association
//! before the analytics write, finalisation, and the confirmation or compensation after it. One module, so the
//! two signals cannot drift apart on the protocol that keeps a dangling reference impossible.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use base64::prelude::*;
use futures::stream::StreamExt;
use sideseat_core::constants::{FILE_HASH_ALGORITHM, FILES_MAX_CONCURRENT_FINALIZATION};
use sideseat_core::utils::file_uri::is_valid_file_hash;
use sideseat_domain::files::FileService;
use sideseat_ports::types::ProjectId;

use crate::traces::extract::files::ExtractedFile;

/// File data extracted during CPU phase, pending I/O write.
///
/// Contains the file bytes and metadata needed to persist to storage.
/// Created during the CPU-only extraction phase, consumed by
/// `persist_extracted_files` in parallel with DuckDB write.
pub(crate) struct PendingFileWrite {
    pub project_id: String,
    pub trace_id: String,
    pub hash: String,
    pub media_type: Option<String>,
    pub size: usize,
    /// Raw base64 bytes (not decoded) for cache misses, empty for cache hits
    pub data: Vec<u8>,
    pub hash_algo: String,
}

/// What became of a batch's files.
///
/// Returned rather than logged, because the caller has a decision to make that a `warn!` cannot: a
/// span row carries `#!B64!#` references, so committing it when the file behind one is missing leaves
/// a reader with a reference to nothing. Backend parity cannot catch that - it proves DuckDB and
/// ClickHouse agree, never that either represents the OTLP input.
#[derive(Default)]
pub(crate) struct FilePersistOutcome {
    /// Files that could not be stored: decode, temp write, metadata or finalization failed. Transient
    /// in nature, so the honest response is to fail the batch and let the exporter retry.
    pub failed: usize,
    /// `(project_id, uri)` deliberately not stored because the project is over its storage quota. Not
    /// an error and not retryable, so the batch is still committed - but the references have to be
    /// *rewritten* first, because a reader cannot tell a reference to a rejected file from a broken one.
    ///
    /// The project is part of the key because a batch spans projects and a hash is content-addressed:
    /// the same URI can be rejected in one project and stored in another.
    pub quota_skipped: Vec<(String, String)>,
    /// `(project_id, trace_id, file_hash)` associations this batch created, and no earlier one.
    ///
    /// A file's bytes are written before the analytics row that references them, which is the ordering
    /// that keeps a dangling reference impossible. The cost is that a batch whose analytics write fails
    /// has already created associations for spans that will never land - and those hold `ref_count`
    /// above zero, so the orphan sweeper (which selects on `ref_count = 0`) can never reclaim them. The
    /// file then occupies the project's quota for good.
    ///
    /// Recorded so the failure path can release precisely what it created. Only the *new* ones: an
    /// association that already existed belongs to an earlier committed batch, and releasing it would
    /// orphan that batch's file. A redelivery re-creates these, so the compensation costs nothing when
    /// the retry succeeds.
    pub created_associations: Vec<(String, String, String)>,
}

/// File pending finalization to permanent storage
struct PendingFinalization {
    project_id: ProjectId,
    hash: String,
    temp_path: std::path::PathBuf,
}

/// Finalize pending files to permanent storage with bounded concurrency.
///
/// Uses `buffer_unordered` to limit concurrent I/O operations, preventing
/// resource exhaustion when processing batches with many files.
async fn finalize_pending_files(
    pending: Vec<PendingFinalization>,
    file_service: &FileService,
    context: &str,
) -> usize {
    if pending.is_empty() {
        return 0;
    }

    let count = pending.len();
    let storage = file_service.storage();

    // Use buffer_unordered for bounded concurrency instead of unbounded join_all
    let results: Vec<_> = futures::stream::iter(pending.into_iter().map(|pf| {
        let storage = Arc::clone(storage);
        async move {
            let result = storage
                .finalize_temp(&pf.project_id, &pf.hash, &pf.temp_path)
                .await;
            (pf, result)
        }
    }))
    .buffer_unordered(FILES_MAX_CONCURRENT_FINALIZATION)
    .collect()
    .await;

    let mut success_count = 0usize;
    let mut failure_count = 0usize;
    for (pf, result) in results {
        match result {
            Ok(()) => success_count += 1,
            Err(e) => {
                failure_count += 1;
                tracing::warn!(
                    error = %e,
                    hash = %pf.hash,
                    project_id = %pf.project_id,
                    "Failed to finalize file to permanent storage"
                );
            }
        }
    }

    if failure_count > 0 {
        tracing::warn!(
            finalized = success_count,
            failed = failure_count,
            total = count,
            context,
            "File finalization complete with failures"
        );
    } else if success_count > 0 {
        tracing::debug!(
            files = success_count,
            context,
            "Files finalized successfully"
        );
    }

    failure_count
}

/// Convert extracted files to pending writes for a given project/trace.
pub(crate) fn to_pending_files<'a>(
    files: Vec<ExtractedFile>,
    project_id: &'a str,
    trace_id: &'a str,
) -> impl Iterator<Item = PendingFileWrite> + 'a {
    files.into_iter().map(move |f| PendingFileWrite {
        project_id: project_id.to_string(),
        trace_id: trace_id.to_string(),
        hash: f.hash,
        media_type: f.media_type,
        size: f.size,
        data: f.data,
        hash_algo: FILE_HASH_ALGORITHM.to_string(),
    })
}

/// Persist extracted files to storage (I/O phase).
///
/// Handles temp file writes, metadata upserts, and finalization.
/// Deduplicates by hash within the batch to avoid redundant I/O.
/// Called in parallel with DuckDB write via tokio::join!.
pub(crate) async fn persist_extracted_files(
    files: Vec<PendingFileWrite>,
    file_service: &FileService,
) -> FilePersistOutcome {
    let mut outcome = FilePersistOutcome::default();
    if files.is_empty() {
        return outcome;
    }

    // Unified quota admission happened before this request was queued. A second file-only quota check here
    // used to silently replace accepted producer content with a placeholder, violating the export contract:
    // the request had been admitted, yet only part of it was stored. File bytes are already represented in the
    // encoded request reservation and reconciliation later measures the content-addressed object itself.
    let affected_projects = files
        .iter()
        .map(|file| file.project_id.as_str())
        .collect::<HashSet<_>>();

    // Decode objects, write temporary content, and record trace associations.
    let (pending_finalizations, write_failures, created_associations) =
        write_and_record_files(&files, file_service).await;
    outcome.created_associations = created_associations;

    // Promote temporary objects only after their metadata is durable.
    let finalize_failures =
        finalize_pending_files(pending_finalizations, file_service, "batch").await;
    outcome.failed = write_failures + finalize_failures;

    // Invalidate quota measurements for every project whose associations changed.
    let affected_projects: Vec<&str> = affected_projects.into_iter().collect();
    file_service
        .invalidate_quota_cache(&affected_projects)
        .await;

    outcome
}

/// Decode base64, write temp files, upsert metadata, and record trace associations.
async fn write_and_record_files(
    files: &[PendingFileWrite],
    file_service: &FileService,
) -> (
    Vec<PendingFinalization>,
    usize,
    Vec<(String, String, String)>,
) {
    let temp_dir = file_service.temp_dir();
    let repo = file_service.database().as_ref();
    let mut pending_finalizations: Vec<PendingFinalization> = Vec::new();

    // Deduplicate: only write/upsert once per unique (project_id, hash)
    let mut written_hashes: HashSet<String> = HashSet::new();
    // Deduplicate: once per unique (project, trace, hash).
    //
    // Keyed without the project this suppressed one project's association whenever another project in
    // the same batch had already claimed the same trace id and hash - the same collision the database
    // primary key had, and widening only the key would have left this one in place.
    let mut trace_hashes: HashSet<(String, String, String)> = HashSet::new();
    // Anything that did not reach storage: the row that references it must not be committed.
    let mut failures = 0usize;
    // Associations this batch created, so a failed analytics write can release exactly its own.
    let mut created: Vec<(String, String, String)> = Vec::new();

    let mut cache_hits = 0usize;
    let mut fresh_ok = 0usize;

    // Process files with data (cache misses) before empty-data entries (cache hits).
    // When concurrent extraction threads race on the shared FileExtractionCache,
    // the thread that runs first gets a cache miss (with data) while later threads
    // get hits (empty data). Since results are collected in request order (not
    // execution order), a cache-hit entry may appear before the data-bearing entry
    // for the same file. Processing data-first ensures the fresh entry always wins
    // dedup, preventing files from being silently skipped.
    for file in files
        .iter()
        .filter(|f| !f.data.is_empty())
        .chain(files.iter().filter(|f| f.data.is_empty()))
    {
        let dedup_key = format!("{}:{}", file.project_id, file.hash);
        let is_new = written_hashes.insert(dedup_key);

        if is_new {
            if file.data.is_empty() {
                // Unreachable by design, and a failure rather than an assumption. This branch used to
                // mean "cache hit, so the bytes are already in storage" - a claim the extraction cache
                // could not support, since it is global and keyed on the source text. Cache hits now
                // carry their bytes, so no data means a producer built a `PendingFileWrite` without
                // any, and writing a reference to nothing is exactly what must not happen.
                cache_hits += 1;
                failures += 1;
                tracing::error!(
                    hash = %file.hash,
                    project_id = %file.project_id,
                    "Pending file has no data; refusing to record a reference to it"
                );
                // And *stop*. Counting the failure while falling through to `associate_file` still
                // recorded the association and the reference - the exact thing being refused.
                continue;
            } else {
                // Fresh extraction: decode base64, write temp file, upsert metadata, finalize
                let decoded = match BASE64_STANDARD.decode(&file.data) {
                    Ok(d) => d,
                    Err(e1) => match BASE64_URL_SAFE.decode(&file.data) {
                        Ok(d) => d,
                        Err(e) => {
                            failures += 1;
                            tracing::warn!(
                                error = %e,
                                std_error = %e1,
                                hash = %file.hash,
                                data_len = file.data.len(),
                                project_id = %file.project_id,
                                "Failed to decode base64, skipping file"
                            );
                            continue;
                        }
                    },
                };

                let temp_path = temp_dir.join(format!("{}_{}", file.project_id, file.hash));
                if let Err(e) = tokio::fs::write(&temp_path, &decoded).await {
                    failures += 1;
                    tracing::warn!(
                        error = %e,
                        hash = %file.hash,
                        project_id = %file.project_id,
                        "Failed to write temp file, skipping"
                    );
                    continue;
                }

                // Metadata and the reference count are recorded below, once per trace association.
                fresh_ok += 1;
                pending_finalizations.push(PendingFinalization {
                    project_id: file.project_id.clone().into(),
                    hash: file.hash.clone(),
                    temp_path,
                });
            }
        } else {
            // Duplicate within batch, already handled above
        }

        // One reference count per trace association, because that is what deletion decrements.
        //
        // `upsert_file` increments `ref_count`, and it used to be called once per batch-unique
        // `(project, hash)` while associations are per `(trace, hash)`. So two traces of one batch
        // sharing a file got two associations and *one* count - and deleting either trace decremented
        // it to zero, deleting a file the other trace still referenced. The invariant is `ref_count`
        // equals the number of associations; incrementing here, beside the association, is what keeps
        // it. The storage write above stays once per `(project, hash)`: the bytes are the same bytes.
        let trace_key = (
            file.project_id.clone(),
            file.trace_id.clone(),
            file.hash.clone(),
        );
        if trace_hashes.insert(trace_key.clone()) {
            match repo
                .associate_file(
                    &file.trace_id,
                    &ProjectId::from(file.project_id.as_str()),
                    &file.hash,
                    file.media_type.as_deref(),
                    file.size as i64,
                    &file.hash_algo,
                )
                .await
            {
                // Every reference is remembered, not only a newly-created row. Each one incremented the
                // association's in-flight writer count, so each must be resolved by exactly one confirm (on
                // success) or release (on failure) - a batch that merely *shares* an existing association is
                // now one of its owners, and the release only deletes a non-durable row once the last owner
                // is gone, so tracking a shared one can no longer orphan the batch that created it.
                Ok(true) => created.push(trace_key),
                Ok(false) => {}
                Err(e) => {
                    failures += 1;
                    tracing::warn!(
                        error = %e,
                        hash = %file.hash,
                        trace_id = %file.trace_id,
                        "Failed to associate file with trace"
                    );
                }
            }
        }
    }

    tracing::debug!(
        total = files.len(),
        unique = written_hashes.len(),
        fresh = fresh_ok,
        cached = cache_hits,
        pending = pending_finalizations.len(),
        "File write summary"
    );

    (pending_finalizations, failures, created)
}

/// Mark this batch's associations durable, reporting a confirmation that matched fewer rows than it had.
///
/// A shortfall means an association this batch owns is **not there** - something removed it between the
/// file write and here - so the span row now committed carries a `#!B64!#` reference whose file nothing
/// holds, and the orphan sweeper reclaims on zero references. That is the dangling reference the
/// write-files-before-rows ordering exists to prevent, so it cannot be inferred from a bare `Ok`: the
/// count is the only evidence, and it was previously discarded.
///
/// Reported rather than repaired. Re-creating the row would point it at bytes that may already have been
/// collected, turning a detectable inconsistency into an undetectable one; the deletion sweep converges
/// on the trace either way. What this buys is that the case is *visible* if it ever happens, instead of
/// being the silent `Ok(0)` it was.
pub(crate) async fn confirm_associations(
    file_service: &FileService,
    created: &[(String, String, String)],
    path: &str,
) {
    if created.is_empty() {
        return;
    }
    match file_service
        .database()
        .as_ref()
        .confirm_trace_file_associations(created)
        .await
    {
        Ok(confirmed) if confirmed < created.len() as u64 => tracing::error!(
            path,
            expected = created.len(),
            confirmed,
            "Fewer file associations were confirmed than this batch owns; a committed span may reference                  a file that nothing holds. Something removed the association between the file write and the                  confirmation."
        ),
        Ok(_) => {}
        Err(e) => tracing::warn!(
            error = %e,
            path,
            "Could not confirm this batch's file associations as durable; they keep a pending writer and                  a later failure path could release them"
        ),
    }
}

/// Release associations this batch referenced, decrementing its own writer.
///
/// # Why a dropped trace's association is not deleted outright here
///
/// A crashed writer's increment is never decremented, so a decrement-to-zero release cannot reclaim it
/// and the file's quota is held. Deleting the non-durable row outright *from this path* was tried and
/// reverted: it is unsafe. `durable = false` does not prove no analytics row committed - confirmation runs
/// after the write and its failure is only logged - and a concurrent batch that passed the fence before
/// the tombstone can be committing spans for the same trace right now. Deleting the shared row then
/// leaves that batch's readable spans pointing at a file with no association, which the orphan sweeper
/// may reclaim: a dangling reference, the one outcome the write-files-before-rows ordering exists to
/// prevent.
///
/// The reclamation belongs where the information is. `advance_pending_deletions` deletes the trace's
/// rows, confirms by direct read that nothing is still readable, and *only then* calls
/// `FileService::cleanup_traces`, which removes every association for the trace regardless of
/// `pending_writers` and recomputes the reference counts. So a stuck increment on a tombstoned trace is
/// reclaimed by that sweep - leased, backed off, and retried until the rows are provably gone - and this
/// path only ever undoes its own reference.
///
/// Best effort and logged rather than fatal: the batch has already failed and will be redelivered,
/// so the useful thing is to leave as little behind as possible. A release that itself fails leaves
/// the association, which is the state this exists to avoid - so it is worth a warning, but not worth
/// turning a retryable batch into a different failure.
///
/// `sync_ref_count` follows each release, recomputing the count from the associations that remain
/// rather than subtracting - so a concurrent batch holding its own association keeps the file, and
/// the count cannot drift however the two interleave.
pub(crate) async fn release_created_associations(
    file_service: &FileService,
    created: &[(String, String, String)],
) {
    if created.is_empty() {
        return;
    }
    let repo = file_service.database().as_ref();
    let mut released = 0usize;
    for (project_id, trace_id, hash) in created {
        let typed_project_id = ProjectId::from(project_id.as_str());
        // No read here, deliberately.
        //
        // An earlier version asked the analytics store whether the trace had rows and skipped the
        // release if it did - which is a read-then-act pair, and therefore not a decision at all: a
        // second batch can commit between the read and the delete, and its file loses its protection.
        // The `provisional` marker replaces that with a single statement. The delete matches only rows
        // still marked provisional, and a batch that committed has already cleared the marker, so
        // "another batch owns this now" is a *stored fact* rather than something observed and hoped to
        // still hold.
        match repo
            .release_trace_file_association(&typed_project_id, trace_id, hash)
            .await
        {
            Ok(true) => {
                released += 1;
                if let Err(e) = repo.sync_ref_count(&typed_project_id, hash).await {
                    tracing::warn!(
                        error = %e,
                        project_id, hash,
                        "Released an association but could not recompute the file's reference count"
                    );
                }
            }
            Ok(false) => {}
            Err(e) => tracing::warn!(
                error = %e,
                project_id, trace_id, hash,
                "Could not release an association after a failed analytics write; the file it \
                 references will hold quota until the association is removed"
            ),
        }
    }
    if released > 0 {
        tracing::debug!(
            released,
            "Released file associations created by a batch whose analytics write failed"
        );
    }
}

/// Settle associations whose rows this batch did not write itself, by the rows that are stored.
///
/// Two callers have such associations. A failed write: a failed ClickHouse insert can still have landed its
/// rows, so its associations cannot be released on the failure alone - if the exporter never retried, its
/// readable rows would name files the sweeper then reclaims. And an exact redelivery, whose rows are already
/// in place: releasing what it re-created would undo the one thing a redelivery can repair, an association
/// an earlier failure lost.
///
/// Each trace's stored references are read, and an association they name is confirmed - those rows are
/// committed - while the rest are released. The read-then-act shape that `release_created_associations`
/// avoids is sound here only because the write in question has finished: none of its rows can land after
/// the read, and a concurrent batch's association holds its own pending count, which this does not touch.
/// A project whose write is `in_doubt` has not finished, so its associations are kept pending - referenced or not:
/// confirming one spends the protection the outstanding write may still need once another record naming the file
/// has gone - and so are all of a trace's whose read fails: an association kept too long holds quota, one
/// released under a live row loses the file.
pub(crate) async fn settle_associations_by_stored_rows(
    file_service: &FileService,
    analytics: &dyn sideseat_ports::traits::SurvivorReferences,
    associations: &[(String, String, String)],
    in_doubt: &HashSet<String>,
) {
    // In order, so the confirmations and releases are written in the same order every time.
    let mut by_trace = BTreeMap::new();
    for association in associations {
        by_trace
            .entry((association.0.as_str(), association.1.as_str()))
            .or_insert_with(Vec::new)
            .push(association);
    }
    let mut confirm = Vec::new();
    let mut release = Vec::new();
    let mut kept = 0usize;
    for ((project_id, trace_id), associations) in by_trace {
        match sideseat_domain::files::FileService::hashes_referenced_by_trace(
            &ProjectId::from(project_id),
            trace_id,
            analytics,
        )
        .await
        {
            Ok(referenced) => {
                for association in associations {
                    // Doubt first: a write still in doubt may land a row naming this file after the record that
                    // names it now has gone, so neither a confirmation nor a release may spend its protection.
                    if in_doubt.contains(project_id) {
                        kept += 1;
                    } else if referenced.contains(&association.2) {
                        confirm.push(association.clone());
                    } else {
                        release.push(association.clone());
                    }
                }
            }
            Err(error) => {
                kept += associations.len();
                tracing::error!(
                    %error,
                    project_id,
                    trace_id,
                    "Could not read which file references are stored; keeping the associations pending"
                );
            }
        }
    }
    if kept > 0 {
        tracing::warn!(
            kept,
            "File associations of a write that may still land are kept pending: released now, a row landing \
             later would name a file the sweeper had reclaimed"
        );
    }
    confirm_associations(file_service, &confirm, "stored rows").await;
    release_created_associations(file_service, &release).await;
}

/// A `#!B64!#` reference that arrived already formed: `(project, trace, uri)`.
///
/// The trace is carried because a reference that *is* backed by a stored file still needs an
/// association, or deleting the trace will not release it and the file outlives everything naming it.
pub(crate) type IncomingReference = (String, String, String);

/// Check references that arrived already formed against storage, and reconcile them.
///
/// A `#!B64!#` reference is a claim that bytes are in this project's file store. Extraction created
/// most of them and their bytes are in this batch - but a client can also send one directly, or replay
/// one from another project, and nothing verified it. Committing an unverified reference is the same
/// user-visible corruption as committing one whose write failed.
///
/// Storage is the authority, not the metadata table: a row can survive a failed finalisation, so
/// `file_exists` would answer yes for bytes that are not there.
///
/// A reference that *is* backed gets an association, because otherwise deleting the trace will not
/// release it and the file outlives everything that named it. One that is not gets returned, for the
/// caller to replace with a note.
pub(crate) async fn reconcile_incoming_references(
    incoming: &[IncomingReference],
    file_service: &FileService,
    already_associated: &[(String, String, String)],
) -> (Vec<(String, String)>, usize, Vec<(String, String, String)>) {
    let mut unbacked = Vec::new();
    let mut failed = 0usize;
    // Associations this call created, so the batch's compensation covers a reference that arrived already
    // formed as well as one whose bytes this batch wrote. Both hold a file's reference count above zero, and
    // both belong to a span that may never commit.
    let mut created: Vec<(String, String, String)> = Vec::new();
    if incoming.is_empty() {
        return (unbacked, failed, created);
    }
    let storage = file_service.storage();
    let repo = file_service.database().as_ref();
    let mut checked: HashMap<(String, String), bool> = HashMap::new();
    // One association per `(project, trace, hash)` per batch, even when two references name the same content
    // through different URIs (e.g. differing MIME prefixes). Each `associate_existing_file` increments
    // `pending_writers`, and the batch resolves each association once - so a second increment for the same
    // tuple would never be decremented on one backend: SQLite's confirm loops per element (decrementing
    // twice), while PostgreSQL's `UNNEST` `IN` touches the row once (decrementing once), drifting the
    // counter apart. Associating once per tuple keeps increment and resolution matched, and is the correct
    // model regardless: one batch is one referencing writer.
    // Seeded with the associations the *file-write* path already made this batch, so a reference naming
    // content this batch just stored does not increment `pending_writers` a second time.
    //
    // `prepare_batch` exempts an incoming reference only when the whole URI matches one it generated -
    // correct for the unbacked-rewrite it also has to do - so a URI carrying the same hash under a different
    // media type (`image/jpeg::H` beside our `image/png::H`) still arrives here. Associating it again made
    // two increments for one `(project, trace, hash)`, while the batch resolves each association once: the
    // surplus increment was never decremented, so the row stayed non-durable at `pending_writers = 1` and
    // held the file's quota permanently. Deduping the merged list afterwards did not fix that - it removed
    // the second *resolution*, not the second increment. One batch is one referencing writer, enforced where
    // the increment happens.
    let mut associated: std::collections::HashSet<(String, String, String)> =
        already_associated.iter().cloned().collect();

    for (project_id, trace_id, uri) in incoming {
        let typed_project_id = ProjectId::from(project_id.as_str());
        let Some(parsed) = sideseat_core::utils::file_uri::parse_file_uri(uri) else {
            continue;
        };
        if !is_valid_file_hash(parsed.hash) {
            tracing::warn!(
                project_id,
                hash = parsed.hash,
                "Incoming file reference has an invalid content hash"
            );
            unbacked.push((project_id.clone(), uri.clone()));
            continue;
        }
        let key = (project_id.clone(), parsed.hash.to_string());
        let exists = match checked.get(&key) {
            Some(known) => *known,
            None => match storage.exists(&typed_project_id, parsed.hash).await {
                Ok(found) => {
                    checked.insert(key, found);
                    found
                }
                // A storage error is not an answer. Reading it as "not found" replaced a *valid*
                // reference with a permanent note over a transient failure - the reference cannot be
                // recovered afterwards, while the batch can be retried.
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        project_id,
                        hash = parsed.hash,
                        "Could not determine whether an incoming file reference is backed"
                    );
                    failed += 1;
                    continue;
                }
            },
        };

        if !exists {
            tracing::warn!(
                project_id,
                hash = parsed.hash,
                "Incoming file reference names content this project does not store"
            );
            unbacked.push((project_id.clone(), uri.clone()));
            continue;
        }

        // Already associated this batch through another URI: the reference is backed (checked above), so
        // nothing more is owed for it - associating again would double-count the writer.
        if !associated.insert((
            project_id.clone(),
            trace_id.clone(),
            parsed.hash.to_string(),
        )) {
            continue;
        }

        // The association is made in one operation that requires the metadata row to exist already.
        //
        // Checking for the row and *then* associating is a race - the row can vanish in between - and
        // `associate_file` would have created one with size 0, undercounting the project's quota forever.
        // Neither the size nor the media type is a fact this process has for a reference it did not
        // produce, so it does not supply them. A file that is missing, or claimed for deletion, refuses:
        // both mean the reference must not be committed.
        match repo
            .associate_existing_file(trace_id, &typed_project_id, parsed.hash)
            .await
        {
            Ok(true) => created.push((
                project_id.clone(),
                trace_id.clone(),
                parsed.hash.to_string(),
            )),
            // Bytes without metadata, or a file mid-deletion. Not something to resolve either way here:
            // rewriting would be wrong if it is only being deleted, and committing would be wrong if the
            // bytes are about to go.
            Ok(false) => {
                failed += 1;
                tracing::warn!(
                    project_id,
                    hash = parsed.hash,
                    "Incoming file reference has no metadata or is being deleted"
                );
            }
            // Without the association, deleting this trace never releases the file and it outlives
            // everything that named it. A leak is not something to commit and move on from.
            Err(e) => {
                failed += 1;
                tracing::warn!(
                    error = %e,
                    project_id,
                    trace_id,
                    hash = parsed.hash,
                    "Failed to associate an incoming file reference with its trace"
                );
            }
        }
    }

    unbacked.sort_unstable();
    unbacked.dedup();
    (unbacked, failed, created)
}
