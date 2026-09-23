//! Content-addressed storage for the large, interpretation-bearing fields of a span.
//!
//! The analytics columns remain populated during the migration. They are the dual-read fallback; a durable
//! body association is preferred when present, and any registry/blob failure falls back to the inline value.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use futures::{StreamExt, stream};
use serde::Serialize;
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::files::FileService;
use crate::storage_governance::StorageGovernanceService;
use sideseat_ports::blobs::{FileStorage, FileStorageError};
use sideseat_ports::error::DataError;
use sideseat_ports::traits::{AnalyticsRepository, SurvivorReferences, TransactionalRepository};
use sideseat_ports::types::{
    ContentBodyBackfillProgress, ContentBodyObject, MessageSpanRow, NormalizedSpan, ProjectId,
    SearchSignal, SpanBodyAssociation, SpanBodyField, SpanBodySource, SpanRow,
};
use tokio::sync::watch;
use tokio::task::JoinHandle;

const BODY_HASH_DOMAIN: &[u8] = b"sideseat-content-body-v1\0";
const BODY_IO_CONCURRENCY: usize = 16;
const BACKFILL_PAGE_SIZE: usize = 256;
const SEARCH_BACKFILL_PAGE_SIZE: usize = 16;
const BACKFILL_PROJECT_PAGE_SIZE: u32 = 100;
const BACKFILL_INTERVAL_SECS: u64 = 30;
const ORPHAN_SWEEP_LIMIT: usize = 256;
const DELETION_CLAIM_STALE_SECS: i64 = 300;

type SpanIdentity = (String, String, String);
type CollectedBodies = (
    BTreeMap<(String, String), BodyBytes>,
    Vec<SpanBodyAssociation>,
    HashMap<SpanIdentity, String>,
);

#[derive(Debug, Error)]
pub enum ContentBodyError {
    #[error(transparent)]
    Database(#[from] DataError),
    #[error(transparent)]
    Storage(#[from] FileStorageError),
    #[error("content-body quota admission failed: {0}")]
    Governance(String),
    #[error("only {staged} of {expected} content-body associations were staged")]
    StageShortfall { expected: usize, staged: u64 },
    #[error("only {confirmed} of {expected} content-body associations were confirmed")]
    ConfirmShortfall { expected: usize, confirmed: u64 },
    #[error(
        "content-body deletion claim could not be finalized for project {project_id}, hash {body_hash}"
    )]
    DeletionFinalization {
        project_id: ProjectId,
        body_hash: String,
    },
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct ContentBodyRestoreCleanupReport {
    pub stale_claims_finalized: u64,
    pub orphans_deleted: u64,
}

#[derive(Clone)]
struct BodyBytes {
    object: ContentBodyObject,
    bytes: Arc<[u8]>,
}

struct FetchedBody {
    hash: String,
    body: Option<String>,
}

/// Provisional ownership created before the analytics write.
pub struct StagedBodies {
    associations: Vec<SpanBodyAssociation>,
    content_digests: HashMap<SpanIdentity, String>,
}

impl StagedBodies {
    pub fn is_empty(&self) -> bool {
        self.associations.is_empty()
    }

    fn retain_identities(&mut self, keep: &HashSet<SpanIdentity>) -> Vec<SpanBodyAssociation> {
        let mut removed = Vec::new();
        self.associations.retain(|association| {
            let identity = (
                association.project_id.to_string(),
                association.trace_id.clone(),
                association.span_id.clone(),
            );
            if keep.contains(&identity) {
                true
            } else {
                removed.push(association.clone());
                false
            }
        });
        self.content_digests
            .retain(|identity, _| keep.contains(identity));
        removed
    }
}

/// Coordinates physical blobs with the body-level transactional ownership registry.
#[derive(Clone)]
pub struct ContentBodyService {
    storage: Arc<dyn FileStorage>,
    database: Arc<dyn TransactionalRepository + Send + Sync>,
}

impl ContentBodyService {
    pub fn from_file_service(files: &FileService) -> Self {
        Self {
            storage: Arc::clone(files.storage()),
            database: Arc::clone(files.database()),
        }
    }

    /// Hash body bytes in a namespace distinct from uploaded-file hashes.
    ///
    /// Both object classes may share one physical backend, but a file is keyed by SHA-256(raw bytes) while
    /// a body is keyed by SHA-256(domain || raw bytes), so their lifecycle registries cannot address the
    /// same object except through a cryptographic collision.
    pub fn hash(bytes: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(BODY_HASH_DOMAIN);
        hasher.update(bytes);
        hex::encode(hasher.finalize())
    }

    /// Register unique objects, reserve their transitional dual-write footprint, store the bytes, then
    /// create provisional field ownership. No analytics row is written by this method.
    pub async fn stage(
        &self,
        spans: &[NormalizedSpan],
        governance: Option<&Arc<StorageGovernanceService>>,
    ) -> Result<StagedBodies, ContentBodyError> {
        let (mut objects, associations, mut content_digests) = collect(spans);
        if associations.is_empty() {
            return Ok(StagedBodies {
                associations,
                content_digests,
            });
        }
        let associations = self.database.unresolved_span_bodies(&associations).await?;
        if associations.is_empty() {
            return Ok(StagedBodies {
                associations,
                content_digests: HashMap::new(),
            });
        }
        let needed_objects = associations
            .iter()
            .map(|association| {
                (
                    association.project_id.to_string(),
                    association.body_hash.clone(),
                )
            })
            .collect::<HashSet<_>>();
        objects.retain(|key, _| needed_objects.contains(key));
        let needed_identities = associations
            .iter()
            .map(|association| {
                (
                    association.project_id.to_string(),
                    association.trace_id.clone(),
                    association.span_id.clone(),
                )
            })
            .collect::<HashSet<_>>();
        content_digests.retain(|identity, _| needed_identities.contains(identity));

        let object_rows = objects
            .values()
            .map(|body| body.object.clone())
            .collect::<Vec<_>>();
        let newly_registered = self.database.register_content_bodies(&object_rows).await?;

        if let Some(governance) = governance {
            let mut bytes_by_project: HashMap<&ProjectId, u64> = HashMap::new();
            for object in &newly_registered {
                *bytes_by_project.entry(&object.project_id).or_default() = bytes_by_project
                    .get(&object.project_id)
                    .copied()
                    .unwrap_or(0)
                    .saturating_add(object.logical_bytes);
            }
            for (project_id, bytes) in bytes_by_project {
                if let Err(error) = governance.admit(project_id, bytes).await {
                    self.discard_new_objects(&newly_registered).await;
                    self.reconcile_projects(governance, &newly_registered).await;
                    return Err(ContentBodyError::Governance(error.to_string()));
                }
            }
        }

        let failures = stream::iter(objects.values().cloned().map(|body| {
            let storage = Arc::clone(&self.storage);
            async move {
                storage
                    .store(&body.object.project_id, &body.object.body_hash, &body.bytes)
                    .await
                    .map_err(|error| (body.object, error))
            }
        }))
        .buffer_unordered(BODY_IO_CONCURRENCY)
        .filter_map(|result| async move { result.err() })
        .collect::<Vec<_>>()
        .await;
        if let Some((object, error)) = failures.into_iter().next() {
            tracing::error!(
                project_id = %object.project_id,
                body_hash = %object.body_hash,
                %error,
                "Could not store a content-addressed span body"
            );
            self.discard_new_objects(&newly_registered).await;
            if let Some(governance) = governance {
                self.reconcile_projects(governance, &newly_registered).await;
            }
            return Err(ContentBodyError::Storage(error));
        }

        let staged = match self.database.stage_span_bodies(&associations).await {
            Ok(staged) => staged,
            Err(error) => {
                self.discard_new_objects(&newly_registered).await;
                if let Some(governance) = governance {
                    self.reconcile_projects(governance, &newly_registered).await;
                }
                return Err(ContentBodyError::Database(error));
            }
        };
        if staged < associations.len() as u64 {
            self.release_associations(&associations).await;
            self.discard_new_objects(&newly_registered).await;
            if let Some(governance) = governance {
                self.reconcile_projects(governance, &newly_registered).await;
            }
            return Err(ContentBodyError::StageShortfall {
                expected: associations.len(),
                staged,
            });
        }

        Ok(StagedBodies {
            associations,
            content_digests,
        })
    }

    pub async fn mark_incomplete_for_spans(&self, spans: &[NormalizedSpan]) {
        let projects = spans
            .iter()
            .map(|span| {
                ProjectId::from(
                    span.project_id
                        .as_deref()
                        .unwrap_or(sideseat_core::constants::DEFAULT_PROJECT_ID),
                )
            })
            .collect::<HashSet<_>>();
        for project_id in projects {
            if let Err(error) = self.database.reset_content_body_backfill(&project_id).await {
                tracing::warn!(
                    %error,
                    %project_id,
                    "Could not reset body-backfill progress after a dual-write failure"
                );
            }
        }
    }

    /// Release bodies for spans removed by a pre-write fence and forget them from the later confirmation.
    pub async fn retain_spans(&self, staged: &mut StagedBodies, spans: &[NormalizedSpan]) {
        let keep = spans
            .iter()
            .map(|span| {
                (
                    span.project_id
                        .as_deref()
                        .unwrap_or(sideseat_core::constants::DEFAULT_PROJECT_ID)
                        .to_string(),
                    span.trace_id.clone(),
                    span.span_id.clone(),
                )
            })
            .collect::<HashSet<_>>();
        let removed = staged.retain_identities(&keep);
        self.release_associations(&removed).await;
    }

    /// Release exact identities removed by the post-write deletion compensation.
    pub async fn remove_identities(
        &self,
        staged: &mut StagedBodies,
        removed: &HashSet<SpanIdentity>,
    ) {
        if removed.is_empty() {
            return;
        }
        let keep = staged
            .content_digests
            .keys()
            .filter(|identity| !removed.contains(*identity))
            .cloned()
            .collect::<HashSet<_>>();
        let released = staged.retain_identities(&keep);
        self.release_associations(&released).await;
    }

    /// Confirm only associations whose delivery is the analytics store's current winner.
    ///
    /// The common path is one strict digest query per project. If a mixed batch contains a losing
    /// correction, it falls back to one query per identity so the other winners still gain durable bodies.
    pub async fn confirm_winners(
        &self,
        staged: &mut StagedBodies,
        analytics: &dyn AnalyticsRepository,
    ) {
        let mut by_project: HashMap<String, Vec<(String, String, String)>> = HashMap::new();
        for ((project, trace, span), digest) in &staged.content_digests {
            by_project.entry(project.clone()).or_default().push((
                trace.clone(),
                span.clone(),
                digest.clone(),
            ));
        }

        let mut winners = HashSet::new();
        for (project, records) in by_project {
            let project_id = ProjectId::from(project.as_str());
            match analytics.spans_match_content(&project_id, &records).await {
                Ok(true) => {
                    winners.extend(
                        records
                            .into_iter()
                            .map(|(trace, span, _)| (project.clone(), trace, span)),
                    );
                }
                Ok(false) => {
                    for (trace, span, digest) in records {
                        match analytics
                            .spans_match_content(
                                &project_id,
                                &[(trace.clone(), span.clone(), digest)],
                            )
                            .await
                        {
                            Ok(true) => {
                                winners.insert((project.clone(), trace, span));
                            }
                            Ok(false) => {}
                            Err(error) => tracing::warn!(
                                %error,
                                project_id = %project,
                                trace_id = %trace,
                                span_id = %span,
                                "Could not determine the winning delivery for a span body"
                            ),
                        }
                    }
                }
                Err(error) => tracing::warn!(
                    %error,
                    project_id = %project,
                    "Could not determine the winning deliveries for span bodies"
                ),
            }
        }

        let losers = staged.retain_identities(&winners);
        self.release_associations(&losers).await;
        if staged.associations.is_empty() {
            return;
        }
        match self
            .database
            .confirm_span_bodies(&staged.associations)
            .await
        {
            Ok(confirmed) if confirmed < staged.associations.len() as u64 => {
                let associations = std::mem::take(&mut staged.associations);
                staged.content_digests.clear();
                self.release_associations(&associations).await;
                self.reset_backfill_for_associations(&associations).await;
                tracing::error!(
                    expected = associations.len(),
                    confirmed,
                    "Fewer span-body associations were confirmed than the winning rows own"
                );
            }
            Ok(_) => {}
            Err(error) => {
                let associations = std::mem::take(&mut staged.associations);
                staged.content_digests.clear();
                self.release_associations(&associations).await;
                self.reset_backfill_for_associations(&associations).await;
                tracing::warn!(
                    %error,
                    "Could not mark winning span-body associations durable; inline columns remain readable"
                );
            }
        }
    }

    pub async fn release_all(&self, staged: &mut StagedBodies) {
        let associations = std::mem::take(&mut staged.associations);
        staged.content_digests.clear();
        self.release_associations(&associations).await;
    }

    /// Prefer durable body objects while preserving the old analytics columns as an availability fallback.
    ///
    /// Fetches are a bounded stream, so a 10 000-turn reconstruction does not create one task or one
    /// simultaneous object-store request per field.
    pub async fn hydrate_message_rows(&self, project_id: &ProjectId, rows: &mut [MessageSpanRow]) {
        let requests = rows
            .iter()
            .enumerate()
            .flat_map(|(index, row)| {
                [
                    (
                        index,
                        row.trace_id.clone(),
                        row.span_id.clone(),
                        SpanBodyField::Messages,
                    ),
                    (
                        index,
                        row.trace_id.clone(),
                        row.span_id.clone(),
                        SpanBodyField::ToolDefinitions,
                    ),
                    (
                        index,
                        row.trace_id.clone(),
                        row.span_id.clone(),
                        SpanBodyField::ToolNames,
                    ),
                ]
            })
            .collect::<Vec<_>>();
        let fetched = self.fetch_bodies(project_id, requests).await;
        for (index, row) in rows.iter_mut().enumerate() {
            let mut cache_hasher = blake3::Hasher::new();
            cache_hasher.update(b"sideseat-message-bodies-v1\0");
            for (field, inline) in [
                (SpanBodyField::Messages, &mut row.messages_json),
                (
                    SpanBodyField::ToolDefinitions,
                    &mut row.tool_definitions_json,
                ),
                (SpanBodyField::ToolNames, &mut row.tool_names_json),
            ] {
                cache_hasher.update(field.as_str().as_bytes());
                if let Some(fetched) = fetched.get(&(index, field)) {
                    if let Some(body) = fetched.body.as_ref() {
                        cache_hasher.update(&[1]);
                        cache_hasher.update(&(fetched.hash.len() as u64).to_le_bytes());
                        cache_hasher.update(fetched.hash.as_bytes());
                        *inline = body.clone();
                    } else {
                        cache_hasher.update(&[0]);
                        cache_hasher.update(&(inline.len() as u64).to_le_bytes());
                        cache_hasher.update(inline.as_bytes());
                    }
                } else {
                    cache_hasher.update(&[0]);
                    cache_hasher.update(&(inline.len() as u64).to_le_bytes());
                    cache_hasher.update(inline.as_bytes());
                }
            }
            row.body_cache_key = Some(cache_hasher.finalize().to_hex().to_string());
        }
    }

    pub async fn hydrate_raw_spans(&self, project_id: &ProjectId, rows: &mut [SpanRow]) {
        let requests = rows
            .iter()
            .enumerate()
            .map(|(index, row)| {
                (
                    index,
                    row.trace_id.clone(),
                    row.span_id.clone(),
                    SpanBodyField::RawSpan,
                )
            })
            .collect();
        for ((index, _), fetched) in self.fetch_bodies(project_id, requests).await {
            if let Some(body) = fetched.body {
                rows[index].raw_span = Some(body);
            }
        }
    }

    pub async fn cleanup_spans(
        &self,
        project_id: &ProjectId,
        spans: &[(String, String)],
    ) -> Result<(), ContentBodyError> {
        let hashes = self.database.delete_span_bodies(project_id, spans).await?;
        self.delete_orphans(project_id, &hashes).await;
        Ok(())
    }

    pub async fn cleanup_traces(
        &self,
        project_id: &ProjectId,
        trace_ids: &[String],
    ) -> Result<(), ContentBodyError> {
        let hashes = self
            .database
            .delete_trace_bodies(project_id, trace_ids)
            .await?;
        self.delete_orphans(project_id, &hashes).await;
        Ok(())
    }

    /// Backfill and reconcile exact ownership for the winning spans of selected traces.
    ///
    /// Two scans close the same commit-after-scan window as file survivor reconciliation: a writer that
    /// confirms between the first scan and release appears in the second and is restored before orphan GC.
    pub async fn reconcile_trace_survivors(
        &self,
        project_id: &ProjectId,
        trace_ids: &[String],
        analytics: &dyn sideseat_ports::traits::SurvivorReferences,
    ) -> Result<(), ContentBodyError> {
        if trace_ids.is_empty() {
            return Ok(());
        }
        for _ in 0..2 {
            let sources = analytics
                .span_body_fields_for_traces(project_id, trace_ids)
                .await?;
            let (objects, associations) = collect_sources(project_id, &sources);
            let object_rows = objects
                .values()
                .map(|body| body.object.clone())
                .collect::<Vec<_>>();
            self.database.register_content_bodies(&object_rows).await?;
            let failures = stream::iter(objects.values().cloned().map(|body| {
                let storage = Arc::clone(&self.storage);
                async move {
                    storage
                        .store(&body.object.project_id, &body.object.body_hash, &body.bytes)
                        .await
                }
            }))
            .buffer_unordered(BODY_IO_CONCURRENCY)
            .filter_map(|result| async move { result.err() })
            .collect::<Vec<_>>()
            .await;
            if let Some(error) = failures.into_iter().next() {
                return Err(ContentBodyError::Storage(error));
            }
            let removed = self
                .database
                .reconcile_span_bodies(project_id, trace_ids, &associations)
                .await?;
            self.delete_orphans(project_id, &removed).await;
        }
        Ok(())
    }

    /// Process one resumable, identity-ordered backfill page for a project.
    pub async fn backfill_project_page(
        &self,
        project_id: &ProjectId,
        analytics: &dyn SurvivorReferences,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<ContentBodyBackfillProgress, ContentBodyError> {
        let previous = self
            .database
            .content_body_backfill_progress(project_id)
            .await?;
        if let Some(progress) = previous.as_ref()
            && progress.complete
        {
            return Ok(progress.clone());
        }
        let after = previous.as_ref().and_then(|progress| {
            progress
                .cursor_trace_id
                .clone()
                .zip(progress.cursor_span_id.clone())
        });
        let sources = analytics
            .span_body_backfill_page(project_id, after, BACKFILL_PAGE_SIZE)
            .await?;
        self.backfill_sources(project_id, &sources).await?;
        let complete = sources.len() < BACKFILL_PAGE_SIZE;
        let cursor = sources
            .last()
            .map(|source| (source.trace_id.clone(), source.span_id.clone()))
            .or_else(|| {
                previous.as_ref().and_then(|progress| {
                    progress
                        .cursor_trace_id
                        .clone()
                        .zip(progress.cursor_span_id.clone())
                })
            });
        let progress = ContentBodyBackfillProgress {
            project_id: project_id.clone(),
            cursor_trace_id: cursor.as_ref().map(|(trace, _)| trace.clone()),
            cursor_span_id: cursor.map(|(_, span)| span),
            complete,
            updated_at: now,
        };
        self.database
            .save_content_body_backfill_progress(&progress)
            .await?;
        if complete {
            tracing::info!(%project_id, "Content-body backfill reached the project cutover criterion");
        } else {
            tracing::warn!(
                %project_id,
                cursor_trace_id = ?progress.cursor_trace_id,
                cursor_span_id = ?progress.cursor_span_id,
                "Content-body backfill drift remains"
            );
        }
        Ok(progress)
    }

    /// Rate-limited, resumable background backfill. One page per live project per interval.
    pub fn start_backfill_task(
        self: Arc<Self>,
        analytics: Arc<dyn AnalyticsRepository + Send + Sync>,
        clock: Arc<dyn sideseat_ports::clock::Clock>,
        mut shutdown_rx: watch::Receiver<bool>,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut interval =
                tokio::time::interval(std::time::Duration::from_secs(BACKFILL_INTERVAL_SECS));
            loop {
                tokio::select! {
                    biased;
                    changed = shutdown_rx.changed() => {
                        if changed.is_err() || *shutdown_rx.borrow() {
                            break;
                        }
                    }
                    _ = interval.tick() => {
                        self.sweep_orphans(clock.now()).await;
                        let mut page = 1;
                        loop {
                            let projects = match self
                                .database
                                .list_projects(page, BACKFILL_PROJECT_PAGE_SIZE)
                                .await
                            {
                                Ok((projects, _)) => projects,
                                Err(error) => {
                                    tracing::warn!(%error, "Could not list projects for content-body backfill");
                                    break;
                                }
                            };
                            if projects.is_empty() {
                                break;
                            }
                            for project in &projects {
                                let project_id = ProjectId::from(project.id.as_str());
                                if let Err(error) = self
                                    .backfill_project_page(&project_id, analytics.as_ref(), clock.now())
                                    .await
                                {
                                    tracing::warn!(
                                        %error,
                                        %project_id,
                                        "Content-body backfill page failed; cursor was not advanced"
                                    );
                                }
                                for signal in [SearchSignal::Spans, SearchSignal::Logs] {
                                    match crate::search::SearchService::backfill_project_page(
                                        analytics.as_ref(),
                                        &project_id,
                                        signal,
                                        SEARCH_BACKFILL_PAGE_SIZE,
                                    )
                                    .await
                                    {
                                        Ok(count) if count == SEARCH_BACKFILL_PAGE_SIZE => {
                                            tracing::warn!(
                                                %project_id,
                                                ?signal,
                                                count,
                                                "Search-index backfill drift remains"
                                            );
                                        }
                                        Ok(count) if count > 0 => {
                                            tracing::info!(
                                                %project_id,
                                                ?signal,
                                                count,
                                                "Search-index backfill reached the current project tail"
                                            );
                                        }
                                        Ok(_) => {}
                                        Err(error) => {
                                            tracing::warn!(
                                                %error,
                                                %project_id,
                                                ?signal,
                                                "Search-index backfill page failed; complete markers were not advanced"
                                            );
                                        }
                                    }
                                }
                            }
                            if projects.len() < BACKFILL_PROJECT_PAGE_SIZE as usize {
                                break;
                            }
                            page += 1;
                        }
                    }
                }
            }
        })
    }

    async fn backfill_sources(
        &self,
        project_id: &ProjectId,
        sources: &[SpanBodySource],
    ) -> Result<(), ContentBodyError> {
        let (objects, associations) = collect_sources(project_id, sources);
        if associations.is_empty() {
            return Ok(());
        }
        let object_rows = objects
            .values()
            .map(|body| body.object.clone())
            .collect::<Vec<_>>();
        self.database.register_content_bodies(&object_rows).await?;
        let failures = stream::iter(objects.values().cloned().map(|body| {
            let storage = Arc::clone(&self.storage);
            async move {
                storage
                    .store(&body.object.project_id, &body.object.body_hash, &body.bytes)
                    .await
            }
        }))
        .buffer_unordered(BODY_IO_CONCURRENCY)
        .filter_map(|result| async move { result.err() })
        .collect::<Vec<_>>()
        .await;
        if let Some(error) = failures.into_iter().next() {
            return Err(ContentBodyError::Storage(error));
        }
        let staged = self.database.stage_span_bodies(&associations).await?;
        if staged < associations.len() as u64 {
            self.release_associations(&associations).await;
            return Err(ContentBodyError::StageShortfall {
                expected: associations.len(),
                staged,
            });
        }
        let confirmed = self.database.confirm_span_bodies(&associations).await?;
        if confirmed < associations.len() as u64 {
            self.release_associations(&associations).await;
            return Err(ContentBodyError::ConfirmShortfall {
                expected: associations.len(),
                confirmed,
            });
        }
        Ok(())
    }

    async fn fetch_bodies(
        &self,
        project_id: &ProjectId,
        requests: Vec<(usize, String, String, SpanBodyField)>,
    ) -> HashMap<(usize, SpanBodyField), FetchedBody> {
        let associations = stream::iter(requests.into_iter().map(|(index, trace, span, field)| {
            let database = Arc::clone(&self.database);
            let project_id = project_id.clone();
            async move {
                match database
                    .get_span_body_hash(&project_id, &trace, &span, field)
                    .await
                {
                    Ok(Some(hash)) => Some(((index, field), hash)),
                    Ok(None) => None,
                    Err(error) => {
                        tracing::warn!(
                            %error,
                            %project_id,
                            trace_id = %trace,
                            span_id = %span,
                            field = field.as_str(),
                            "Content-body registry read failed; using the inline analytics column"
                        );
                        None
                    }
                }
            }
        }))
        .buffer_unordered(BODY_IO_CONCURRENCY)
        .filter_map(|value| async move { value })
        .collect::<Vec<_>>()
        .await;

        let hashes = associations
            .iter()
            .map(|(_, hash)| hash.clone())
            .collect::<HashSet<_>>();
        let bodies = stream::iter(hashes.into_iter().map(|hash| {
            let storage = Arc::clone(&self.storage);
            let project_id = project_id.clone();
            async move {
                let body = match storage.get(&project_id, &hash).await {
                    Ok(bytes) => match String::from_utf8(bytes) {
                        Ok(body) => Some(body),
                        Err(error) => {
                            tracing::warn!(
                                %error,
                                %project_id,
                                body_hash = %hash,
                                "Content body is not UTF-8; using the inline analytics column"
                            );
                            None
                        }
                    },
                    Err(error) => {
                        tracing::warn!(
                            %error,
                            %project_id,
                            body_hash = %hash,
                            "Content body is unavailable; using the inline analytics column"
                        );
                        None
                    }
                };
                (hash, body)
            }
        }))
        .buffer_unordered(BODY_IO_CONCURRENCY)
        .collect::<HashMap<_, _>>()
        .await;

        associations
            .into_iter()
            .map(|(key, hash)| {
                let body = bodies.get(&hash).cloned().flatten();
                (key, FetchedBody { hash, body })
            })
            .collect()
    }

    async fn release_associations(&self, associations: &[SpanBodyAssociation]) {
        for association in associations {
            match self.database.release_span_body(association).await {
                // Winner races can make the same historical body alternate between live and orphaned many
                // times during concurrent replay. Immediate physical deletion turns that into delete/write
                // thrashing. The orphan sweeper applies a grace period; explicit retention/deletion paths call
                // `delete_orphans` directly and remain immediate.
                Ok(_) => {}
                Err(error) => tracing::warn!(
                    %error,
                    project_id = %association.project_id,
                    trace_id = %association.trace_id,
                    span_id = %association.span_id,
                    field = association.field.as_str(),
                    "Could not release a provisional span-body association"
                ),
            }
        }
    }

    async fn discard_new_objects(&self, objects: &[ContentBodyObject]) {
        for object in objects {
            self.delete_one_orphan(&object.project_id, &object.body_hash)
                .await;
        }
    }

    async fn delete_orphans(&self, project_id: &ProjectId, hashes: &[String]) {
        for hash in hashes {
            self.delete_one_orphan(project_id, hash).await;
        }
    }

    async fn delete_one_orphan(&self, project_id: &ProjectId, hash: &str) {
        if let Err(error) = self.try_delete_one_orphan(project_id, hash).await {
            tracing::warn!(
                %error,
                %project_id,
                body_hash = %hash,
                "Could not delete an orphaned content body"
            );
        }
    }

    async fn delete_claimed_orphan(&self, project_id: &ProjectId, hash: &str) {
        if let Err(error) = self.try_delete_claimed_orphan(project_id, hash).await {
            tracing::warn!(
                %error,
                %project_id,
                body_hash = %hash,
                "Could not finish an orphaned content-body deletion claim"
            );
        }
    }

    async fn try_delete_one_orphan(
        &self,
        project_id: &ProjectId,
        hash: &str,
    ) -> Result<bool, ContentBodyError> {
        if !self
            .database
            .claim_content_body_for_deletion(project_id, hash)
            .await?
        {
            return Ok(false);
        }
        self.try_delete_claimed_orphan(project_id, hash).await?;
        Ok(true)
    }

    async fn try_delete_claimed_orphan(
        &self,
        project_id: &ProjectId,
        hash: &str,
    ) -> Result<(), ContentBodyError> {
        if let Err(error) = self.storage.delete(project_id, hash).await {
            self.database
                .release_content_body_deletion_claim(project_id, hash)
                .await?;
            return Err(ContentBodyError::Storage(error));
        }
        if self
            .database
            .delete_claimed_content_body(project_id, hash)
            .await?
        {
            return Ok(());
        }
        self.database
            .release_content_body_deletion_claim(project_id, hash)
            .await?;
        Err(ContentBodyError::DeletionFinalization {
            project_id: project_id.clone(),
            body_hash: hash.to_owned(),
        })
    }

    /// Drain every body orphan and abandoned deletion claim while the restore marker excludes writers.
    ///
    /// Unlike the live sweeper this has no grace period and no one-page bound: independently restored
    /// metadata can be arbitrarily older or newer than analytics, so repair must reach a fixed point before
    /// reads resume.
    pub async fn cleanup_orphans_after_restore(
        &self,
    ) -> Result<ContentBodyRestoreCleanupReport, ContentBodyError> {
        let mut report = ContentBodyRestoreCleanupReport::default();
        let all_timestamps = chrono::DateTime::<chrono::Utc>::from_timestamp_nanos(i64::MAX);

        loop {
            let claims = self
                .database
                .get_stale_claimed_content_bodies(all_timestamps, ORPHAN_SWEEP_LIMIT)
                .await?;
            if claims.is_empty() {
                break;
            }
            for (project_id, hash) in claims {
                self.try_delete_claimed_orphan(&project_id, &hash).await?;
                report.stale_claims_finalized += 1;
            }
        }

        loop {
            let orphans = self
                .database
                .get_orphan_content_bodies(all_timestamps, ORPHAN_SWEEP_LIMIT)
                .await?;
            if orphans.is_empty() {
                break;
            }
            for (project_id, hash) in orphans {
                if self.try_delete_one_orphan(&project_id, &hash).await? {
                    report.orphans_deleted += 1;
                }
            }
        }

        Ok(report)
    }

    async fn sweep_orphans(&self, now: chrono::DateTime<chrono::Utc>) {
        let stale_before = now - chrono::Duration::seconds(DELETION_CLAIM_STALE_SECS);
        match self
            .database
            .get_stale_claimed_content_bodies(stale_before, ORPHAN_SWEEP_LIMIT)
            .await
        {
            Ok(claims) => {
                for (project_id, hash) in claims {
                    self.delete_claimed_orphan(&project_id, &hash).await;
                }
            }
            Err(error) => tracing::warn!(
                %error,
                "Could not list stale content-body deletion claims"
            ),
        }

        match self
            .database
            .get_orphan_content_bodies(now - chrono::Duration::minutes(5), ORPHAN_SWEEP_LIMIT)
            .await
        {
            Ok(orphans) => {
                for (project_id, hash) in orphans {
                    self.delete_one_orphan(&project_id, &hash).await;
                }
            }
            Err(error) => tracing::warn!(%error, "Could not list orphaned content bodies"),
        }
    }

    async fn reset_backfill_for_associations(&self, associations: &[SpanBodyAssociation]) {
        let projects = associations
            .iter()
            .map(|association| association.project_id.clone())
            .collect::<HashSet<_>>();
        for project_id in projects {
            if let Err(error) = self.database.reset_content_body_backfill(&project_id).await {
                tracing::warn!(
                    %error,
                    %project_id,
                    "Could not reset body-backfill progress after confirmation failure"
                );
            }
        }
    }

    async fn reconcile_projects(
        &self,
        governance: &StorageGovernanceService,
        objects: &[ContentBodyObject],
    ) {
        let projects = objects
            .iter()
            .map(|object| object.project_id.clone())
            .collect::<HashSet<_>>();
        for project_id in projects {
            if let Err(error) = governance.reconcile_project(&project_id).await {
                tracing::warn!(
                    %error,
                    %project_id,
                    "Could not reconcile quota after rolling back body registration"
                );
            }
        }
    }
}

fn collect(spans: &[NormalizedSpan]) -> CollectedBodies {
    let mut objects = BTreeMap::new();
    let mut associations = Vec::new();
    let mut seen = HashSet::new();
    let mut content_digests = HashMap::new();

    for span in spans {
        let project = span
            .project_id
            .as_deref()
            .unwrap_or(sideseat_core::constants::DEFAULT_PROJECT_ID);
        let project_id = ProjectId::from(project);
        let identity = (
            project.to_string(),
            span.trace_id.clone(),
            span.span_id.clone(),
        );
        content_digests.insert(identity, span.content_digest.clone());
        for (field, body) in [
            (SpanBodyField::Messages, span.messages.as_deref()),
            (
                SpanBodyField::ToolDefinitions,
                span.tool_definitions.as_deref(),
            ),
            (SpanBodyField::ToolNames, span.tool_names.as_deref()),
            (SpanBodyField::RawSpan, span.raw_span.as_deref()),
        ] {
            let Some(body) = body else {
                continue;
            };
            let hash = ContentBodyService::hash(body.as_bytes());
            let key = (project.to_string(), hash.clone());
            objects.entry(key).or_insert_with(|| BodyBytes {
                object: ContentBodyObject {
                    project_id: project_id.clone(),
                    body_hash: hash.clone(),
                    logical_bytes: body.len() as u64,
                },
                bytes: Arc::from(body.as_bytes()),
            });
            let association_key = (
                project.to_string(),
                span.trace_id.clone(),
                span.span_id.clone(),
                field,
                hash.clone(),
            );
            if seen.insert(association_key) {
                associations.push(SpanBodyAssociation {
                    project_id: project_id.clone(),
                    trace_id: span.trace_id.clone(),
                    span_id: span.span_id.clone(),
                    field,
                    body_hash: hash,
                    logical_bytes: body.len() as u64,
                });
            }
        }
    }
    (objects, associations, content_digests)
}

fn collect_sources(
    project_id: &ProjectId,
    sources: &[SpanBodySource],
) -> (
    BTreeMap<(String, String), BodyBytes>,
    Vec<SpanBodyAssociation>,
) {
    let mut objects = BTreeMap::new();
    let mut associations = Vec::new();
    let mut seen = HashSet::new();
    for source in sources {
        for (field, body) in [
            (SpanBodyField::Messages, source.messages.as_deref()),
            (
                SpanBodyField::ToolDefinitions,
                source.tool_definitions.as_deref(),
            ),
            (SpanBodyField::ToolNames, source.tool_names.as_deref()),
            (SpanBodyField::RawSpan, source.raw_span.as_deref()),
        ] {
            let Some(body) = body else {
                continue;
            };
            let hash = ContentBodyService::hash(body.as_bytes());
            objects
                .entry((project_id.to_string(), hash.clone()))
                .or_insert_with(|| BodyBytes {
                    object: ContentBodyObject {
                        project_id: project_id.clone(),
                        body_hash: hash.clone(),
                        logical_bytes: body.len() as u64,
                    },
                    bytes: Arc::from(body.as_bytes()),
                });
            if seen.insert((
                source.trace_id.clone(),
                source.span_id.clone(),
                field,
                hash.clone(),
            )) {
                associations.push(SpanBodyAssociation {
                    project_id: project_id.clone(),
                    trace_id: source.trace_id.clone(),
                    span_id: source.span_id.clone(),
                    field,
                    body_hash: hash,
                    logical_bytes: body.len() as u64,
                });
            }
        }
    }
    (objects, associations)
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use chrono::{TimeZone, Utc};
    use sideseat_adapter_blob_storage::FilesystemStorage;
    use sideseat_adapter_sqlite::{SqliteRepository, SqliteService};
    use sideseat_core::storage::AppStorage;
    use sideseat_ports::clock::Clock;
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tempfile::TempDir;

    #[derive(Debug)]
    struct TestClock;

    impl Clock for TestClock {
        fn now(&self) -> chrono::DateTime<Utc> {
            test_now()
        }
    }

    fn test_now() -> chrono::DateTime<Utc> {
        Utc.timestamp_opt(1_704_067_200, 0).single().unwrap()
    }

    async fn setup() -> (
        TempDir,
        Arc<dyn TransactionalRepository + Send + Sync>,
        Arc<dyn FileStorage>,
        ContentBodyService,
    ) {
        let temp = TempDir::new().unwrap();
        let app_storage = AppStorage::init_for_test(temp.path().to_path_buf());
        let sqlite = Arc::new(
            SqliteService::init(&app_storage, Arc::new(TestClock))
                .await
                .unwrap(),
        );
        let database: Arc<dyn TransactionalRepository + Send + Sync> =
            Arc::new(SqliteRepository(sqlite));
        let storage: Arc<dyn FileStorage> =
            Arc::new(FilesystemStorage::new(temp.path().join("bodies")));
        let service = ContentBodyService {
            storage: Arc::clone(&storage),
            database: Arc::clone(&database),
        };
        (temp, database, storage, service)
    }

    fn message_row(messages: &str) -> MessageSpanRow {
        MessageSpanRow {
            trace_id: "trace".into(),
            span_id: "span".into(),
            parent_span_id: None,
            span_timestamp: test_now(),
            span_end_timestamp: Some(test_now()),
            messages_json: messages.into(),
            tool_definitions_json: "inline-tools".into(),
            tool_names_json: "inline-names".into(),
            body_cache_key: None,
            model: None,
            provider: None,
            status_code: None,
            exception_type: None,
            exception_message: None,
            exception_stacktrace: None,
            input_tokens: 0,
            output_tokens: 0,
            total_tokens: 0,
            cost_total: 0.0,
            observation_type: None,
            session_id: None,
            ingested_at: test_now(),
            scope_name: None,
            scope_version: None,
            span_name: None,
            framework: None,
            response_model: None,
            response_id: None,
            temperature: None,
            top_p: None,
            max_tokens: None,
            finish_reasons: None,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            reasoning_tokens: 0,
            cost_input: 0.0,
            cost_output: 0.0,
        }
    }

    struct BackfillRows(Vec<SpanBodySource>);

    #[async_trait]
    impl SurvivorReferences for BackfillRows {
        async fn file_reference_fields_for_traces(
            &self,
            _project_id: &ProjectId,
            _trace_ids: &[String],
        ) -> Result<Vec<String>, DataError> {
            Ok(Vec::new())
        }

        async fn span_body_fields_for_traces(
            &self,
            _project_id: &ProjectId,
            trace_ids: &[String],
        ) -> Result<Vec<SpanBodySource>, DataError> {
            Ok(self
                .0
                .iter()
                .filter(|row| trace_ids.contains(&row.trace_id))
                .cloned()
                .collect())
        }

        async fn span_body_backfill_page(
            &self,
            _project_id: &ProjectId,
            after: Option<(String, String)>,
            limit: usize,
        ) -> Result<Vec<SpanBodySource>, DataError> {
            Ok(self
                .0
                .iter()
                .filter(|row| {
                    after
                        .as_ref()
                        .is_none_or(|cursor| (&row.trace_id, &row.span_id) > (&cursor.0, &cursor.1))
                })
                .take(limit)
                .cloned()
                .collect())
        }
    }

    struct CountingStorage {
        inner: FilesystemStorage,
        gets: AtomicUsize,
    }

    impl CountingStorage {
        fn new(path: std::path::PathBuf) -> Self {
            Self {
                inner: FilesystemStorage::new(path),
                gets: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait]
    impl FileStorage for CountingStorage {
        async fn store(
            &self,
            project_id: &ProjectId,
            hash: &str,
            data: &[u8],
        ) -> Result<(), FileStorageError> {
            self.inner.store(project_id, hash, data).await
        }

        async fn get(
            &self,
            project_id: &ProjectId,
            hash: &str,
        ) -> Result<Vec<u8>, FileStorageError> {
            self.gets.fetch_add(1, Ordering::Relaxed);
            self.inner.get(project_id, hash).await
        }

        async fn exists(
            &self,
            project_id: &ProjectId,
            hash: &str,
        ) -> Result<bool, FileStorageError> {
            self.inner.exists(project_id, hash).await
        }

        async fn delete(&self, project_id: &ProjectId, hash: &str) -> Result<(), FileStorageError> {
            self.inner.delete(project_id, hash).await
        }

        async fn delete_project(&self, project_id: &ProjectId) -> Result<u64, FileStorageError> {
            self.inner.delete_project(project_id).await
        }

        async fn finalize_temp(
            &self,
            project_id: &ProjectId,
            hash: &str,
            temp_path: &Path,
        ) -> Result<(), FileStorageError> {
            self.inner.finalize_temp(project_id, hash, temp_path).await
        }
    }

    #[test]
    fn body_hash_is_domain_separated_and_stable() {
        assert_eq!(
            ContentBodyService::hash(b"same"),
            ContentBodyService::hash(b"same")
        );
        assert_ne!(
            ContentBodyService::hash(b"same"),
            hex::encode(Sha256::digest(b"same"))
        );
    }

    #[test]
    fn collection_deduplicates_objects_but_not_field_ownership() {
        let span = NormalizedSpan {
            project_id: Some("p".into()),
            trace_id: "t".into(),
            span_id: "s".into(),
            content_digest: "d".into(),
            messages: Some("[]".into()),
            tool_names: Some("[]".into()),
            ..Default::default()
        };
        let (objects, associations, digests) = collect(&[span]);
        assert_eq!(objects.len(), 1);
        assert_eq!(associations.len(), 2);
        assert_eq!(digests.len(), 1);
    }

    #[tokio::test]
    async fn confirmed_body_is_preferred_and_missing_blob_falls_back_inline() {
        let (_temp, database, storage, service) = setup().await;
        let project_id = ProjectId::from("project");
        let span = NormalizedSpan {
            project_id: Some(project_id.to_string()),
            trace_id: "trace".into(),
            span_id: "span".into(),
            content_digest: "digest".into(),
            messages: Some("body-messages".into()),
            tool_definitions: Some("body-tools".into()),
            tool_names: Some("body-names".into()),
            ..Default::default()
        };
        let staged = service.stage(&[span], None).await.unwrap();
        assert_eq!(
            database
                .confirm_span_bodies(&staged.associations)
                .await
                .unwrap(),
            3
        );

        let mut rows = vec![message_row("inline-messages")];
        service.hydrate_message_rows(&project_id, &mut rows).await;
        assert_eq!(rows[0].messages_json, "body-messages");
        assert_eq!(rows[0].tool_definitions_json, "body-tools");
        assert_eq!(rows[0].tool_names_json, "body-names");
        let hydrated_key = rows[0].body_cache_key.clone().unwrap();

        storage
            .delete(&project_id, &ContentBodyService::hash(b"body-messages"))
            .await
            .unwrap();
        let mut fallback = vec![message_row("inline-messages")];
        service
            .hydrate_message_rows(&project_id, &mut fallback)
            .await;
        assert_eq!(fallback[0].messages_json, "inline-messages");
        assert_ne!(
            fallback[0].body_cache_key.as_deref(),
            Some(hydrated_key.as_str())
        );
    }

    #[tokio::test]
    async fn shared_body_is_fetched_once_per_hydration() {
        let (temp, database, _storage, _service) = setup().await;
        let project_id = ProjectId::from("project");
        let storage = Arc::new(CountingStorage::new(temp.path().join("counted-bodies")));
        let service = ContentBodyService {
            storage: storage.clone(),
            database: Arc::clone(&database),
        };
        let spans = ["trace-a", "trace-b"].map(|trace_id| NormalizedSpan {
            project_id: Some(project_id.to_string()),
            trace_id: trace_id.into(),
            span_id: "span".into(),
            content_digest: format!("digest-{trace_id}"),
            messages: Some("shared-body".into()),
            ..Default::default()
        });
        let staged = service.stage(&spans, None).await.unwrap();
        database
            .confirm_span_bodies(&staged.associations)
            .await
            .unwrap();

        let mut rows = vec![message_row("inline"), message_row("inline")];
        rows[0].trace_id = "trace-a".into();
        rows[1].trace_id = "trace-b".into();
        service.hydrate_message_rows(&project_id, &mut rows).await;

        assert_eq!(rows[0].messages_json, "shared-body");
        assert_eq!(rows[1].messages_json, "shared-body");
        assert_eq!(storage.gets.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn backfill_resumes_after_a_full_page_and_marks_cutover_complete() {
        let (_temp, database, _storage, service) = setup().await;
        let project_id = ProjectId::from("project");
        let rows = BackfillRows(
            (0..300)
                .map(|index| SpanBodySource {
                    trace_id: format!("trace-{index:04}"),
                    span_id: "span".into(),
                    messages: Some("shared-body".into()),
                    tool_definitions: None,
                    tool_names: None,
                    raw_span: None,
                })
                .collect(),
        );

        let first = service
            .backfill_project_page(&project_id, &rows, test_now())
            .await
            .unwrap();
        assert!(!first.complete);
        assert_eq!(first.cursor_trace_id.as_deref(), Some("trace-0255"));

        let second = service
            .backfill_project_page(
                &project_id,
                &rows,
                test_now() + chrono::Duration::seconds(1),
            )
            .await
            .unwrap();
        assert!(second.complete);
        assert_eq!(second.cursor_trace_id.as_deref(), Some("trace-0299"));
        assert_eq!(
            database
                .get_span_body_hash(&project_id, "trace-0299", "span", SpanBodyField::Messages,)
                .await
                .unwrap()
                .as_deref(),
            Some(ContentBodyService::hash(b"shared-body").as_str())
        );
    }

    #[tokio::test]
    async fn stale_deletion_claim_is_finished_after_worker_crash() {
        let (_temp, database, storage, service) = setup().await;
        let project_id = ProjectId::from("project");
        let body_hash = ContentBodyService::hash(b"orphan");
        database
            .register_content_bodies(&[ContentBodyObject {
                project_id: project_id.clone(),
                body_hash: body_hash.clone(),
                logical_bytes: 6,
            }])
            .await
            .unwrap();
        storage
            .store(&project_id, &body_hash, b"orphan")
            .await
            .unwrap();
        assert!(
            database
                .claim_content_body_for_deletion(&project_id, &body_hash)
                .await
                .unwrap()
        );

        service
            .sweep_orphans(test_now() + chrono::Duration::seconds(301))
            .await;

        assert!(!storage.exists(&project_id, &body_hash).await.unwrap());
        assert!(
            database
                .get_stale_claimed_content_bodies(test_now() + chrono::Duration::hours(1), 10,)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn restore_cleanup_drains_more_than_one_orphan_page_without_grace() {
        let (_temp, database, storage, service) = setup().await;
        let project_id = ProjectId::from("project");
        let objects = (0..(ORPHAN_SWEEP_LIMIT + 44))
            .map(|index| {
                let bytes = format!("orphan-{index:03}");
                ContentBodyObject {
                    project_id: project_id.clone(),
                    body_hash: ContentBodyService::hash(bytes.as_bytes()),
                    logical_bytes: bytes.len() as u64,
                }
            })
            .collect::<Vec<_>>();
        database
            .register_content_bodies(&objects)
            .await
            .expect("register body orphans");
        for (index, object) in objects.iter().enumerate() {
            storage
                .store(
                    &project_id,
                    &object.body_hash,
                    format!("orphan-{index:03}").as_bytes(),
                )
                .await
                .expect("store body orphan");
        }

        let report = service
            .cleanup_orphans_after_restore()
            .await
            .expect("restore cleanup");

        assert_eq!(report.orphans_deleted, objects.len() as u64);
        assert_eq!(report.stale_claims_finalized, 0);
        assert!(
            database
                .get_orphan_content_bodies(
                    chrono::DateTime::<Utc>::from_timestamp_nanos(i64::MAX),
                    1,
                )
                .await
                .expect("remaining orphans")
                .is_empty()
        );
        for object in [objects.first().unwrap(), objects.last().unwrap()] {
            assert!(
                !storage
                    .exists(&project_id, &object.body_hash)
                    .await
                    .expect("orphan bytes lookup")
            );
        }
    }
}
