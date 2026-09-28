use async_stream::stream;
use async_trait::async_trait;

use super::protocol::{extract_message_fields, parse_xreadgroup_response};
use super::*;

#[async_trait]
impl TopicBackend for RedisTopicBackend {
    // =========================================================================
    // Broadcast (Pub/Sub)
    // =========================================================================

    async fn publish(&self, topic: &str, payload: &[u8]) -> Result<(), TopicError> {
        let channel = self.pubsub_channel(topic);
        let mut conn = self.pool.get().await.map_err(pool_error)?;

        // PUBLISH to Redis ONLY (not to local bridge)
        // Messages flow: Redis → Bridge Task → Local Broadcast → Subscribers
        // This eliminates duplicate messages for same-process pub/sub
        let _: i64 = deadpool_redis::redis::cmd("PUBLISH")
            .arg(&channel)
            .arg(payload)
            .query_async(&mut conn)
            .await
            .map_err(redis_error)?;

        Ok(())
    }

    async fn subscribe(&self, topic: &str) -> Result<BroadcastSubscription, TopicError> {
        // Get or create bridge
        let (bridge, is_new) = self.pubsub_manager.get_or_create_bridge(topic);

        // Start bridge task if this is a new bridge
        if is_new {
            self.start_bridge_task(topic);
        }

        // Increment subscriber count
        bridge.add_subscriber();

        // Get receiver from local broadcast
        let receiver = bridge.subscribe();

        // Create managed subscription (cleans up on drop)
        let managed = ManagedSubscription::new(
            receiver,
            Arc::clone(&bridge),
            Arc::clone(&self.pubsub_manager),
        );

        // Wrap in stream
        let stream = stream! {
            let mut managed = managed;
            loop {
                match managed.recv().await {
                    Ok(payload) => yield Ok(payload),
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        yield Err(TopicError::Lagged(n));
                    }
                }
            }
        };

        Ok(BroadcastSubscription {
            receiver: Box::pin(stream),
        })
    }

    // =========================================================================
    // Stream
    // =========================================================================

    /// Append to the stream, refusing rather than trimming when the backlog is too large.
    ///
    /// Deliberately no `MAXLEN`: length-based trimming cannot know what consumer groups still need. Entries
    /// leave through `stream_trim_consumed`; excess backlog returns `BufferFull`.
    async fn stream_publish(
        &self,
        topic: &str,
        // Redis streams are a single log per key, so there is no partition to choose - the key is recorded on the
        // entry instead, which is what a later migration to a partitioned broker reads.
        partition_key: &str,
        payload: &[u8],
    ) -> Result<String, TopicError> {
        let key = self.stream_key(topic);

        // Fast path: this instance's own observation says the stream is at its limit.
        //
        // On its own that would strand a replica indefinitely - another instance can trim the stream and
        // this one would never learn, so its cache stays at the limit and every publish is refused
        // against an empty backlog. So a *reachable* limit here upgrades to a fresh `XLEN` before
        // refusing, which is one round trip on the refusal path only. The steady state (below the
        // limit) still pays no extra round trip: the previous publish's pipelined XLEN is what
        // populates `observed_backlog`, and reads there decide the fast path.
        if self.observed_backlog.get(&key).map(|o| *o).unwrap_or(0) >= self.max_backlog() {
            let mut conn = self.pool.get().await.map_err(pool_error)?;
            let fresh: u64 = deadpool_redis::redis::cmd("XLEN")
                .arg(&key)
                .query_async(&mut conn)
                .await
                .unwrap_or(u64::MAX);
            self.observed_backlog.insert(key.clone(), fresh);
            if fresh >= self.max_backlog() {
                return Err(TopicError::BufferFull);
            }
        }

        let mut conn = self.pool.get().await.map_err(pool_error)?;

        // One round trip for both, so the threshold costs nothing in the steady state.
        let mut pipe = deadpool_redis::redis::pipe();
        pipe.cmd("XADD")
            .arg(&key)
            .arg("*")
            .arg("payload")
            .arg(payload)
            // Recorded on the entry. Redis has no partitions, so this changes nothing here - it is what a later
            // migration to a partitioned broker reads to place the record, and writing it now means the history
            // is already keyed when that happens.
            .arg("partition_key")
            .arg(partition_key)
            .cmd("XLEN")
            .arg(&key);
        let (id, length): (String, u64) = pipe.query_async(&mut conn).await.map_err(redis_error)?;

        // Record the length returned by XADD before waiting for durability. The entry exists even if replica
        // confirmation fails and the exporter retries it.
        let was_over = length >= self.max_backlog();
        self.observed_backlog.insert(key.clone(), length);

        // Durability across a failover, when the deployment has replicas to lose.
        //
        // `WAITAOF`, not `WAIT`. `WAIT` blocks until N replicas have the entry *in memory*, which is not the
        // guarantee being bought here: a replica that received the write but had not yet fsynced it, then
        // promoted after a crash, serves a keyspace without the entry - and the exporter was told 200 long
        // ago. `WAITAOF numlocal numreplicas` blocks until the append-only file has been fsynced locally and
        // on that many replicas, which is the actual "survives a failover" property. It requires
        // `appendonly yes`, which the startup durability probe already enforces.
        //
        // `numlocal = 1` is confirmed too, not assumed: `appendfsync always` makes it true, but confirming
        // it costs nothing on top of the round trip already being paid and turns a silent misconfiguration
        // into a refusal. A shortfall on either count is reported so the OTLP route answers 503 and the data
        // stays with the exporter.
        if self.min_replica_acks > 0 {
            let acked: Vec<i64> = deadpool_redis::redis::cmd("WAITAOF")
                .arg(1)
                .arg(self.min_replica_acks)
                .arg(REPLICA_ACK_TIMEOUT_MS)
                .query_async(&mut conn)
                .await
                .map_err(|e| {
                    TopicError::Stream(format!("WAITAOF for durable acknowledgement failed: {e}"))
                })?;
            // `[numlocal, numreplicas]`.
            let local = acked.first().copied().unwrap_or(0);
            let replicas = acked.get(1).copied().unwrap_or(0);
            if local < 1 || (replicas as u32) < self.min_replica_acks {
                return Err(TopicError::Stream(format!(
                    "a queued trace was fsynced locally={local} and on {replicas} of {} replicas within \
                     {}ms; refusing to report it as durably stored",
                    self.min_replica_acks, REPLICA_ACK_TIMEOUT_MS
                )));
            }
        }

        if was_over {
            tracing::warn!(
                stream = %key,
                length,
                limit = self.max_backlog(),
                "Stream backlog is at its limit; further publishes are refused until consumers catch up"
            );
        }

        Ok(id)
    }

    /// Remove entries that every consumer group has finished with.
    ///
    /// The safe boundary is the oldest entry any group still needs: its oldest *pending* entry if it has
    /// one, otherwise one past its last delivered id. `XTRIM MINID` then removes strictly older entries,
    /// so nothing unread and nothing unacknowledged is ever deleted - which is the whole difference from
    /// the `MAXLEN` this replaces.
    ///
    /// A stream with no consumer group is left alone: nobody has read it yet, so every entry is still
    /// needed. Returning 0 there rather than trimming is the difference between an idle stream and an
    /// emptied one.
    async fn stream_trim_consumed(&self, topic: &str) -> Result<u64, TopicError> {
        let key = self.stream_key(topic);
        let mut conn = self.pool.get().await.map_err(pool_error)?;

        let groups: RedisValue = deadpool_redis::redis::cmd("XINFO")
            .arg("GROUPS")
            .arg(&key)
            .query_async(&mut conn)
            .await
            .unwrap_or(RedisValue::Nil);
        let RedisValue::Array(groups) = groups else {
            return Ok(0);
        };
        if groups.is_empty() {
            return Ok(0);
        }

        let mut boundary: Option<StreamId> = None;
        for group in &groups {
            let Some(name) = group_field(group, "name") else {
                // A group whose name cannot be read is a group whose progress is unknown, and trimming
                // on incomplete information is how unread entries get deleted.
                return Ok(0);
            };
            let needed = match self.oldest_needed_id(&mut conn, &key, &name).await {
                Some(id) => id,
                None => return Ok(0),
            };
            boundary = Some(match boundary {
                Some(current) if current <= needed => current,
                _ => needed,
            });
        }

        let Some(boundary) = boundary else {
            return Ok(0);
        };
        let trimmed: u64 = deadpool_redis::redis::cmd("XTRIM")
            .arg(&key)
            .arg("MINID")
            .arg(boundary.to_string())
            .query_async(&mut conn)
            .await
            .map_err(redis_error)?;

        if trimmed > 0 {
            // The refusal threshold reads this, so a trim has to update it or publishing stays refused
            // until the next append observes the shorter stream.
            if let Some(mut observed) = self.observed_backlog.get_mut(&key) {
                *observed = observed.saturating_sub(trimmed);
            }
            tracing::debug!(stream = %key, trimmed, boundary = %boundary, "Trimmed consumed stream entries");
        }
        Ok(trimmed)
    }

    async fn stream_subscribe(
        &self,
        topic: &str,
        group: &str,
        consumer: &str,
    ) -> Result<StreamSubscription, TopicError> {
        // Ensure consumer group exists
        self.ensure_consumer_group(topic, group).await?;

        let key = self.stream_key(topic);
        let group = group.to_string();
        let consumer = consumer.to_string();
        let pool = self.pool.clone();

        let stream = stream! {
            loop {
                // Get connection from pool
                let mut conn = match pool.get().await {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::warn!(error = %e, "Failed to get Redis connection, retrying...");
                        tokio::time::sleep(Duration::from_secs(1)).await;
                        continue;
                    }
                };

                // XREADGROUP with block
                let result: RedisResult<RedisValue> = deadpool_redis::redis::cmd("XREADGROUP")
                    .arg("GROUP")
                    .arg(&group)
                    .arg(&consumer)
                    .arg("BLOCK")
                    .arg(XREADGROUP_BLOCK_MS)
                    .arg("COUNT")
                    .arg(256)
                    .arg("STREAMS")
                    .arg(&key)
                    .arg(">")  // Only new messages
                    .query_async(&mut conn)
                    .await;

                match result {
                    Ok(RedisValue::Nil) => {
                        // Timeout, no messages, continue
                        continue;
                    }
                    Ok(value) => {
                        // Parse response: [[stream_name, [[id, [field, value, ...]]]]]
                        if let Some(messages) = parse_xreadgroup_response(value) {
                            for msg in messages {
                                yield Ok(msg);
                            }
                        }
                    }
                    Err(e) => {
                        let err_str = e.to_string();
                        if err_str.contains("NOGROUP") {
                            // Consumer group was lost (e.g. stream key recreated).
                            // Re-create it starting from ID 0 to consume all pending.
                            tracing::warn!("Consumer group lost, recreating from start...");
                            if let Ok(mut conn) = pool.get().await {
                                let result: RedisResult<String> = deadpool_redis::redis::cmd("XGROUP")
                                    .arg("CREATE")
                                    .arg(&key)
                                    .arg(&group)
                                    .arg("0") // From beginning to consume pending
                                    .arg("MKSTREAM")
                                    .query_async(&mut conn)
                                    .await;
                                if let Err(e) = result {
                                    tracing::error!(error = %e, "Failed to recreate consumer group");
                                    tokio::time::sleep(Duration::from_secs(1)).await;
                                    continue;
                                }
                            }
                            tokio::time::sleep(Duration::from_millis(100)).await;
                        } else {
                            tracing::warn!(error = %e, "XREADGROUP error, retrying...");
                            tokio::time::sleep(Duration::from_secs(1)).await;
                        }
                    }
                }
            }
        };

        Ok(StreamSubscription {
            receiver: Box::pin(stream),
        })
    }

    async fn stream_ack(&self, topic: &str, group: &str, id: &str) -> Result<(), TopicError> {
        let key = self.stream_key(topic);
        let mut conn = self.pool.get().await.map_err(pool_error)?;

        let _: i64 = deadpool_redis::redis::cmd("XACK")
            .arg(&key)
            .arg(group)
            .arg(id)
            .query_async(&mut conn)
            .await
            .map_err(redis_error)?;

        Ok(())
    }

    async fn stream_dead_letter(
        &self,
        topic: &str,
        group: &str,
        id: &str,
        reason: &str,
        payload: &[u8],
    ) -> Result<(), TopicError> {
        let key = self.stream_key(topic);
        let mut conn = self.pool.get().await.map_err(pool_error)?;
        self.dead_letter(&mut conn, &key, group, id, reason, payload)
            .await
    }

    async fn stream_ack_batch(
        &self,
        topic: &str,
        group: &str,
        ids: &[String],
    ) -> Result<(), TopicError> {
        if ids.is_empty() {
            return Ok(());
        }
        let key = self.stream_key(topic);
        let mut conn = self.pool.get().await.map_err(pool_error)?;

        let mut cmd = deadpool_redis::redis::cmd("XACK");
        cmd.arg(&key).arg(group);
        for id in ids {
            cmd.arg(id.as_str());
        }
        let _: i64 = cmd.query_async(&mut conn).await.map_err(redis_error)?;

        Ok(())
    }

    async fn stream_claim(
        &self,
        topic: &str,
        group: &str,
        consumer: &str,
        min_idle_ms: u64,
        count: usize,
    ) -> Result<Vec<StreamMessage>, TopicError> {
        let key = self.stream_key(topic);
        let mut conn = self.pool.get().await.map_err(pool_error)?;

        // Which pending entries this pass will claim, chosen by `scan_pending`: a rotating, bounded scan
        // that prefers the entries with the fewest deliveries. Nothing is discarded on the strength of a
        // retry counter - see [`CHRONIC_DELIVERY_REPORT`].
        let (ids_to_claim, cursor_action) = self
            .scan_pending(&mut conn, topic, &key, group, min_idle_ms, count)
            .await?;
        if ids_to_claim.is_empty() {
            // No claim to enable, so the rotation has done its reading; commit the (Reset) cursor.
            self.apply_cursor(&mut conn, cursor_action).await;
            return Ok(vec![]);
        }

        // XCLAIM the messages
        let mut cmd = deadpool_redis::redis::cmd("XCLAIM");
        cmd.arg(&key).arg(group).arg(consumer).arg(min_idle_ms);

        for id in &ids_to_claim {
            cmd.arg(id);
        }

        let claimed: RedisValue = cmd.query_async(&mut conn).await.map_err(redis_error)?;

        // Preserve unreadable payloads on `<stream>:dead` before acknowledging them. If preservation fails,
        // leave the entry pending so acknowledged data is never discarded.
        let mut messages = Vec::new();
        // Which requested ids this pass actually took, so the cursor cannot step over one it did not.
        //
        // `XCLAIM` succeeding does not mean it claimed everything asked for: it silently omits an entry
        // whose idle time no longer meets `min_idle_ms`, which a peer consumer resets simply by claiming it
        // first. That peer may then crash, leaving the entry abandoned - and if this pass had advanced past
        // it, a continuously-full pending list ahead of the cursor means it is never revisited.
        let mut handled: HashSet<String> = HashSet::new();
        if let RedisValue::Array(entries) = claimed {
            for entry in entries {
                let RedisValue::Array(parts) = entry else {
                    continue;
                };
                let id = parts.first().and_then(redis_string);
                let message = match parts.get(1) {
                    Some(RedisValue::Array(fields)) => extract_message_fields(fields),
                    _ => None,
                };
                match (id, message) {
                    (Some(id), Some((payload, partition))) => {
                        handled.insert(id.clone());
                        messages.push(StreamMessage {
                            id,
                            partition,
                            payload,
                        });
                    }
                    (Some(id), None) => {
                        let raw = format!("{:?}", parts.get(1));
                        match self
                            .dead_letter(
                                &mut conn,
                                &key,
                                group,
                                &id,
                                "unreadable_payload",
                                raw.as_bytes(),
                            )
                            .await
                        {
                            Ok(()) => {
                                tracing::error!(
                                    stream = %key,
                                    group,
                                    message_id = %id,
                                    "Moved a queued entry with no readable payload to the dead-letter stream; \
                                     leaving it pending would hold the stream's trim boundary forever"
                                );
                                // Handled only if the ACK *succeeded*. A dead-letter `XADD` that lands while
                                // the `XACK` fails leaves the entry still pending - still holding the trim
                                // boundary - so the cursor must not pass it; the next pass retries the ack.
                                // (A duplicate on the dead-letter stream is the deliberate trade: the bytes
                                // are preserved twice rather than lost, and ingestion is idempotent.)
                                match self.stream_ack(topic, group, &id).await {
                                    Ok(()) => {
                                        handled.insert(id.clone());
                                    }
                                    Err(e) => tracing::warn!(
                                        message_id = %id,
                                        error = %e,
                                        "Could not acknowledge a dead-lettered entry; it stays pending and the \
                                         scan cursor holds at it"
                                    ),
                                }
                            }
                            Err(e) => tracing::error!(
                                stream = %key,
                                group,
                                message_id = %id,
                                error = %e,
                                "Could not preserve an unreadable entry on the dead-letter stream; leaving it \
                                 pending rather than discarding it"
                            ),
                        }
                    }
                    // No id at all: nothing to acknowledge, and nothing that can be acted on.
                    (None, _) => tracing::warn!(
                        stream = %key,
                        group,
                        "A claimed entry had no readable id"
                    ),
                }
            }
        }

        // An entry this pass did not claim is reconsidered by the next fixed-endpoint rotation; no per-entry
        // hold can pin the cursor.
        let action = cursor_action;
        self.apply_cursor(&mut conn, action).await;

        Ok(messages)
    }

    async fn stream_stats(&self, topic: &str, group: &str) -> Result<StreamStats, TopicError> {
        let key = self.stream_key(topic);
        let mut conn = self.pool.get().await.map_err(pool_error)?;

        // XLEN for stream length
        let length: u64 = deadpool_redis::redis::cmd("XLEN")
            .arg(&key)
            .query_async(&mut conn)
            .await
            .unwrap_or(0);

        // XPENDING summary for pending info
        let pending_info: RedisValue = deadpool_redis::redis::cmd("XPENDING")
            .arg(&key)
            .arg(group)
            .query_async(&mut conn)
            .await
            .unwrap_or(RedisValue::Nil);

        let mut pending = 0u64;
        let mut consumers = 0u64;
        let mut oldest_pending_ms = None;

        if let RedisValue::Array(parts) = pending_info
            && parts.len() >= 4
        {
            // [pending_count, smallest_id, largest_id, [[consumer, count], ...]]
            if let RedisValue::Int(p) = &parts[0] {
                pending = *p as u64;
            }
            if let RedisValue::Array(consumer_list) = &parts[3] {
                consumers = consumer_list.len() as u64;
            }
        }

        // Get oldest pending message age
        if pending > 0 {
            let pending_detail: RedisValue = deadpool_redis::redis::cmd("XPENDING")
                .arg(&key)
                .arg(group)
                .arg("-")
                .arg("+")
                .arg(1)
                .query_async(&mut conn)
                .await
                .unwrap_or(RedisValue::Nil);

            if let RedisValue::Array(entries) = pending_detail
                && let Some(RedisValue::Array(parts)) = entries.first()
                && parts.len() >= 3
                && let RedisValue::Int(idle) = &parts[2]
            {
                oldest_pending_ms = Some(*idle as u64);
            }
        }

        Ok(StreamStats {
            length,
            pending,
            consumers,
            oldest_pending_ms,
        })
    }

    // =========================================================================
    // Health
    // =========================================================================

    async fn health_check(&self) -> Result<(), TopicError> {
        let mut conn = self
            .pool
            .get()
            .await
            .map_err(|e| TopicError::Connection(e.to_string()))?;

        deadpool_redis::redis::cmd("PING")
            .query_async::<String>(&mut conn)
            .await
            .map_err(|e| TopicError::Connection(e.to_string()))?;

        Ok(())
    }

    async fn shutdown(&self) {
        RedisTopicBackend::shutdown(self).await;
    }

    fn backend_name(&self) -> &'static str {
        "redis"
    }

    fn is_durable(&self) -> bool {
        // A Redis stream holds the message until it is acknowledged, and an unacknowledged one is
        // reclaimed by another consumer - which is what makes acknowledging before writing honest.
        //
        // Checked, not assumed: `probe_redis_durability` requires AOF with `appendfsync always` at startup,
        // and refuses an eviction policy that could delete an unread entry. `everysec` was accepted for a
        // while on the grounds that it is what production Redis runs - but it lets a 200 precede the fsync,
        // so a host failure loses up to a second of exports already reported as stored, and no amount of
        // documenting that makes the data durable. An operator who wants that throughput has the in-memory
        // backend, which writes inside the request rather than acknowledging early.
        //
        // One window remains and is not closed here: a failover can promote a replica that had not yet
        // received the entry. Closing it needs a per-publish `WAIT`/`WAITAOF` and the latency that implies,
        // and it is not something this probe can verify from a single connection.
        true
    }
}
