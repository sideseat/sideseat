use super::*;

impl TracePipeline {
    /// Remove spans belonging to sessions that have been deleted.
    ///
    /// Separate from the trace check, because they fence different things. A session is deleted by
    /// resolving it to trace ids and deleting those, so a trace of the same session that arrives *after*
    /// that resolution has no trace tombstone - it was never in the snapshot - and would recreate a session
    /// the caller was told was gone. The session id is durable; the trace list is one instant's view.
    ///
    /// `Err` means the table could not be read, which is not the same as empty: the caller refuses the
    /// batch so the exporter retries, exactly as for the project fence and the trace tombstone.
    pub(super) async fn drop_spans_for_deleted_sessions(
        &self,
        spans: &mut Vec<NormalizedSpan>,
    ) -> Result<usize, ()> {
        let mut by_project: HashMap<&str, Vec<String>> = HashMap::new();
        for span in spans.iter() {
            if let Some(session_id) = span.session_id.as_deref().filter(|s| !s.is_empty()) {
                by_project
                    .entry(span.project_id.as_deref().unwrap_or(DEFAULT_PROJECT_ID))
                    .or_default()
                    .push(session_id.to_string());
            }
        }
        if by_project.is_empty() {
            return Ok(0);
        }
        // The session reads and the tombstones they decide in one pass: a deletion read before a restore never
        // tombstones a trace the restore brought back.
        let fence = self.file_service.collection_fence().clone();
        let pass = fence.enter("session fence");
        let repo = self.file_service.database().as_ref();
        let mut deleted: HashSet<(String, String)> = HashSet::new();
        #[expect(
            clippy::iter_over_hash_type,
            reason = "each project's sessions are read into a set, so the order of the reads cannot matter"
        )]
        for (project, mut session_ids) in by_project {
            let typed_project = ProjectId::from(project);
            session_ids.sort_unstable();
            session_ids.dedup();
            match repo
                .deleted_sessions_among(&typed_project, &session_ids)
                .await
            {
                Ok(found) => {
                    deleted.extend(
                        found
                            .into_iter()
                            .map(|session_id| (project.to_string(), session_id)),
                    );
                }
                Err(e) => {
                    self.file_cache.invalidate_all();
                    tracing::error!(
                        error = %e,
                        project = project,
                        "Refusing the batch: could not settle whether its sessions have been deleted"
                    );
                    return Err(());
                }
            }
        }
        if deleted.is_empty() {
            return Ok(0);
        }

        // Candidates from the batch, then **confirmed against the store** before anything is destroyed.
        //
        // A batch sees only the spans in hand. A trace whose root was stored earlier under a live session A,
        // and whose *child* naming a deleted session B arrives now, looks like a B trace from the batch
        // alone - so it was tombstoned and the sweep then deleted its stored A content. Data loss caused by
        // a partial export, which is the ordinary shape of a streaming exporter.
        //
        // So the store decides when it holds the trace's root, whose session is the trace's; the batch decides
        // when it carries the root itself. Otherwise nothing yet says which session the trace is in, and it is
        // left to the session sweep, which waits for the root or `retention.session_deletion_grace_minutes`.
        // Erring toward keeping data and letting the sweep reclaim it is the right direction; the reverse
        // destroys rows no read can recover.
        let mut deleted_traces = traces_of_sessions(spans, &deleted);
        if !deleted_traces.is_empty() {
            let batch_sessions = canonical_session_of_traces(spans);

            // Which traces this batch actually witnesses the *beginning* of - it carries a span with no
            // parent.
            //
            // A span that has a parent is not a trace's root, whose session decides, so a batch of only such
            // spans is no evidence at all about which session the trace belongs to. That matters for a trace the store
            // has never seen, where the batch is the only evidence available: two instances ingesting one
            // trace concurrently, or an exporter whose first flush happens to carry a child, would otherwise
            // let a deleted session B tombstone a trace whose root says live session A - and the tombstone
            // is what makes that permanent.
            let batch_has_root: HashSet<(String, String)> =
                super::fences_early::traces_begun(spans);
            let mut by_project: HashMap<String, Vec<String>> = HashMap::new();
            #[expect(
                clippy::iter_over_hash_type,
                reason = "grouped into lists that are only read"
            )]
            for (project, trace) in &deleted_traces {
                by_project
                    .entry(project.clone())
                    .or_default()
                    .push(trace.clone());
            }
            #[expect(
                clippy::iter_over_hash_type,
                reason = "each project's sessions are read; what is kept is removed from a set"
            )]
            for (project, trace_ids) in by_project {
                let typed_project = ProjectId::from(project.as_str());
                match self
                    .analytics
                    .as_ref()
                    .get_trace_memberships(&typed_project, &trace_ids)
                    .await
                {
                    Ok(memberships) => {
                        let stored: HashMap<&str, &sideseat_ports::types::TraceMembership> =
                            memberships
                                .iter()
                                .map(|m| (m.trace_id.as_str(), m))
                                .collect();
                        for trace in &trace_ids {
                            let key = (project.clone(), trace.clone());
                            let fence = match stored.get(trace.as_str()) {
                                // The store holds the trace's root, naming its session: fence only if that is a
                                // deleted one - never on the strength of the batch's partial view.
                                Some(m) if m.rooted => {
                                    m.session_id.as_ref().is_some_and(|session| {
                                        deleted.contains(&(project.clone(), session.clone()))
                                    })
                                }
                                // Stored, with no deciding root. The batch's root may be a stale redelivery that a
                                // stored correction supersedes, so it does not decide here: the write goes ahead and
                                // the post-write check reads the winning revision. Without a root the trace is the
                                // session sweep's, which takes it once its root says so or
                                // `retention.session_deletion_grace_minutes` passes without one.
                                Some(_) => false,
                                // Not stored at all: the batch's own root decides.
                                None => batch_has_root.contains(&key),
                            };
                            if !fence {
                                tracing::debug!(
                                    project_id = %project,
                                    trace_id = %trace,
                                    batch_session = %batch_sessions.get(&key).cloned().unwrap_or_default(),
                                    "Not fencing a trace the batch attributed to a deleted session: its stored \
                                     root names a live one, or no root decides it yet"
                                );
                                deleted_traces.remove(&key);
                            }
                        }
                    }
                    Err(e) => {
                        // Refuse the batch rather than guess. Falling back to the batch's view was the
                        // pre-existing behaviour and it is the *destructive* side of the choice: a transient
                        // read failure would permanently tombstone a trace that belongs to a live session,
                        // and the sweep would then delete its stored rows. Nothing recovers that.
                        //
                        // The exporter still has this data, so an error it can retry loses nothing - which
                        // is the same reasoning the acknowledgement rules follow: a 200 has to mean stored,
                        // and where that cannot be established the answer is a failure, not a guess.
                        tracing::warn!(
                            error = %e,
                            project_id = %project,
                            "Could not confirm trace sessions against the store; refusing the batch so the \
                             exporter retries rather than risking a tombstone on a partial view"
                        );
                        return Err(());
                    }
                }
            }
        }
        if deleted_traces.is_empty() {
            return Ok(0);
        }

        // Tombstone them, so the trace sweep can find them later.
        //
        // Without this, a trace dropped *here* is invisible to both sweeps and its files leak. The sequence:
        // a writer associates a file for trace T and crashes before writing any analytics row; the session is
        // then deleted, but T was not in the deletion's snapshot so nothing tombstoned it; the redelivery
        // increments the association again and is dropped here, decrementing once - leaving `pending_writers`
        // at 1 and `ref_count` at 1 permanently. The session sweep cannot discover T (it resolves sessions to
        // traces through the analytics store, and T has no rows there), and the trace sweep cannot either
        // (it walks tombstones, and T has none). Recording the tombstone hands T to the trace sweep, which
        // deletes its rows, confirms nothing is readable, and then removes every association regardless of
        // `pending_writers`.
        //
        // A tombstone that cannot be written **refuses the batch**, exactly as an unreadable fence does.
        //
        // Logging and continuing was tried and is wrong: the drop then proceeds, this batch's own increment
        // is released, and the message is acknowledged - so the *earlier* crashed writer's increment is left
        // with no analytics row and no tombstone, which is precisely the state neither sweep can discover.
        // The leak becomes permanent on a transient database error. Refusing keeps the message pending, and
        // the redelivery writes the tombstone once the store recovers; the spans are dropped on that pass
        // instead, which is the same outcome one retry later.
        if !deleted_traces.is_empty() {
            // Projects and their traces in order: the tombstones are journal rows, and rows written in a
            // hash map's order stored different bytes from one process to the next.
            let mut by_project: BTreeMap<&str, Vec<String>> = BTreeMap::new();
            #[expect(
                clippy::iter_over_hash_type,
                reason = "grouped, then each project's traces are sorted before they are written"
            )]
            for (project, trace) in &deleted_traces {
                by_project
                    .entry(project.as_str())
                    .or_default()
                    .push(trace.clone());
            }
            // Without the lease, refused as an unwritable tombstone is: the message stays pending.
            if pass.is_none() || !fence.is_open() {
                return Err(());
            }
            let repo = self.file_service.database().as_ref();
            for (project, mut trace_ids) in by_project {
                if !fence.is_open() {
                    return Err(());
                }
                trace_ids.sort_unstable();
                let typed_project = ProjectId::from(project);
                // Journalled with the tombstone, in one transaction - see the compensation path above for why a
                // session entry alone does not cover these traces.
                if let Err(e) = repo
                    .record_deleted_traces_journalled(&typed_project, &trace_ids)
                    .await
                {
                    self.file_cache.invalidate_all();
                    tracing::error!(
                        error = %e,
                        project,
                        traces = trace_ids.len(),
                        "Refusing the batch: could not tombstone the traces of a deleted session, and \
                         dropping them untombstoned would leave any earlier writer's file associations \
                         permanently unreclaimable"
                    );
                    return Err(());
                }
            }
        }

        let before = spans.len();
        spans.retain(|s| {
            !deleted_traces.contains(&(
                s.project_id
                    .as_deref()
                    .unwrap_or(DEFAULT_PROJECT_ID)
                    .to_string(),
                s.trace_id.clone(),
            ))
        });
        tracing::warn!(
            dropped = before - spans.len(),
            sessions = deleted.len(),
            traces = deleted_traces.len(),
            "Dropped spans for sessions that have been deleted"
        );
        Ok(before - spans.len())
    }

    /// Remove exact span identities covered by the permanent deletion journal.
    ///
    /// This is the fence for requested span deletions and non-deterministic pressure eviction. A queued
    /// redelivery otherwise recreates the row immediately after the quota sweep removes it, undoing both the
    /// reclamation and the restore guarantee.
    pub(super) async fn drop_spans_for_journalled_deletions(
        &self,
        spans: &mut Vec<NormalizedSpan>,
    ) -> Result<usize, ()> {
        let mut by_project = HashMap::<&str, Vec<(String, String)>>::new();
        for span in spans.iter() {
            by_project
                .entry(span.project_id.as_deref().unwrap_or(DEFAULT_PROJECT_ID))
                .or_default()
                .push((span.trace_id.clone(), span.span_id.clone()));
        }
        let repo = self.file_service.database().as_ref();
        let mut deleted = HashSet::<(String, String, String)>::new();
        #[expect(
            clippy::iter_over_hash_type,
            reason = "each project's journalled spans are read into a set"
        )]
        for (project, mut identities) in by_project {
            identities.sort_unstable();
            identities.dedup();
            let typed_project = ProjectId::from(project);
            match repo
                .journaled_spans_among(&typed_project, &identities)
                .await
            {
                Ok(found) => {
                    deleted.extend(
                        found
                            .into_iter()
                            .map(|(trace_id, span_id)| (project.to_string(), trace_id, span_id)),
                    );
                }
                Err(error) => {
                    self.file_cache.invalidate_all();
                    tracing::error!(
                        project_id = project,
                        %error,
                        "Refusing the batch: could not read span deletion journal fences"
                    );
                    return Err(());
                }
            }
        }
        if deleted.is_empty() {
            return Ok(0);
        }
        let before = spans.len();
        spans.retain(|span| {
            !deleted.contains(&(
                span.project_id
                    .as_deref()
                    .unwrap_or(DEFAULT_PROJECT_ID)
                    .to_string(),
                span.trace_id.clone(),
                span.span_id.clone(),
            ))
        });
        tracing::warn!(
            dropped = before - spans.len(),
            "Dropped spans covered by requested or pressure deletion journal entries"
        );
        Ok(before - spans.len())
    }

    /// Remove spans belonging to traces that have been deleted.
    ///
    /// The project fence does not cover this: a live project can have an individual trace deleted, and
    /// this batch's files and associations were already written - deliberately, before the analytics row
    /// that references them. So a batch in flight when `delete_traces` ran would commit a span carrying
    /// a `#!B64!#` reference to bytes the deletion has reclaimed, for a trace the caller was told 204
    /// for, and a queued redelivery would do it minutes later.
    ///
    /// `Err` means the tombstone table could not be read, which is not the same as empty: the caller
    /// refuses the batch so the exporter retries, exactly as it does for an unreadable project fence.
    /// Ingestion is idempotent by span id, so a retry costs a rewrite.
    pub(super) async fn drop_spans_for_deleted_traces(
        &self,
        spans: &mut Vec<NormalizedSpan>,
    ) -> Result<usize, ()> {
        // Grouped by project, because the tombstone is keyed by both and a trace id comes from the
        // client - two projects can legitimately present the same one.
        let mut by_project: HashMap<&str, Vec<String>> = HashMap::new();
        for span in spans.iter() {
            by_project
                .entry(span.project_id.as_deref().unwrap_or(DEFAULT_PROJECT_ID))
                .or_default()
                .push(span.trace_id.clone());
        }
        let repo = self.file_service.database().as_ref();
        let mut deleted: HashSet<(String, String)> = HashSet::new();
        #[expect(
            clippy::iter_over_hash_type,
            reason = "each project's deleted traces are read into a set"
        )]
        for (project, mut trace_ids) in by_project {
            let typed_project = ProjectId::from(project);
            trace_ids.sort_unstable();
            trace_ids.dedup();
            match repo.deleted_traces_among(&typed_project, &trace_ids).await {
                Ok(found) => {
                    deleted.extend(
                        found
                            .into_iter()
                            .map(|trace_id| (project.to_string(), trace_id)),
                    );
                }
                Err(e) => {
                    self.file_cache.invalidate_all();
                    tracing::error!(
                        error = %e,
                        project = project,
                        "Refusing the batch: could not settle whether its traces have been deleted"
                    );
                    return Err(());
                }
            }
        }
        if deleted.is_empty() {
            return Ok(0);
        }
        let before = spans.len();
        spans.retain(|s| {
            !deleted.contains(&(
                s.project_id
                    .as_deref()
                    .unwrap_or(DEFAULT_PROJECT_ID)
                    .to_string(),
                s.trace_id.clone(),
            ))
        });
        tracing::warn!(
            dropped = before - spans.len(),
            traces = deleted.len(),
            "Dropped spans for traces that have been deleted"
        );
        Ok(before - spans.len())
    }

    /// Which of a batch's projects will not accept writes, and whether the question could be answered.
    ///
    /// `Err` means the fence is unknown, which is not the same as open. Dropping a live project's spans
    /// because a query failed would be data loss caused by the fence, and writing to a project that may
    /// be going away is the corruption it exists to prevent - so the caller refuses the batch instead and
    /// the exporter retries. Ingestion is idempotent by span id, so a retry costs a rewrite.
    ///
    /// One query per distinct project, and a batch almost always names one: the SDK sends a project's
    /// spans to that project's endpoint.
    pub(super) async fn projects_refusing_writes(
        &self,
        spans: &[NormalizedSpan],
    ) -> Result<HashSet<String>, String> {
        // Same default the write path applies, so the fence and the write agree on which project a
        // span without one belongs to.
        let mut projects: Vec<&str> = spans
            .iter()
            .map(|s| s.project_id.as_deref().unwrap_or(DEFAULT_PROJECT_ID))
            .collect();
        projects.sort_unstable();
        projects.dedup();
        let repo = self.file_service.database().as_ref();
        let mut refusing = HashSet::new();
        for project in projects {
            match repo.project_accepts_writes(project).await {
                Ok(true) => {}
                Ok(false) => {
                    refusing.insert(project.to_string());
                }
                Err(e) => return Err(format!("project {project}: {e}")),
            }
        }
        Ok(refusing)
    }

    /// Remove exact at-least-once redeliveries before they create another analytics revision.
    ///
    /// Corrections remain append-only: only a row whose current winner has the same content digest is
    /// skipped. On any read error the row is preserved, because an unnecessary revision is safer than
    /// dropping a change.
    pub(super) async fn drop_exact_redeliveries(&self, spans: &mut Vec<NormalizedSpan>) -> usize {
        let mut by_project: HashMap<String, Vec<(usize, String, String, String)>> = HashMap::new();
        for (index, span) in spans.iter().enumerate() {
            by_project
                .entry(
                    span.project_id
                        .clone()
                        .unwrap_or_else(|| DEFAULT_PROJECT_ID.to_string()),
                )
                .or_default()
                .push((
                    index,
                    span.trace_id.clone(),
                    span.span_id.clone(),
                    span.content_digest.clone(),
                ));
        }

        let mut exact = HashSet::new();
        #[expect(
            clippy::iter_over_hash_type,
            reason = "each project's exact redeliveries are read into a set of batch indexes"
        )]
        for (project, records) in by_project {
            let project_id = ProjectId::from(project.as_str());
            let digests = records
                .iter()
                .map(|(_, trace, span, digest)| (trace.clone(), span.clone(), digest.clone()))
                .collect::<Vec<_>>();
            let matching = match self
                .analytics
                .spans_with_matching_content(&project_id, &digests)
                .await
            {
                Ok(matching) => matching,
                Err(error) => {
                    tracing::warn!(
                        %error,
                        %project_id,
                        "Could not check exact span redeliveries; preserving the revisions"
                    );
                    continue;
                }
            };
            let candidates: Vec<(String, String)> = records
                .iter()
                .filter(|(_, trace, span, digest)| {
                    matching.contains(&(trace.clone(), span.clone(), digest.clone()))
                })
                .map(|(_, trace, span, _)| (trace.clone(), span.clone()))
                .collect();
            // Matching rows make a redelivery skippable only if the raw authority holds them too: rows a failed
            // repair left uncovered are written again, which repairs the record, instead of being skipped for
            // ever while the export can never settle.
            match crate::raw_coverage::covered(
                self.analytics.as_ref(),
                self.file_service.database().as_ref(),
                &project_id,
                &candidates,
            )
            .await
            {
                Ok(covered) => exact.extend(
                    records
                        .into_iter()
                        .filter(|(_, trace, span, digest)| {
                            matching.contains(&(trace.clone(), span.clone(), digest.clone()))
                                && covered.contains(&(trace.clone(), span.clone()))
                        })
                        .map(|(index, _, _, _)| index),
                ),
                Err(error) => tracing::warn!(
                    %error,
                    %project_id,
                    "Could not check exact span redeliveries; preserving the revisions"
                ),
            }
        }

        if exact.is_empty() {
            return 0;
        }
        let before = spans.len();
        let mut index = 0usize;
        spans.retain(|_| {
            let keep = !exact.contains(&index);
            index += 1;
            keep
        });
        before - spans.len()
    }
}
