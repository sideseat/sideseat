use super::*;

impl TracePipeline {
    /// Remove everything belonging to a project that will not accept writes.
    ///
    /// Returns how many spans were dropped, so a caller can *report* a partial drop rather than answer an
    /// unqualified success for records it discarded. `Err(())` means the fence could not be read - which
    /// refuses the batch rather than assuming the fence is open, and clears the extraction cache as every
    /// refusal must, because its entries are written when bytes are extracted rather than stored.
    pub(super) async fn drop_spans_for_dead_projects(
        &self,
        spans: &mut Vec<NormalizedSpan>,
        files: &mut Vec<PendingFileWrite>,
        incoming: &mut Vec<IncomingReference>,
    ) -> Result<usize, ()> {
        let refusing = match self.projects_refusing_writes(spans).await {
            Ok(refusing) => refusing,
            Err(e) => {
                self.file_cache.invalidate_all();
                tracing::error!(
                    error = %e,
                    spans = spans.len(),
                    "Refusing the batch: could not settle whether its project accepts writes"
                );
                return Err(());
            }
        };
        if refusing.is_empty() {
            return Ok(0);
        }
        let before = spans.len();
        spans.retain(|s| !refusing.contains(s.project_id.as_deref().unwrap_or(DEFAULT_PROJECT_ID)));
        files.retain(|f| !refusing.contains(f.project_id.as_str()));
        incoming.retain(|(project, _, _)| !refusing.contains(project.as_str()));
        tracing::warn!(
            dropped = before - spans.len(),
            projects = ?refusing,
            "Dropped spans for projects that do not accept writes (deleted or being deleted)"
        );
        Ok(before - spans.len())
    }

    /// After the write, check the tombstones again and remove anything that slipped through.
    ///
    /// The check before the write cannot be atomic with it - the tombstone is a row in the transactional
    /// store and the spans go to the analytics store, so no transaction spans them. A deletion landing in
    /// that window would leave the trace resurrected: its files reclaimed, its row committed, and the
    /// caller already answered 204.
    ///
    /// So the window is closed by *compensation* rather than by locking: the write is followed by a second
    /// look, and anything now tombstoned is deleted along with the associations this batch created for it.
    /// This is not a substitute for the pre-write check - that is what keeps the common case from ever
    /// writing - and it is not sufficient alone either, since a crash between the write and this line
    /// leaves the spans. The deletion sweep is what covers that, exactly as it covers a project whose
    /// tombstone outlived its claim.
    /// Stamp each event with its trace's session as **the store** resolves it.
    ///
    /// A subscriber filtered by session compares `event.session_id`, so the value has to be the one a
    /// subsequent read will return - and only the store can say what that is. The canonical session is the
    /// root's, else the one on the trace's earliest span, and the root - or an earlier span - may have arrived in
    /// a previous batch, so resolving from the batch alone announced a span under session B while every read
    /// placed its trace under A. The live stream and the page then disagree about the same trace, which for a debugging tool
    /// reads as a message that appears and then cannot be found.
    ///
    /// Run **after** the write, so the store's answer includes this batch's own spans - and the store's answer
    /// is taken *whole*: a trace it reports no session for has no session, which is a fact and not a gap. Only
    /// overwriting the traces it named left a stale value standing, so a batch that corrected a span by
    /// removing its session still announced the old one.
    ///
    /// A **failed** read clears the session rather than falling back to the batch's view. Both outcomes lose
    /// something, and they are not symmetric: an event with no session reaches every unfiltered and
    /// trace-filtered subscriber and no session page, while a wrongly stamped one is delivered to a page the
    /// trace does not appear on *and* withheld from the page it does - a false positive and a false negative
    /// from one guess. The next read is authoritative either way.
    ///
    /// **Unconditional**, and that costs about 2.7 ms per batch (22.4 -> 25.1 ms on the `langgraph/swarm`
    /// ingestion benchmark). Skipping it for traces whose parentless span this batch carries would recover about
    /// half of that, and is still wrong: a root without a session leaves the earliest span to decide, and that span
    /// can be stored already.
    pub(super) async fn stamp_stored_sessions(&self, events: &mut [SseSpanEvent]) {
        let mut by_project: HashMap<String, HashSet<String>> = HashMap::new();
        for event in events.iter() {
            let project = event
                .project_id
                .as_deref()
                .unwrap_or(DEFAULT_PROJECT_ID)
                .to_string();
            by_project
                .entry(project)
                .or_default()
                .insert(event.trace_id.clone());
        }
        if by_project.is_empty() {
            return;
        }

        let mut stored: HashMap<(String, String), String> = HashMap::new();
        // Projects the store answered for. Absence from `stored` then means "no session", while absence from
        // here means "unknown" - two different answers that a single map cannot distinguish.
        let mut answered: HashSet<String> = HashSet::new();
        #[expect(
            clippy::iter_over_hash_type,
            reason = "each project's sessions are read into maps"
        )]
        for (project, trace_ids) in by_project {
            let typed_project = ProjectId::from(project.as_str());
            let trace_ids: Vec<String> = trace_ids.into_iter().collect();
            match self
                .analytics
                .as_ref()
                .get_trace_session_pairs(&typed_project, &trace_ids, None)
                .await
            {
                Ok(pairs) => {
                    answered.insert(project.clone());
                    for (trace, session) in pairs {
                        stored.insert((project.clone(), trace), session);
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        project_id = %project,
                        error = %e,
                        "Could not resolve canonical sessions for SSE; publishing these events with no \
                         session rather than one that may be wrong"
                    );
                }
            }
        }

        for event in events.iter_mut() {
            let project = event
                .project_id
                .as_deref()
                .unwrap_or(DEFAULT_PROJECT_ID)
                .to_string();
            let key = (project.clone(), event.trace_id.clone());
            // Answered: the store's word, including "none". Unanswered: no session, never a guess.
            event.session_id = if answered.contains(&project) {
                stored.get(&key).cloned()
            } else {
                None
            };
        }
    }

    /// After the write, tombstone the traces of any session deleted in the meantime.
    ///
    /// The session check before the write is not atomic with it, exactly as the trace check is not - and a
    /// session deletion landing in that window leaves a trace the deletion's own snapshot never named. This
    /// hands such traces to the *trace* protocol by tombstoning them, which then removes their rows on the
    /// next line and reconciles their files in the sweep. Doing it this way rather than duplicating the
    /// deletion logic means there is one place that knows how a trace is taken away.
    ///
    /// Only a trace this batch `begun` - wrote a parentless span of - is taken, as the pre-write fence fences only
    /// such a trace or one the store already knew. A child is no evidence of its trace's session, and right after
    /// the write the store's answer for a trace it knew nothing else of is that child's. Tombstoning on it lost
    /// acknowledged data: another instance writes the trace's earlier root under a live session, passes its checks
    /// and answers 200, and this tombstone - decided from the read before that root landed - then has the sweep
    /// delete it. A child-only trace of a deleted session is left to the session sweep, which re-resolves it.
    ///
    /// Returns the trace ids it tombstoned, so the caller can compensate them in the same pass.
    pub(super) async fn tombstone_traces_of_deleted_sessions(
        &self,
        written: &[(String, String, String)],
        begun: &HashSet<(String, String)>,
    ) -> HashSet<(String, String)> {
        let mut affected: HashSet<(String, String)> = HashSet::new();
        if written.is_empty() {
            return affected;
        }
        // The reads and the tombstones they decide in one pass; without the lease, the session sweep collects them.
        let fence = self.file_service.collection_fence().clone();
        let Some(_pass) = fence.enter("session compensation") else {
            return affected;
        };

        // Which session each written trace belongs to, **from the store**, not from the batch.
        //
        // This runs after the write, so the store holds these spans *and* everything delivered earlier - it
        // is the complete and authoritative view, which a batch never is. Taking the batch's answer here
        // undid the pre-write confirmation entirely: a child-only export naming a deleted session B, for a
        // trace stored under a live session A, was kept by the fence and then tombstoned by this - and the
        // sweep deleted its stored A content. It also fixes the concurrent case, where another instance's
        // root was committing while this batch was checked.
        let mut traces_by_project: HashMap<String, Vec<String>> = HashMap::new();
        for (project, trace, _) in written {
            if !begun.contains(&(project.clone(), trace.clone())) {
                continue;
            }
            traces_by_project
                .entry(project.clone())
                .or_default()
                .push(trace.clone());
        }
        let mut session_of: HashMap<(String, String), String> = HashMap::new();
        #[expect(
            clippy::iter_over_hash_type,
            reason = "each project's sessions are read into a map"
        )]
        for (project, mut trace_ids) in traces_by_project {
            let typed_project = ProjectId::from(project.as_str());
            trace_ids.sort_unstable();
            trace_ids.dedup();
            match self
                .analytics
                .as_ref()
                .get_trace_session_pairs(&typed_project, &trace_ids, None)
                .await
            {
                Ok(pairs) => {
                    for (trace, session) in pairs {
                        session_of.insert((project.clone(), trace), session);
                    }
                }
                Err(e) => {
                    // The deletion sweep re-resolves sessions, so anything missed here is collected there.
                    // Guessing from the batch is what this change exists to stop.
                    tracing::warn!(
                        error = %e,
                        project_id = %project,
                        "Could not resolve written traces' sessions after the write; leaving them to the \
                         deletion sweep"
                    );
                }
            }
        }
        if session_of.is_empty() {
            return affected;
        }

        // In order: what each project finds is journalled as tombstones, rows written in the order met.
        let mut sessions_by_project: BTreeMap<&str, Vec<String>> = BTreeMap::new();
        #[expect(
            clippy::iter_over_hash_type,
            reason = "grouped into lists each sorted before it is read"
        )]
        for ((project, _trace), session) in &session_of {
            sessions_by_project
                .entry(project.as_str())
                .or_default()
                .push(session.clone());
        }
        let repo = self.file_service.database().as_ref();
        for (project, mut session_ids) in sessions_by_project {
            let typed_project = ProjectId::from(project);
            session_ids.sort_unstable();
            session_ids.dedup();
            let deleted = match repo
                .deleted_sessions_among(&typed_project, &session_ids)
                .await
            {
                Ok(found) if found.is_empty() => continue,
                Ok(found) => found,
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        project,
                        "Could not re-check session tombstones after the write; the deletion sweep will \
                         collect anything that slipped through"
                    );
                    continue;
                }
            };
            let doomed: Vec<String> = written
                .iter()
                .filter(|(p, trace, _)| {
                    p == project
                        && session_of
                            .get(&(p.clone(), trace.clone()))
                            .is_some_and(|s| deleted.contains(s))
                })
                .map(|(_, trace, _)| trace.clone())
                .collect();
            if doomed.is_empty() {
                continue;
            }
            // The journalled form, and the argument for the plain one was wrong.
            //
            // It ran: "this is a fence, not a deletion - the session's own entry journalled it, and a second
            // entry is a duplicate counted against the quota." The first clause is true and the conclusion does
            // not follow. A session entry is replayed by *re-resolving the session* against restored data, and
            // these traces are exactly the ones a resolution can miss: a restore holding a trace's child spans
            // without the root that carried the session id cannot resolve it to the session, so nothing explains
            // its absence and it is resurrected.
            //
            // So the trace gets its own entry, in the same transaction as its tombstone. The duplication is real
            // and is the cheaper error: an id and an instant per trace, against a resurrected trace.
            if !fence.is_open() {
                break;
            }
            if let Err(e) = repo
                .record_deleted_traces_journalled(&typed_project, &doomed)
                .await
            {
                tracing::warn!(
                    error = %e,
                    project,
                    "Could not tombstone traces of a session deleted during the write; the session sweep \
                     will collect them"
                );
                continue;
            }
            tracing::warn!(
                project,
                traces = doomed.len(),
                "Tombstoned traces of sessions deleted during the write"
            );
            for trace in doomed {
                affected.insert((project.to_string(), trace));
            }
        }
        affected
    }

    pub(super) async fn collect_spans_written_for_deleted_traces(
        &self,
        written: &[(String, String, String)],
        created_associations: &mut Vec<(String, String, String)>,
    ) -> HashSet<(String, String, String)> {
        // Keyed by (project, trace, span), not (trace, span): a span id is unique only within a trace and a
        // trace id only within a project, so a survivor set that drops the project can let a compensated
        // span in one project suppress the SSE for a same-id survivor in another (gotcha #22). The deletion
        // itself was already per-project; only the returned identity was blind.
        let mut removed: HashSet<(String, String, String)> = HashSet::new();
        if written.is_empty() {
            return removed;
        }
        // The look and the delete in one pass: a tombstone read before a restore never deletes the spans the
        // restore brought back. Without the lease the spans stay, and the deleted-trace sweep collects them
        // once it is held again.
        let fence = self.file_service.collection_fence().clone();
        let Some(_pass) = fence.enter("write compensation") else {
            return removed;
        };
        // In order: what each project finds is deleted and its files released, writes made in the order met.
        let mut by_project: BTreeMap<&str, Vec<(String, String)>> = BTreeMap::new();
        for (project, trace, span) in written {
            by_project
                .entry(project.as_str())
                .or_default()
                .push((trace.clone(), span.clone()));
        }
        let repo = self.file_service.database().as_ref();
        for (project, mut identities) in by_project {
            let typed_project = ProjectId::from(project);
            identities.sort_unstable();
            identities.dedup();
            let mut trace_ids = identities
                .iter()
                .map(|(trace_id, _)| trace_id.clone())
                .collect::<Vec<_>>();
            trace_ids.sort_unstable();
            trace_ids.dedup();
            let tombstoned = match repo.deleted_traces_among(&typed_project, &trace_ids).await {
                Ok(found) => found,
                // Nothing to compensate on: the sweep will find these if they are genuinely deleted.
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        project,
                        "Could not re-check trace tombstones after the write; the deletion sweep will \
                         collect anything that slipped through"
                    );
                    HashSet::new()
                }
            };
            let journaled = match repo
                .journaled_spans_among(&typed_project, &identities)
                .await
            {
                Ok(found) => found,
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        project,
                        "Could not re-check journalled span deletions after the write; restore replay or \
                         retention cleanup must collect anything that slipped through"
                    );
                    HashSet::new()
                }
            };
            if tombstoned.is_empty() && journaled.is_empty() {
                continue;
            }
            let doomed: Vec<(String, String)> = written
                .iter()
                .filter(|(p, trace_id, span_id)| {
                    p == project
                        && (tombstoned.contains(trace_id)
                            || journaled.contains(&(trace_id.clone(), span_id.clone())))
                })
                .map(|(_, t, span)| (t.clone(), span.clone()))
                .collect();
            // The analytics delete is keyed (trace, span) within the project it is scoped to; the returned
            // set carries the project so the SSE filter can tell projects apart.
            // The rows go first, and the associations *only if* they went.
            //
            // Released unconditionally, a failed `delete_spans` leaves readable rows whose files are now
            // eligible for collection - a dangling reference, which is the one outcome the whole
            // write-files-before-rows ordering exists to make impossible. Leaving the association instead
            // leaves a file held by a row that is about to be swept, which the sweep then reconciles.
            if !fence.is_open() {
                break;
            }
            if let Err(e) = self
                .analytics
                .as_ref()
                .delete_spans(&typed_project, &doomed)
                .await
            {
                tracing::error!(
                    error = %e,
                    project,
                    spans = doomed.len(),
                    "Could not remove spans written for a deleted trace, so their file associations are \
                     left in place; releasing them would leave a readable row pointing at collectable \
                     bytes. The deletion sweep retries both."
                );
                continue;
            }
            tracing::warn!(
                project,
                spans = doomed.len(),
                traces = tombstoned.len(),
                journalled_spans = journaled.len(),
                "Removed spans whose trace or exact identity was deleted during the write"
            );
            removed.extend(
                doomed
                    .iter()
                    .map(|(t, span)| (project.to_string(), t.clone(), span.clone())),
            );
            let orphaned: Vec<(String, String, String)> = created_associations
                .iter()
                .filter(|(p, t, _)| p == project && tombstoned.contains(t))
                .cloned()
                .collect();
            // Remove them from the set as well, so the caller's confirm does not then mark a
            // just-released association durable. This runs *before* confirm for that reason.
            // Kept in the set while fenced: confirmed, they hold the files of rows the deletion sweep removes, and
            // it reconciles them then.
            if !fence.is_open() {
                break;
            }
            created_associations.retain(|(p, t, _)| !(p == project && tombstoned.contains(t)));
            self.release_created_associations(&orphaned).await;
        }
        removed
    }

    /// Release the associations of traces that are no longer in the batch, and **remove them from
    /// `created`**, so each association is resolved exactly once.
    ///
    /// The pruning is not incidental - it is what makes the counter model correct. `created` is later
    /// confirmed (on a successful write) or released (on a failed one), and with the counter an association
    /// that was already released here must not be touched again: a second release over-decrements
    /// `pending_writers` and can delete a peer batch's row, and a confirm marks a dropped association durable
    /// so its file is never reclaimed. Under the old boolean flag a double-resolve was a harmless no-op,
    /// which is why this was missing. Every drop - a dead project, a deleted session, a deleted trace -
    /// funnels through here, so the set that reaches the write holds only associations whose trace survived.
    pub(super) async fn release_associations_of_dropped(
        &self,
        created: &mut Vec<(String, String, String)>,
        surviving_spans: &[NormalizedSpan],
    ) {
        let orphaned = Self::take_associations_of_dropped(created, surviving_spans);
        self.release_created_associations(&orphaned).await;
    }

    /// As [`Self::release_associations_of_dropped`], for spans dropped because their rows are already stored:
    /// the associations are settled by those rows (`settle_associations_by_stored_rows`) instead of released.
    pub(super) async fn settle_associations_of_dropped(
        &self,
        created: &mut Vec<(String, String, String)>,
        surviving_spans: &[NormalizedSpan],
    ) {
        let orphaned = Self::take_associations_of_dropped(created, surviving_spans);
        self.settle_associations_by_stored_rows(&orphaned, &HashSet::new())
            .await;
    }

    /// Remove from `created`, and return, the associations of traces no surviving span belongs to.
    fn take_associations_of_dropped(
        created: &mut Vec<(String, String, String)>,
        surviving_spans: &[NormalizedSpan],
    ) -> Vec<(String, String, String)> {
        if created.is_empty() {
            return Vec::new();
        }
        let surviving: HashSet<(&str, &str)> = surviving_spans
            .iter()
            .map(|s| {
                (
                    s.project_id.as_deref().unwrap_or(DEFAULT_PROJECT_ID),
                    s.trace_id.as_str(),
                )
            })
            .collect();
        let orphaned: Vec<(String, String, String)> = created
            .iter()
            .filter(|(project, trace, _)| !surviving.contains(&(project.as_str(), trace.as_str())))
            .cloned()
            .collect();
        created
            .retain(|(project, trace, _)| surviving.contains(&(project.as_str(), trace.as_str())));
        orphaned
    }
    pub(super) async fn confirm_associations(
        &self,
        created: &[(String, String, String)],
        path: &str,
    ) {
        crate::media::confirm_associations(&self.file_service, created, path).await;
    }
    pub(super) async fn settle_associations_by_stored_rows(
        &self,
        associations: &[(String, String, String)],
        in_doubt: &HashSet<String>,
    ) {
        crate::media::settle_associations_by_stored_rows(
            &self.file_service,
            self.analytics.as_ref(),
            associations,
            in_doubt,
        )
        .await;
    }
    pub(super) async fn release_created_associations(&self, created: &[(String, String, String)]) {
        crate::media::release_created_associations(&self.file_service, created).await;
    }
}

/// The traces whose beginning - a span with no parent - `spans` carry: the only ones a batch has evidence of the
/// session of ([`TracePipeline::tombstone_traces_of_deleted_sessions`]).
pub(super) fn traces_begun(spans: &[NormalizedSpan]) -> HashSet<(String, String)> {
    spans
        .iter()
        .filter(|s| s.parent_span_id.as_deref().unwrap_or("").is_empty())
        .map(|s| {
            (
                s.project_id
                    .as_deref()
                    .unwrap_or(DEFAULT_PROJECT_ID)
                    .to_string(),
                s.trace_id.clone(),
            )
        })
        .collect()
}
