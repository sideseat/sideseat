use super::*;

impl TracePipeline {
    pub fn new(
        analytics: Arc<dyn AnalyticsRepository + Send + Sync>,
        pricing: Arc<PricingService>,
        topics: Arc<TopicService>,
        file_service: Arc<FileService>,
        staging: Arc<StagingService>,
    ) -> Self {
        Self {
            analytics,
            pricing,
            topics,
            file_service,
            staging,
            storage_governance: None,
            file_cache: FileExtractionCache::new(),
        }
    }

    pub fn with_storage_governance(
        mut self,
        storage_governance: Arc<StorageGovernanceService>,
    ) -> Self {
        self.storage_governance = Some(storage_governance);
        self
    }

    /// Start the pipeline processor, consuming from the given stream topic.
    ///
    /// Uses consumer groups for at-least-once delivery:
    /// - Messages are acknowledged after successful processing
    /// - Unacknowledged messages are re-delivered on restart
    /// - Stuck messages are claimed after CLAIM_MIN_IDLE_MS
    pub fn start(
        self: Arc<Self>,
        topic: StreamTopic<StagedPayloadRef>,
        mut shutdown_rx: watch::Receiver<bool>,
    ) -> JoinHandle<()> {
        // Generate unique consumer name: {uuid}:{pid}
        let consumer = format!("{}:{}", Uuid::new_v4(), std::process::id());

        tokio::spawn(async move {
            // Subscribe, retrying until it succeeds or the server is shutting down.
            //
            // A single failure used to end the task while the instance stayed healthy and kept accepting
            // writes - so every export was queued and *nothing* consumed it, for the life of the process,
            // with no error after the first line. Redis being briefly unreachable at startup is the
            // ordinary way to reach that, and it is exactly when a retry is obviously right.
            let mut subscriber = loop {
                match topic.subscribe(CONSUMER_GROUP, &consumer).await {
                    Ok(s) => break s,
                    Err(e) => {
                        if *shutdown_rx.borrow() {
                            tracing::debug!(
                                "TracePipeline abandoning subscription during shutdown"
                            );
                            return;
                        }
                        tracing::error!(
                            error = %e,
                            "Failed to subscribe to the trace topic; retrying. Until this succeeds \
                             nothing is consuming the queue."
                        );
                        tokio::select! {
                            biased;
                            changed = shutdown_rx.changed() => {
                                if changed.is_err() || *shutdown_rx.borrow() {
                                    return;
                                }
                            }
                            _ = tokio::time::sleep(SUBSCRIBE_RETRY_DELAY) => {}
                        }
                    }
                }
            };

            // Get acker and claimer for message operations (Send + Sync)
            let acker = subscriber.acker();
            let claimer = subscriber.claimer();

            tracing::debug!(
                consumer = %consumer,
                group = CONSUMER_GROUP,
                "TracePipeline started"
            );

            // Create interval for periodic claim recovery
            let mut claim_interval =
                tokio::time::interval(Duration::from_secs(CLAIM_INTERVAL_SECS));
            claim_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            // Tokio's first tick is immediate; consume it so recovery starts after one full interval.
            claim_interval.tick().await;

            let mut shutdown_requested = false;
            let mut buffered = WeightedFairQueue::new();

            loop {
                if shutdown_requested {
                    // Drain remaining messages with timeout
                    let queued = buffered.pop_batch(1).pop();
                    let next = match queued {
                        Some(pair) => Ok(Ok(pair)),
                        None => tokio::time::timeout(
                            Duration::from_millis(100),
                            subscriber.recv_partitioned(),
                        )
                        .await
                        .map(|result| result.map(|(id, _, message)| (id, message)))
                        .map_err(|_| ()),
                    };
                    match next {
                        Ok(Ok((msg_id, msg))) => {
                            if self.process_staged_reference(&msg).await {
                                if let Err(e) = acker.ack(&msg_id).await {
                                    tracing::warn!(error = %e, msg_id = %msg_id, "Failed to ack during drain");
                                }
                            } else {
                                tracing::warn!(msg_id = %msg_id, "Skipping ack during drain: staged delivery is still pending");
                            }
                            continue;
                        }
                        Ok(Err(TopicError::Lagged(n))) => {
                            tracing::warn!(lagged = n, "TracePipeline lagged during drain");
                            continue;
                        }
                        _ => break,
                    }
                }

                // Wait for the first delivery while servicing shutdown and abandoned claims.
                //
                // Arm order matters. `biased` polls in order and takes the first ready branch, so the
                // *maintenance tick* must come **before** the receive branch: under sustained saturation
                // `subscriber.recv()` is always ready, and putting it first meant claiming and trimming
                // never fired. That is the case they matter most - a full stream is exactly when trim
                // has work to do - so an ordering that starves them there defeats their purpose. The
                // tick still yields to shutdown, which is what `biased` earns its keep for.
                let first = tokio::select! {
                    biased;
                    changed = shutdown_rx.changed() => {
                        if changed.is_err() || *shutdown_rx.borrow() {
                            tracing::debug!("TracePipeline received shutdown, draining...");
                            shutdown_requested = true;
                        }
                        continue;
                    }
                    _ = claim_interval.tick() => {
                        // Periodically claim stuck messages from other consumers
                        self.claim_stuck_messages(&claimer, &acker, &consumer).await;
                        // And discard what every group is finished with. This is the only thing that
                        // bounds the stream: publishing no longer trims by length, because a length
                        // bound deletes the oldest entries whether or not anyone read them - and each
                        // had already been answered 200.
                        match claimer.trim_consumed().await {
                            Ok(0) => {}
                            Ok(trimmed) => tracing::debug!(trimmed, "Trimmed consumed stream entries"),
                            Err(e) => tracing::warn!(error = %e, "Failed to trim consumed stream entries"),
                        }
                        continue;
                    }
                    _ = async {}, if !buffered.is_empty() => None,
                    result = subscriber.recv_partitioned(), if buffered.is_empty() => {
                        match result {
                            Ok(delivery) => Some(delivery),
                            Err(TopicError::Lagged(n)) => {
                                tracing::warn!(lagged = n, "TracePipeline lagged");
                                continue;
                            }
                            Err(TopicError::ChannelClosed) => break,
                            // A payload nothing can parse is acknowledged and skipped, not fatal.
                            //
                            // This used to break out of the loop, so one malformed message stopped all
                            // trace ingestion for the life of the process - and because it was never
                            // acknowledged, a restart was met by the same message. Nothing can store a
                            // payload it cannot decode, so the choice is between discarding one message
                            // loudly and blocking every message behind it silently.
                            Err(TopicError::Undecodable { id, detail, raw }) => {
                                // Preserve the accepted bytes on the dead-letter stream before acking. The
                                // entry decoded as a stream structure but not as a trace export - a corrupt
                                // or version-incompatible protobuf - so `stream_claim`'s structural
                                // dead-lettering never saw it, and a bare ack would silently lose bytes
                                // answered 200. Ack only if the payload is safely preserved.
                                match acker.dead_letter(&id, "invalid_protobuf", &raw).await {
                                    Ok(()) => {
                                        tracing::error!(
                                            message_id = %id,
                                            error = %detail,
                                            "Dead-lettered a queued payload that cannot be decoded"
                                        );
                                        if let Err(e) = acker.ack(&id).await {
                                            tracing::warn!(message_id = %id, error = %e, "Could not acknowledge a dead-lettered payload");
                                        }
                                    }
                                    Err(e) => tracing::error!(
                                        message_id = %id,
                                        error = %e,
                                        "Could not preserve an undecodable payload on the dead-letter stream; leaving it pending rather than discarding it"
                                    ),
                                }
                                continue;
                            }
                            Err(e) => {
                                tracing::error!(error = %e, "TracePipeline receive error");
                                break;
                            }
                        }
                    }
                };

                if let Some((msg_id, partition, payload_ref)) = first {
                    buffered.push(partition, (msg_id, payload_ref));
                }

                // Prefetch beyond one write batch, then choose a weighted-fair batch.
                while buffered.len() < PIPELINE_PREFETCH_MAX_SIZE {
                    match tokio::time::timeout(
                        Duration::from_micros(PIPELINE_BATCH_DRAIN_TIMEOUT_US),
                        subscriber.recv_partitioned(),
                    )
                    .await
                    {
                        Ok(Ok((msg_id, partition, payload_ref))) => {
                            buffered.push(partition, (msg_id, payload_ref));
                        }
                        _ => break,
                    }
                }
                let batch = buffered.pop_batch(PIPELINE_BATCH_MAX_SIZE);

                let batch_size = batch.len();
                if batch_size > 1 {
                    tracing::debug!(batch_size, "Processing batched requests");
                }

                // Load the selected batch before handing it to the persistence port.
                let mut ready = Vec::with_capacity(batch.len());
                let mut ack_ids = Vec::new();
                for (msg_id, payload_ref) in batch {
                    match self.load_staged_trace(&payload_ref).await {
                        Ok(Some((payload, request, received))) => {
                            ready.push((msg_id, payload, request, received));
                        }
                        Ok(None) => ack_ids.push(msg_id),
                        Err(error) => {
                            tracing::error!(
                                staged_payload_id = %payload_ref.id,
                                %error,
                                "Could not load a staged trace payload"
                            );
                            if self.note_staging_failure(&payload_ref.id).await {
                                ack_ids.push(msg_id);
                            }
                        }
                    }
                }

                if !ready.is_empty() {
                    let requests = ready
                        .iter()
                        .map(|(_, _, request, _)| request.clone())
                        .collect::<Vec<_>>();
                    let received = ready
                        .iter()
                        .map(|(_, _, _, received)| received.clone())
                        .collect::<Vec<_>>();
                    let db_ok = self.run_batch(&requests, &received).await;
                    for (msg_id, payload, _, _) in ready {
                        if db_ok && self.settle_staged_trace(&payload).await {
                            ack_ids.push(msg_id);
                        } else if self.note_staging_failure(&payload.id).await {
                            // Cap exhaustion quarantines the blob/registry row. Stop queue churn,
                            // but never release its bytes.
                            ack_ids.push(msg_id);
                        }
                    }
                }

                if !ack_ids.is_empty()
                    && let Err(e) = acker.ack_batch(&ack_ids).await
                {
                    tracing::warn!(error = %e, count = ack_ids.len(), "Failed to batch ack staged trace references");
                }
            }

            tracing::debug!("TracePipeline shutdown complete");
        })
    }

    /// Claim and process stuck messages from other consumers.
    ///
    /// Messages that have been pending for longer than CLAIM_MIN_IDLE_MS are
    /// claimed from other (possibly crashed) consumers, processed, and acknowledged.
    async fn claim_stuck_messages(
        &self,
        claimer: &StreamClaimer,
        acker: &StreamAcker,
        consumer: &str,
    ) {
        match claimer
            .claim(consumer, CLAIM_MIN_IDLE_MS, CLAIM_MAX_COUNT)
            .await
        {
            Ok(messages) if messages.is_empty() => {
                tracing::trace!("No stuck messages to claim");
            }
            Ok(messages) => {
                let count = messages.len();
                tracing::debug!(count, "Claiming stuck messages");

                for msg in messages {
                    // Decode and process the claimed message
                    match StagedPayloadRef::decode(&msg.payload[..]) {
                        Ok(payload_ref) => {
                            if self.process_staged_reference(&payload_ref).await {
                                if let Err(e) = acker.ack(&msg.id).await {
                                    tracing::warn!(error = %e, msg_id = %msg.id, "Failed to ack claimed message");
                                }
                            } else {
                                tracing::warn!(msg_id = %msg.id, "Skipping ack for claimed staged reference: delivery remains pending");
                            }
                        }
                        Err(e) => {
                            // Preserve the bytes on the dead-letter stream, then ack. A claimed entry that
                            // cannot be decoded was answered 200 when queued, so discarding it silently loses
                            // accepted data; ack only once it is safely dead-lettered, or leave it pending.
                            match acker
                                .dead_letter(&msg.id, "invalid_protobuf", &msg.payload)
                                .await
                            {
                                Ok(()) => {
                                    tracing::error!(error = %e, msg_id = %msg.id, "Dead-lettered an undecodable claimed message");
                                    if let Err(ack_err) = acker.ack(&msg.id).await {
                                        tracing::warn!(error = %ack_err, msg_id = %msg.id, "Failed to ack a dead-lettered message");
                                    }
                                }
                                Err(dl_err) => tracing::error!(
                                    error = %dl_err,
                                    msg_id = %msg.id,
                                    "Could not preserve an undecodable claimed message on the dead-letter stream; leaving it pending"
                                ),
                            }
                        }
                    }
                }

                tracing::debug!(count, "Finished processing claimed messages");
            }
            Err(e) => {
                tracing::warn!(error = %e, "Failed to claim stuck messages");
            }
        }
    }

    async fn load_staged_trace(
        &self,
        payload_ref: &StagedPayloadRef,
    ) -> Result<Option<(StagedPayload, ExportTraceServiceRequest, ReceivedPayload)>, String> {
        let Some((payload, bytes)) = self
            .staging
            .load(&payload_ref.id)
            .await
            .map_err(|error| error.to_string())?
        else {
            return Ok(None);
        };
        if payload.signal != StagedSignal::Traces {
            return Err(format!(
                "queue reference names a {:?} payload on the trace topic",
                payload.signal
            ));
        }
        let (request, received) =
            crate::received::staged_traces(&bytes, payload.project_id.as_str())?;
        Ok(Some((payload, request, received)))
    }

    async fn settle_staged_trace(&self, payload: &StagedPayload) -> bool {
        match self.staging.settle(payload).await {
            Ok(StagingDisposition::Confirmed) => true,
            Ok(StagingDisposition::DeliberatelyAbsent) => {
                tracing::info!(
                    staged_payload_id = %payload.id,
                    "Discarded a staged trace payload whose absent records are explained by deletion or retention"
                );
                true
            }
            Ok(StagingDisposition::Pending) => false,
            Err(error) => {
                tracing::warn!(
                    staged_payload_id = %payload.id,
                    %error,
                    "Could not confirm a staged trace payload"
                );
                false
            }
        }
    }

    async fn note_staging_failure(&self, id: &str) -> bool {
        match self.staging.note_failed_attempt(id).await {
            Ok(exhausted) => exhausted,
            Err(error) => {
                tracing::error!(
                    staged_payload_id = id,
                    %error,
                    "Could not update staged-payload redrive state"
                );
                false
            }
        }
    }

    async fn process_staged_reference(&self, payload_ref: &StagedPayloadRef) -> bool {
        let (payload, request, received) = match self.load_staged_trace(payload_ref).await {
            Ok(Some(loaded)) => loaded,
            Ok(None) => return true,
            Err(error) => {
                tracing::error!(
                    staged_payload_id = %payload_ref.id,
                    %error,
                    "Could not load staged trace reference"
                );
                return self.note_staging_failure(&payload_ref.id).await;
            }
        };
        if !self.run(&request, &received).await.is_final() {
            return self.note_staging_failure(&payload.id).await;
        }
        if self.settle_staged_trace(&payload).await {
            true
        } else {
            self.note_staging_failure(&payload.id).await
        }
    }
}
