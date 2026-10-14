//! Fence, validate and persist OTLP logs before acknowledgement.

use std::collections::HashSet;

use chrono::{DateTime, Utc};
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use sideseat_core::constants::DEFAULT_PROJECT_ID;
use sideseat_core::utils::retry::{DEFAULT_BASE_DELAY_MS, DEFAULT_MAX_ATTEMPTS};
use sideseat_core::utils::time::is_storable;
use sideseat_ports::traits::{AnalyticsRepository, TransactionalRepository};
use sideseat_ports::types::ProjectId;

use super::extract::{extract_logs_batch, extract_logs_batch_with_media};
use sideseat_domain::files::FileService;
use sideseat_domain::storage_governance::StorageGovernanceService;

pub struct Stored {
    pub stored: usize,
    pub total: usize,
    pub gone: usize,
    pub unstorable: usize,
}

/// Store a logs export. With a file service, the records' media is stored by content hash and referenced
/// (`super::media`); without one, it stays inline.
pub async fn ingest(
    request: &ExportLogsServiceRequest,
    analytics: &(dyn AnalyticsRepository + Send + Sync),
    database: &(dyn TransactionalRepository + Send + Sync),
    files: Option<&FileService>,
    received_at: DateTime<Utc>,
) -> Result<Stored, String> {
    ingest_inner(request, analytics, database, files, received_at, None).await
}

pub async fn ingest_governed(
    request: &ExportLogsServiceRequest,
    analytics: &(dyn AnalyticsRepository + Send + Sync),
    database: &(dyn TransactionalRepository + Send + Sync),
    files: Option<&FileService>,
    received_at: DateTime<Utc>,
    governance: &StorageGovernanceService,
) -> Result<Stored, String> {
    ingest_inner(
        request,
        analytics,
        database,
        files,
        received_at,
        Some(governance),
    )
    .await
}

async fn ingest_inner(
    request: &ExportLogsServiceRequest,
    analytics: &(dyn AnalyticsRepository + Send + Sync),
    database: &(dyn TransactionalRepository + Send + Sync),
    file_service: Option<&FileService>,
    received_at: DateTime<Utc>,
    governance: Option<&StorageGovernanceService>,
) -> Result<Stored, String> {
    let (mut logs, media) = match file_service.filter(|service| service.is_enabled()) {
        Some(_) => extract_logs_batch_with_media(request, received_at),
        None => (extract_logs_batch(request, received_at), Vec::new()),
    };
    let total = logs.len();
    let mut gone = 0;

    let mut projects: Vec<&str> = logs
        .iter()
        .map(|log| log.project_id.as_deref().unwrap_or(DEFAULT_PROJECT_ID))
        .collect();
    projects.sort_unstable();
    projects.dedup();
    let mut refusing = HashSet::new();
    for project in projects {
        match database.project_accepts_writes(project).await {
            Ok(true) => {}
            Ok(false) => {
                refusing.insert(project.to_string());
            }
            Err(error) => return Err(format!("could not read the project fence: {error}")),
        }
    }
    if !refusing.is_empty() {
        let before = logs.len();
        logs.retain(|log| {
            !refusing.contains(log.project_id.as_deref().unwrap_or(DEFAULT_PROJECT_ID))
        });
        gone = before - logs.len();
    }

    let before = logs.len();
    logs.retain(|log| {
        is_storable(log.timestamp)
            && log.time.map(is_storable).unwrap_or(true)
            && log.observed_time.map(is_storable).unwrap_or(true)
    });
    let unstorable = before - logs.len();

    if !logs.is_empty() {
        // Files first, then the rows that name them: the ordering that makes a dangling reference impossible,
        // as for spans (`crate::media`). Only the files a stored row still names - a record the fences dropped
        // holds no association.
        let mut created = Vec::new();
        if let Some(file_service) = file_service {
            created = store_media(file_service, &logs, media).await?;
            match verify_incoming_references(file_service, &mut logs, &created).await {
                Ok(verified) => created.extend(verified),
                // Every association this export holds goes with the export: the stored media's as well as the
                // verification's own, or a retry's second increment would be confirmed once and the first held.
                Err(error) => {
                    crate::media::release_created_associations(file_service, &created).await;
                    return Err(error);
                }
            }
        }
        match write_logs(&mut logs, analytics, governance).await {
            Ok(in_doubt) => {
                if let Some(file_service) = file_service {
                    compensate_deleted_traces(file_service, analytics, &mut created, &in_doubt)
                        .await;
                    confirm_settled(file_service, &created, &in_doubt).await;
                }
            }
            Err(failure) => {
                if let Some(file_service) = file_service {
                    // A write that never began holds nothing a row could name; one that did may still have
                    // landed rows, so its associations are settled by what is stored - and kept pending only
                    // where the store says the write is in doubt, since released under a live row a file is lost.
                    match &failure {
                        WriteFailure::AfterWrite { in_doubt, .. } => {
                            compensate_deleted_traces(
                                file_service,
                                analytics,
                                &mut created,
                                in_doubt,
                            )
                            .await;
                            confirm_settled(file_service, &created, in_doubt).await;
                        }
                        WriteFailure::BeforeWrite(_) => {
                            crate::media::release_created_associations(file_service, &created)
                                .await;
                        }
                        WriteFailure::Write { in_doubt, .. } => {
                            // A failed write may still have landed rows, so a trace deleted in flight loses
                            // them and their associations here too, before the rest are settled.
                            compensate_deleted_traces(
                                file_service,
                                analytics,
                                &mut created,
                                in_doubt,
                            )
                            .await;
                            crate::media::settle_associations_by_stored_rows(
                                file_service,
                                analytics,
                                &created,
                                in_doubt,
                            )
                            .await;
                        }
                    }
                }
                return Err(failure.to_string());
            }
        }
    }
    Ok(Stored {
        stored: logs.len(),
        total,
        gone,
        unstorable,
    })
}

/// Store the media the surviving rows name, returning the associations this export created. A file that could
/// not be stored fails the export, after releasing what it created: the exporter retries, and a row naming bytes
/// that are not there cannot be repaired by a later delivery.
async fn store_media(
    file_service: &FileService,
    logs: &[sideseat_ports::types::NormalizedLog],
    media: Vec<crate::media::PendingFileWrite>,
) -> Result<Vec<(String, String, String)>, String> {
    // Keyed by trace as an association is: two traces sharing bytes are two owners, and a dropped record's trace
    // must not keep one on the strength of another trace's surviving record.
    let mut named = HashSet::new();
    for log in logs {
        let (Some(trace), project) = (
            log.trace_id.as_deref(),
            log.project_id.as_deref().unwrap_or(DEFAULT_PROJECT_ID),
        ) else {
            continue;
        };
        for uri in &log.media {
            if let Some(parsed) = sideseat_core::utils::file_uri::parse_file_uri(uri) {
                named.insert((
                    project.to_string(),
                    trace.to_string(),
                    parsed.hash.to_string(),
                ));
            }
        }
    }
    let media: Vec<_> = media
        .into_iter()
        .filter(|file| {
            named.contains(&(
                file.project_id.clone(),
                file.trace_id.clone(),
                file.hash.clone(),
            ))
        })
        .collect();
    if media.is_empty() {
        return Ok(Vec::new());
    }
    let outcome = crate::media::persist_extracted_files(media, file_service).await;
    if outcome.failed > 0 || !outcome.quota_skipped.is_empty() {
        crate::media::release_created_associations(file_service, &outcome.created_associations)
            .await;
        return Err(format!(
            "{} of the export's media files could not be stored",
            outcome.failed + outcome.quota_skipped.len()
        ));
    }
    Ok(outcome.created_associations)
}

/// References a producer wrote into its own content, verified as span content's are
/// (`crate::media::reconcile_incoming_references`): one this project's store holds gains its trace's association,
/// and one it does not - or that names a record with no trace to own it - is replaced with a note, because a reader
/// cannot tell a reference to nothing from a broken image. Project-scoped: bytes another project holds are not this
/// one's. Returns the associations created, which the write confirms or settles with the media's own.
async fn verify_incoming_references(
    file_service: &FileService,
    logs: &mut [sideseat_ports::types::NormalizedLog],
    already_associated: &[(String, String, String)],
) -> Result<Vec<(String, String, String)>, String> {
    let mut incoming = Vec::new();
    for log in logs.iter() {
        let (Some(trace), project) = (
            log.trace_id.as_deref(),
            log.project_id.as_deref().unwrap_or(DEFAULT_PROJECT_ID),
        ) else {
            continue;
        };
        for uri in derived_references(log) {
            if !log.media.contains(&uri) {
                incoming.push((project.to_string(), trace.to_string(), uri));
            }
        }
    }
    incoming.sort_unstable();
    incoming.dedup();
    let (unbacked, failed, created) =
        crate::media::reconcile_incoming_references(&incoming, file_service, already_associated)
            .await;
    if failed > 0 {
        crate::media::release_created_associations(file_service, &created).await;
        return Err(format!(
            "could not settle whether {failed} of the export's media references are backed"
        ));
    }
    let unbacked: HashSet<(String, String)> = unbacked.into_iter().collect();
    for log in logs.iter_mut() {
        let project = log
            .project_id
            .as_deref()
            .unwrap_or(DEFAULT_PROJECT_ID)
            .to_string();
        let held = derived_references(log);
        let mut missing = Vec::new();
        for uri in held {
            if log.media.contains(&uri) {
                continue;
            }
            // A record with no trace has nothing to own the bytes under, so its reference is unbacked whatever the
            // store holds - in this record only: another record's same reference may be backed, and its own.
            if log.trace_id.is_none() || unbacked.contains(&(project.clone(), uri.clone())) {
                missing.push(uri);
            } else {
                // Verified: the record names it as surely as its own media, so the release must see it.
                log.media.push(uri);
            }
        }
        note_unbacked(log, &missing);
    }
    Ok(created)
}

/// Every `#!B64!#` reference a log's derived values hold. Values only: a key is the producer's own text, never where
/// content says what it holds, so a key spelled like a reference is neither verified nor replaced. The body text is
/// the body rendered, keys included, so the body's values stand for it.
fn derived_references(log: &sideseat_ports::types::NormalizedLog) -> Vec<String> {
    fn values(value: &serde_json::Value, uris: &mut Vec<String>) {
        let mut pending = vec![value];
        while let Some(value) = pending.pop() {
            match value {
                serde_json::Value::String(text) => {
                    sideseat_domain::files::collect_file_references_in_str(text, uris)
                }
                serde_json::Value::Object(members) => pending.extend(members.values()),
                serde_json::Value::Array(items) => pending.extend(items.iter()),
                _ => {}
            }
        }
    }
    let mut uris = Vec::new();
    values(&log.body, &mut uris);
    values(&log.attributes, &mut uris);
    if let Some(messages) = log.messages.as_deref() {
        match serde_json::from_str::<serde_json::Value>(messages) {
            Ok(parsed) => values(&parsed, &mut uris),
            // Deeper than the parser reads: the messages are derived again from the raw form, as a structure.
            Err(_) => match super::media::derivation_record(&log.raw_log) {
                Some(record) => {
                    let messages = sideseat_domain::log_messages::log_record_messages(
                        &super::content::log_record_content(&record),
                        log.scope_name.as_deref(),
                    );
                    values(
                        &serde_json::to_value(&messages).unwrap_or_default(),
                        &mut uris,
                    );
                }
                None => sideseat_domain::files::collect_file_references_in_str(messages, &mut uris),
            },
        }
    }
    // A reference names a file only through a content hash. One whose hash is no content hash names nothing: it is
    // the producer's text, never verified or noted - a note there could join with what decoding puts after it, as
    // JSON text spelling a longer hash's last digit `\u0061` does, and corrupt the reference it was part of.
    uris.retain(|uri| {
        sideseat_core::utils::file_uri::parse_file_uri(uri)
            .is_some_and(|parsed| sideseat_core::utils::file_uri::is_valid_file_hash(parsed.hash))
    });
    uris.sort_unstable();
    uris.dedup();
    uris
}

/// Replace each of a record's unbacked references with a note saying what is missing: in the record its columns
/// are derived from, rebuilt from the raw form, and every derived column re-derived - so the body, its text and
/// the messages agree, and a producer's key keeps its text in each. The raw form keeps the producer's string.
pub(super) fn note_unbacked(log: &mut sideseat_ports::types::NormalizedLog, missing: &[String]) {
    if missing.is_empty() {
        return;
    }
    log.noted_references = missing.to_vec();
    let notes: Vec<(String, String)> = missing
        .iter()
        .map(|uri| {
            (
                uri.clone(),
                sideseat_domain::files::missing_content_note(uri),
            )
        })
        .collect();
    match super::media::derivation_record(&log.raw_log) {
        Some(mut record) => {
            super::media::note_references(&mut record, &notes);
            let derived = super::extract::derive_content_noted(
                &record,
                log.scope_name.as_deref(),
                log.project_id.as_deref(),
                log.trace_id.as_deref(),
                &notes,
            );
            log.body = derived.body;
            log.body_text = derived.body_text;
            log.attributes = derived.attributes;
            log.messages = derived.messages;
            // The frame key stays the one the producer's content stated: it joins this record to the request span
            // that names the same frame, and a note is a display rewrite, never a change of identity.
            // What re-derivation could not reach in the content - a reference only decoding reveals, as in JSON
            // text that spells `#` as `\u0023` - was replaced in the messages as they were built, and is replaced
            // where it lands in the structured columns.
            rewrite_json(&mut log.body, &notes);
            rewrite_json(&mut log.attributes, &notes);
        }
        // A raw form that does not parse back cannot be re-derived, so the structured columns are rewritten by their
        // values, and the body's text - a rendering that cannot tell a key from a value - is dropped rather than
        // rewritten: search and every view read the noted body in its place, so the two never disagree.
        None => {
            rewrite_values(log, &notes);
            log.body_text = None;
        }
    }
    // The record's charge follows what is stored.
    log.logical_bytes = crate::accounting::log_logical_bytes(log);
}

/// Whether the record's raw form, read back as it is stored, rebuilds its body, the body's text and its attributes
/// exactly - with the references it noted - so none is stored (`domain::log_views`), and its messages
/// (`domain::log_messages`). Checked against the stored text, not the
/// value in hand: a number the text does not give back exactly makes the views stored, never wrong.
/// How much larger than its messages a raw record may be for the messages to be rebuilt from it rather than stored.
const REBUILT_RAW_FACTOR: usize = 2;
const REBUILT_RAW_SLACK: usize = 1024;

pub(super) fn settle_views(log: &mut sideseat_ports::types::NormalizedLog) {
    let derived = sideseat_domain::log_views::LogViews {
        body: sideseat_core::utils::json::json_to_opt_string(&log.body),
        body_text: log.body_text.clone(),
        attributes: sideseat_core::utils::json::json_to_opt_string(&log.attributes),
    };
    let raw = sideseat_core::utils::json::json_to_opt_string(&log.raw_log);
    log.views_rebuild_from_raw = raw.as_deref().is_some_and(|raw| {
        sideseat_domain::log_views::rebuilds(raw, &log.noted_references, &derived)
    });
    // The messages too, by the one derivation a read rebuilds them with - but kept where they hold a file reference:
    // a release reads a record's stored messages for those, and never derives them.
    let mut held = Vec::new();
    if let Some(messages) = log.messages.as_deref() {
        sideseat_domain::files::collect_file_references_in_str(messages, &mut held);
    }
    log.messages_rebuild_from_raw = held.is_empty()
        && raw.as_deref().is_some_and(|raw| {
            // A read of a record's messages - a span's view, a request's frames, bounded by their bytes - fetches
            // the raw record in their place, so only one not much larger than they are: what a read transfers stays
            // within a constant of the bound it budgets, however much else the record holds.
            // The raw record and everything else a rebuild reads beside it.
            let messages = log.messages.as_deref().map_or(0, str::len);
            let inputs = raw.len()
                + log.scope_name.as_deref().map_or(0, str::len)
                + log.noted_references_column().map_or(0, |noted| noted.len());
            inputs <= REBUILT_RAW_FACTOR * messages + REBUILT_RAW_SLACK
        })
        && raw.as_deref().is_some_and(|raw| {
            sideseat_domain::log_messages::rebuild_messages(
                raw,
                log.scope_name.as_deref(),
                &log.noted_references,
            )
            .as_ref()
                == Some(&log.messages)
        });
}

/// Replace each reference with its note in every string value of a JSON value - keys keep their text.
fn rewrite_json(value: &mut serde_json::Value, notes: &[(String, String)]) {
    let mut pending = vec![value];
    while let Some(value) = pending.pop() {
        match value {
            serde_json::Value::String(text) => {
                for (uri, note) in notes {
                    if let Some(rewritten) =
                        sideseat_domain::files::replace_file_reference(text, uri, note)
                    {
                        *text = rewritten;
                    }
                }
            }
            serde_json::Value::Object(members) => pending.extend(members.values_mut()),
            serde_json::Value::Array(items) => pending.extend(items.iter_mut()),
            _ => {}
        }
    }
}

/// The derived columns rewritten as they are, where the raw form cannot be derived again: the structured ones by
/// their values; the messages by theirs where the text parses, and as text otherwise - an unbacked reference must
/// not be stored, whatever the rewrite costs.
fn rewrite_values(log: &mut sideseat_ports::types::NormalizedLog, notes: &[(String, String)]) {
    rewrite_json(&mut log.body, notes);
    rewrite_json(&mut log.attributes, notes);
    if let Some(messages) = log.messages.as_mut() {
        match serde_json::from_str::<serde_json::Value>(messages) {
            Ok(mut parsed) => {
                rewrite_json(&mut parsed, notes);
                *messages = parsed.to_string();
            }
            Err(_) => {
                for (uri, note) in notes {
                    if let Some(rewritten) =
                        sideseat_domain::files::replace_file_reference(messages, uri, note)
                    {
                        *messages = rewritten;
                    }
                }
            }
        }
    }
}

/// Confirm the associations of the projects whose write is settled. A project an attempt left in doubt keeps its
/// pending writer even though a later attempt committed: the earlier insert may still land, and a confirmed
/// association can be released under it. Held, its file outlives no reference; the repair and deletion sweeps
/// settle it.
async fn confirm_settled(
    file_service: &FileService,
    created: &[(String, String, String)],
    in_doubt: &HashSet<String>,
) {
    let settled: Vec<(String, String, String)> = created
        .iter()
        .filter(|(project, _, _)| !in_doubt.contains(project))
        .cloned()
        .collect();
    crate::media::confirm_associations(file_service, &settled, "logs").await;
}

/// Rows written for a trace deleted while the export was in flight: the deletion may have released the trace's
/// associations and reclaimed its bytes after the files were stored, so the rows go and so do this export's
/// associations for that trace, as span ingestion compensates (`collect_spans_written_for_deleted_traces`). The
/// rows go first, and the associations only if they went: released under a readable row, a file is lost.
async fn compensate_deleted_traces(
    file_service: &FileService,
    analytics: &(dyn AnalyticsRepository + Send + Sync),
    created: &mut Vec<(String, String, String)>,
    in_doubt: &HashSet<String>,
) {
    // The look and the delete in one pass; without the lease, the deletion sweep collects the rows.
    let fence = file_service.collection_fence().clone();
    let Some(_pass) = fence.enter("log compensation") else {
        return;
    };
    let mut by_project: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    for (project, trace, _) in created.iter() {
        by_project
            .entry(project.clone())
            .or_default()
            .push(trace.clone());
    }
    for (project, mut traces) in by_project {
        traces.sort_unstable();
        traces.dedup();
        let typed = ProjectId::from(project.as_str());
        let tombstoned = match file_service
            .database()
            .deleted_traces_among(&typed, &traces)
            .await
        {
            Ok(found) => found,
            Err(error) => {
                tracing::warn!(%error, project, "Could not re-check trace tombstones after the log write; the deletion sweep collects anything that slipped through");
                continue;
            }
        };
        if tombstoned.is_empty() {
            continue;
        }
        let doomed: Vec<String> = tombstoned.iter().cloned().collect();
        if !fence.is_open() {
            return;
        }
        if let Err(error) = analytics.delete_traces(&typed, &doomed).await {
            tracing::error!(%error, project, "Could not remove logs written for a deleted trace, so their associations are kept; the deletion sweep retries both");
            continue;
        }
        // Where the write is in doubt its rows may still land, so the associations stay pending for the settlement:
        // the deletion sweep removes the trace's rows and then its associations, once nothing is readable.
        if in_doubt.contains(&project) {
            continue;
        }
        let orphaned: Vec<(String, String, String)> = created
            .iter()
            .filter(|(p, t, _)| p == &project && tombstoned.contains(t))
            .cloned()
            .collect();
        // Kept while fenced: confirmed, they hold the files of rows the deletion sweep removes, and it reconciles
        // them then.
        if !fence.is_open() {
            return;
        }
        created.retain(|(p, t, _)| !(p == &project && tombstoned.contains(t)));
        crate::media::release_created_associations(file_service, &orphaned).await;
    }
}

/// Why a log write failed: before the insert, so nothing could have landed, or in it, with the store's error.
enum WriteFailure {
    BeforeWrite(String),
    /// The insert failed on every attempt: the last error, and every project any attempt left in doubt.
    Write {
        error: sideseat_ports::error::DataError,
        in_doubt: HashSet<String>,
    },
    /// The rows are stored, and what failed after them is accounting: the export is retried, but its rows name
    /// their files, so the associations are confirmed. With the projects an earlier attempt left in doubt.
    AfterWrite {
        error: String,
        in_doubt: HashSet<String>,
    },
}

impl std::fmt::Display for WriteFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BeforeWrite(error) | Self::AfterWrite { error, .. } => formatter.write_str(error),
            Self::Write { error, .. } => write!(formatter, "{error}"),
        }
    }
}

/// Insert the rows, retrying what did not commit: a project an attempt commits is not written again, and a project
/// any attempt left in doubt stays in doubt, also once a later attempt commits it - the earlier one may still land,
/// after a compensation that read none of its rows. Answers the projects in doubt, on success as on failure.
async fn insert_with_retries(
    logs: &[sideseat_ports::types::NormalizedLog],
    analytics: &(dyn AnalyticsRepository + Send + Sync),
) -> Result<HashSet<String>, WriteFailure> {
    let project_of = |log: &sideseat_ports::types::NormalizedLog| {
        log.project_id
            .clone()
            .unwrap_or_else(|| DEFAULT_PROJECT_ID.to_string())
    };
    let mut committed: HashSet<String> = HashSet::new();
    let mut in_doubt: HashSet<String> = HashSet::new();
    let mut remaining: Vec<sideseat_ports::types::NormalizedLog> = logs.to_vec();
    let mut attempt = 1u32;
    loop {
        let error = match analytics.insert_logs(&remaining).await {
            Ok(()) => return Ok(in_doubt),
            Err(error) => error,
        };
        if let sideseat_ports::error::DataError::PartiallyWritten {
            committed_projects, ..
        } = &error
        {
            committed.extend(committed_projects.iter().cloned());
        }
        let unwritten: Vec<String> = remaining
            .iter()
            .map(project_of)
            .filter(|project| !committed.contains(project))
            .collect();
        in_doubt.extend(error.projects_in_doubt(unwritten.iter().map(String::as_str)));
        remaining.retain(|log| !committed.contains(&project_of(log)));
        if remaining.is_empty() {
            return Ok(in_doubt);
        }
        if attempt >= DEFAULT_MAX_ATTEMPTS {
            return Err(WriteFailure::Write { error, in_doubt });
        }
        tokio::time::sleep(std::time::Duration::from_millis(
            DEFAULT_BASE_DELAY_MS * 2_u64.pow(attempt - 1),
        ))
        .await;
        attempt += 1;
    }
}

async fn write_logs(
    logs: &mut [sideseat_ports::types::NormalizedLog],
    analytics: &(dyn AnalyticsRepository + Send + Sync),
    governance: Option<&StorageGovernanceService>,
) -> Result<HashSet<String>, WriteFailure> {
    sideseat_domain::search::index_logs(logs);
    for log in logs.iter_mut() {
        settle_views(log);
    }
    if let Some(governance) = governance {
        governance
            .stamp_logs(logs)
            .await
            .map_err(|error| WriteFailure::BeforeWrite(error.to_string()))?;
    }
    let written_projects = logs
        .iter()
        .map(|log| ProjectId::from(log.project_id.as_deref().unwrap_or(DEFAULT_PROJECT_ID)))
        .collect::<Vec<_>>();
    let in_doubt = insert_with_retries(logs, analytics).await?;
    if let Some(governance) = governance {
        governance
            .patch_after_write(&written_projects)
            .await
            .map_err(|error| WriteFailure::AfterWrite {
                error: error.to_string(),
                in_doubt: in_doubt.clone(),
            })?;
    }
    Ok(in_doubt)
}

#[cfg(test)]
#[path = "ingest_tests.rs"]
mod tests;
