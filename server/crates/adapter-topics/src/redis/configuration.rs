use sideseat_ports::queue::TopicError;

/// Sanitize Redis URL for logging (removes password)
pub(super) fn sanitize_redis_url(url: &str) -> String {
    if let Some(at_pos) = url.rfind('@') {
        let scheme_end = url.find("://").map(|i| i + 3).unwrap_or(0);
        if let Some(colon_pos) = url[scheme_end..at_pos].find(':') {
            let abs_colon = scheme_end + colon_pos;
            let prefix = &url[..abs_colon + 1];
            let suffix = &url[at_pos..];
            return format!("{prefix}***{suffix}");
        }
    }
    url.to_string()
}

/// How many replicas the server currently has, from `INFO replication`.
///
/// Read only to *warn*: an operator whose Redis has replicas and requires no acknowledgement has a gap that
/// is invisible until a failover, and a startup line is the last moment anyone is looking.
pub(super) async fn connected_replicas(
    conn: &mut deadpool_redis::Connection,
) -> Result<u32, TopicError> {
    let info: String = deadpool_redis::redis::cmd("INFO")
        .arg("replication")
        .query_async(conn)
        .await
        .map_err(|e| TopicError::Connection(format!("INFO replication failed: {e}")))?;
    for line in info.lines() {
        if let Some(value) = line.trim().strip_prefix("connected_slaves:") {
            return Ok(value.trim().parse().unwrap_or(0));
        }
    }
    Ok(0)
}

/// Persistence and eviction settings that make the queue's durability promise honest.
///
/// Returns `Ok(())` when the server is configured for at least "one-second data loss on host failure"
/// (AOF on with `everysec` or `always`), and refuses when memory pressure could evict a queue entry.
///
/// # Why these are the right knobs to check, and not others
///
/// `PING` says the server is *reachable*, not that it is durable. The two properties we depend on:
///
/// * **AOF.** RDB alone is a periodic snapshot; a crash between snapshots loses everything since. AOF
///   `everysec` bounds that to one second, which is the minimum for a queue whose 200 has to mean
///   "durably stored". `always` is stricter.
/// * **Eviction.** A stream is a key, and an LRU/LFU policy over the whole keyspace can evict it under
///   memory pressure - which is exactly when a queue is most likely to be under pressure. `noeviction`
///   refuses new writes instead, which the OTLP path already handles as backpressure. The `-with-ttl`
///   variants confine eviction to keys with an explicit TTL, and streams here have none.
pub(super) async fn probe_redis_durability(
    conn: &mut deadpool_redis::Connection,
) -> Result<(), TopicError> {
    async fn read_config(
        conn: &mut deadpool_redis::Connection,
        field: &'static str,
    ) -> Result<Option<String>, TopicError> {
        let value: Vec<String> = deadpool_redis::redis::cmd("CONFIG")
            .arg("GET")
            .arg(field)
            .query_async(conn)
            .await
            .map_err(|e| TopicError::Connection(format!("CONFIG GET {field} failed: {e}")))?;
        // Ignore the fetched key and read only the value slot. When the server refuses `CONFIG` (some
        // managed Redis deployments do), we get an empty reply - the caller then decides whether that
        // is fatal, which is a per-field question.
        Ok(value.get(1).cloned())
    }

    let appendonly = read_config(conn, "appendonly").await?;
    let appendfsync = read_config(conn, "appendfsync").await?;
    let policy = read_config(conn, "maxmemory-policy").await?;

    if appendonly.is_none() && appendfsync.is_none() && policy.is_none() {
        // The server refuses `CONFIG` entirely - a managed offering, typically. We cannot verify, so we
        // decline to *claim* durability rather than pretend to have checked. The stream backend still
        // works; `is_durable()` will read this decision.
        tracing::warn!(
            "Redis CONFIG is not readable; treating the backend as non-durable. Set the persistence \
             and eviction settings out-of-band, and pin them via your managed provider's controls."
        );
        return Err(TopicError::Connection(
            "cannot verify AOF / eviction settings via CONFIG GET".to_string(),
        ));
    }

    let ao = appendonly.unwrap_or_default().to_ascii_lowercase();
    if ao != "yes" {
        return Err(TopicError::Connection(format!(
            "appendonly is {ao:?}; AOF must be enabled for a durable queue"
        )));
    }
    // `always`, not `everysec`.
    //
    // The queue's whole purpose is that a 200 means the data is stored. With `everysec` a 200 can precede
    // the next fsync, so a host failure loses up to a second of *acknowledged* exports - and documenting
    // that window makes the loss honest without making the data durable, which is not the promise. An
    // operator who wants the throughput of `everysec` has the in-memory topic backend, where the request
    // writes to the analytics store before answering and nothing is acknowledged early.
    let fs = appendfsync.unwrap_or_default().to_ascii_lowercase();
    if fs != "always" {
        return Err(TopicError::Connection(format!(
            "appendfsync is {fs:?}; a queue that acknowledges before the write needs `always`. With \
             `everysec` a 200 can precede the fsync, so a host failure loses up to a second of exports \
             this server has already reported as stored. Use the default in-memory topic backend if you \
             prefer that throughput - it writes inside the request instead of acknowledging early."
        )));
    }

    let ev = policy.unwrap_or_default().to_ascii_lowercase();
    let safe = matches!(
        ev.as_str(),
        "noeviction" | "volatile-lru" | "volatile-lfu" | "volatile-random" | "volatile-ttl"
    );
    if !safe {
        return Err(TopicError::Connection(format!(
            "maxmemory-policy is {ev:?}; a keyspace-wide LRU/LFU can evict a stream entry that has \
             been answered 200. Use `noeviction` or one of the `volatile-*` variants."
        )));
    }
    Ok(())
}
