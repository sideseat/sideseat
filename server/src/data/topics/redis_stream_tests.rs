//! The durable ingestion queue, against a real Redis.
//!
//! # Why this suite exists
//!
//! The Redis stream backend is what makes an asynchronous 200 honest: a payload is acknowledged only
//! after it has been written, and an unacknowledged one is redelivered. Every claim in that sentence is
//! about Redis behaviour - consumer groups, pending entries, trimming - and none of it had ever run
//! against a Redis. The unit tests covered key prefixes and URL redaction.
//!
//! What that missed was the defect this suite now pins: publishing used `XADD ... MAXLEN ~ 100000`.
//! Redis trims by *length*, with no notion of whether an entry has been read, so any backlog past the
//! bound deleted the oldest payloads - each already answered 200 by HTTP or gRPC. A queue that discards
//! accepted work is worse than no queue, because the loss is silent.
//!
//! Skips with a message when `SIDESEAT_TEST_REDIS_URL` is unset, so `make check` stays green without
//! Docker; `make test-redis` starts a pinned container and sets it.

use std::sync::Arc;

use super::backend::TopicBackend;
use super::error::TopicError;
use super::redis::RedisTopicBackend;

const URL_ENV: &str = "SIDESEAT_TEST_REDIS_URL";

/// A backend on a scratch stream, with the stream removed first so a rerun starts clean.
async fn backend(topic: &str) -> Option<(Arc<RedisTopicBackend>, String)> {
    let Ok(url) = std::env::var(URL_ENV) else {
        eprintln!(
            "redis stream tests: skipped - set {URL_ENV} to a Redis URL (or run `make test-redis`)"
        );
        return None;
    };
    let backend = Arc::new(
        RedisTopicBackend::new(&url)
            .await
            .expect("connect to the test Redis"),
    );
    // A unique topic per test, so tests can run in parallel without sharing a stream.
    let topic = format!("{topic}-{}", uuid::Uuid::new_v4());
    Some((backend, topic))
}

/// Nothing unread is ever deleted, and a backlog at the limit is refused instead.
///
/// The old `MAXLEN ~ N` on `XADD` deleted the oldest entries to hold the length down. This drives the
/// stream past a deliberately tiny limit and requires that (a) every payload published successfully is
/// still readable afterwards, and (b) the publish that would have exceeded the limit failed, so the
/// exporter still holds its data. Those two together are the property; either alone is passable by a
/// backend that quietly drops.
#[tokio::test]
async fn a_full_backlog_is_refused_and_nothing_published_is_discarded() {
    let Some((backend, topic)) = backend("backlog").await else {
        return;
    };
    backend.set_max_backlog_for_test(8);

    let mut accepted = Vec::new();
    let mut refusals = 0;
    for i in 0..40u32 {
        match backend
            .stream_publish(&topic, format!("payload-{i}").as_bytes())
            .await
        {
            Ok(id) => accepted.push((id, i)),
            Err(TopicError::BufferFull) => refusals += 1,
            Err(e) => panic!("unexpected publish error: {e}"),
        }
    }

    assert!(
        refusals > 0,
        "a backlog well past the limit must be refused, not absorbed by deleting older entries"
    );
    assert!(
        accepted.len() >= 8,
        "the limit must be reached before it is enforced, or nothing is being queued at all: \
         accepted {} of 40",
        accepted.len()
    );

    // Every accepted payload is still there. `XRANGE` over the whole stream, because a trim would show
    // up as a missing prefix and the assertion has to name which one went.
    let present = backend
        .read_all_for_test(&topic)
        .await
        .expect("read the stream back");
    for (id, i) in &accepted {
        assert!(
            present.iter().any(|(entry_id, payload)| {
                entry_id == id && payload == format!("payload-{i}").as_bytes()
            }),
            "payload-{i} was published successfully and is gone: the queue discarded accepted work"
        );
    }
    assert_eq!(
        present.len(),
        accepted.len(),
        "the stream holds exactly what was accepted"
    );
}

/// A consumer catching up lets the stream shrink, and publishing resumes.
///
/// This is the other half of refusing: a bounded queue is only usable if the bound lifts. The
/// boundary is *progress*, so entries go only once every group has acknowledged them.
#[tokio::test]
async fn consuming_the_backlog_releases_the_refusal() {
    let Some((backend, topic)) = backend("release").await else {
        return;
    };
    backend.set_max_backlog_for_test(4);
    let group = "traces";

    // Fill until refused.
    let mut published = 0;
    while backend.stream_publish(&topic, b"payload").await.is_ok() {
        published += 1;
        assert!(published < 100, "the limit was never enforced");
    }
    assert!(published >= 4);

    // Nothing has been consumed, so nothing may be trimmed - not even by a group that exists but has
    // read nothing.
    backend
        .ensure_group_for_test(&topic, group)
        .await
        .expect("create the group");
    let trimmed = backend
        .stream_trim_consumed(&topic)
        .await
        .expect("trim with an idle group");
    assert_eq!(
        trimmed, 0,
        "a group that has read nothing still needs everything"
    );

    // Read and acknowledge the whole backlog.
    let mut subscription = backend
        .stream_subscribe(&topic, group, "consumer-1")
        .await
        .expect("subscribe");
    let mut acked = 0;
    while acked < published {
        let message = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            futures::StreamExt::next(&mut subscription.receiver),
        )
        .await
        .expect("a queued entry is delivered promptly")
        .expect("the subscription stays open")
        .expect("delivery succeeds");
        backend
            .stream_ack(&topic, group, &message.id)
            .await
            .expect("ack");
        acked += 1;
    }

    let trimmed = backend
        .stream_trim_consumed(&topic)
        .await
        .expect("trim after the backlog was acknowledged");
    assert_eq!(
        trimmed, published as u64,
        "everything acknowledged by every group may go, and nothing else"
    );

    // And the refusal lifts, without waiting for a later publish to observe the shorter stream.
    backend
        .stream_publish(&topic, b"after-the-drain")
        .await
        .expect("publishing resumes once the backlog is gone");
}

/// An entry delivered but not acknowledged is never trimmed, even after a consumer disappears.
///
/// This is the case a length bound gets wrong in the most damaging way: the payload is in flight, its
/// consumer died, `stream_claim` is about to hand it to someone else - and trimming by length would
/// delete it first. The boundary is the oldest *pending* entry precisely so this cannot happen.
#[tokio::test]
async fn an_unacknowledged_entry_is_never_trimmed_and_is_redelivered() {
    let Some((backend, topic)) = backend("pending").await else {
        return;
    };
    let group = "traces";
    backend
        .ensure_group_for_test(&topic, group)
        .await
        .expect("create the group");

    let first = backend
        .stream_publish(&topic, b"in-flight")
        .await
        .expect("publish");
    let second = backend
        .stream_publish(&topic, b"following")
        .await
        .expect("publish");

    // Deliver both to a consumer that then vanishes without acknowledging.
    let mut subscription = backend
        .stream_subscribe(&topic, group, "doomed")
        .await
        .expect("subscribe");
    for _ in 0..2 {
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            futures::StreamExt::next(&mut subscription.receiver),
        )
        .await
        .expect("delivered")
        .expect("open")
        .expect("ok");
    }
    drop(subscription);

    let trimmed = backend
        .stream_trim_consumed(&topic)
        .await
        .expect("trim with entries in flight");
    assert_eq!(
        trimmed, 0,
        "an entry that was delivered but not acknowledged is still owed"
    );

    // A different consumer claims them, which is what redelivery means here.
    let claimed = backend
        .stream_claim(&topic, group, "rescuer", 0, 10)
        .await
        .expect("claim the abandoned entries");
    let ids: Vec<&str> = claimed.iter().map(|m| m.id.as_str()).collect();
    assert!(
        ids.contains(&first.as_str()) && ids.contains(&second.as_str()),
        "both abandoned entries must be reclaimable: got {ids:?}"
    );

    for message in &claimed {
        backend
            .stream_ack(&topic, group, &message.id)
            .await
            .expect("ack");
    }
    assert_eq!(
        backend
            .stream_trim_consumed(&topic)
            .await
            .expect("trim after the rescue"),
        2,
        "once the rescuer acknowledged them they may go"
    );
}

/// The slowest consumer group sets the boundary.
///
/// Two groups read the same stream independently - which is the point of groups - so an entry one has
/// finished with is still owed to the other. Trimming on the fast group's progress would delete it.
#[tokio::test]
async fn the_slowest_group_decides_what_may_be_trimmed() {
    let Some((backend, topic)) = backend("groups").await else {
        return;
    };
    backend
        .ensure_group_for_test(&topic, "fast")
        .await
        .expect("group");
    backend
        .ensure_group_for_test(&topic, "slow")
        .await
        .expect("group");

    for i in 0..4u32 {
        backend
            .stream_publish(&topic, format!("payload-{i}").as_bytes())
            .await
            .expect("publish");
    }

    // The fast group consumes and acknowledges everything; the slow group reads nothing.
    let mut subscription = backend
        .stream_subscribe(&topic, "fast", "consumer")
        .await
        .expect("subscribe");
    for _ in 0..4 {
        let message = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            futures::StreamExt::next(&mut subscription.receiver),
        )
        .await
        .expect("delivered")
        .expect("open")
        .expect("ok");
        backend
            .stream_ack(&topic, "fast", &message.id)
            .await
            .expect("ack");
    }

    assert_eq!(
        backend
            .stream_trim_consumed(&topic)
            .await
            .expect("trim with one group behind"),
        0,
        "the slow group has read nothing, so nothing may go"
    );
    assert_eq!(
        backend
            .read_all_for_test(&topic)
            .await
            .expect("read back")
            .len(),
        4,
        "and every entry is still there for it to read"
    );
}

/// A Redis without persistence is refused at startup.
///
/// The defect this pins: `is_durable()` returned true unconditionally, so any PINGable Redis was
/// treated as a durable queue. In production that could be a cache-tier instance with AOF off and a
/// keyspace-wide LRU, both of which lose data an OTLP export was answered 200 for. Refusing at
/// startup means the operator learns the configuration is wrong before any exporter is fooled by it.
///
/// Skipped when Docker is unavailable; `make test-redis` sets its own AOF and eviction, so this test
/// spins up a second container with them off.
#[tokio::test]
async fn a_non_durable_redis_is_refused_at_startup() {
    if std::env::var(URL_ENV).is_err() {
        eprintln!("redis stream tests: skipped - set {URL_ENV}");
        return;
    }
    if std::process::Command::new("docker")
        .arg("--version")
        .output()
        .map(|o| !o.status.success())
        .unwrap_or(true)
    {
        eprintln!("a_non_durable_redis_is_refused_at_startup: skipped - docker unavailable");
        return;
    }

    let name = format!("sideseat-redis-nondurable-{}", uuid::Uuid::new_v4());
    let port = 63980u16;
    struct Cleanup(String);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::process::Command::new("docker")
                // : the redis image declares a volume, so removing the container without it leaves an
                // anonymous volume behind - which is how 246 of them accumulated before the make targets
                // were fixed the same way.
                .args(["rm", "-fv", &self.0])
                .output();
        }
    }
    let _cleanup = Cleanup(name.clone());
    let started = std::process::Command::new("docker")
        .args([
            "run",
            "-d",
            "--name",
            &name,
            "-p",
            &format!("{port}:6379"),
            "redis:7.4-alpine",
            "redis-server",
            "--maxmemory",
            "16mb",
            "--maxmemory-policy",
            "allkeys-lru",
            "--appendonly",
            "no",
        ])
        .output();
    assert!(
        started
            .as_ref()
            .map(|o| o.status.success())
            .unwrap_or(false),
        "failed to start the throwaway Redis: {started:?}"
    );
    // Give it a moment to accept connections.
    for _ in 0..30 {
        let ping = std::process::Command::new("docker")
            .args(["exec", &name, "redis-cli", "ping"])
            .output();
        if ping
            .map(|o| String::from_utf8_lossy(&o.stdout).contains("PONG"))
            .unwrap_or(false)
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    let result = RedisTopicBackend::new(&format!("redis://127.0.0.1:{port}")).await;
    let err = match result {
        Ok(_) => panic!("a Redis with AOF off and allkeys-lru must not be accepted as durable"),
        Err(e) => e,
    };
    let message = err.to_string();
    assert!(
        message.contains("appendonly") || message.contains("maxmemory-policy"),
        "the refusal must name the offending setting, so an operator knows what to fix: {message}"
    );
}

/// An abandoned entry behind a wall of fresh ones is still reclaimed.
///
/// `stream_claim` used to ask for the first `count` pending entries from the start of the list and then
/// discard the ones that were not idle enough - so a group with more than `count` recently-delivered
/// entries in front hid every abandoned entry behind them, and one at position `count + 1` was never
/// examined however long its consumer had been dead. Filtering with `XPENDING ... IDLE` makes the window
/// contain only eligible entries.
///
/// The test delivers a batch, acknowledges nothing, then asks for a claim window *smaller* than the
/// backlog - which is the shape that produced the starvation.
#[tokio::test]
async fn an_abandoned_entry_behind_fresh_ones_is_still_reclaimed() {
    let Some((backend, topic)) = backend("starvation").await else {
        return;
    };
    let group = "traces";
    backend
        .ensure_group_for_test(&topic, group)
        .await
        .expect("group");

    // One entry delivered and abandoned first, so it is the oldest and the most idle.
    let abandoned = backend
        .stream_publish(&topic, b"abandoned")
        .await
        .expect("publish");
    let mut doomed = backend
        .stream_subscribe(&topic, group, "doomed")
        .await
        .expect("subscribe");
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        futures::StreamExt::next(&mut doomed.receiver),
    )
    .await
    .expect("delivered")
    .expect("open")
    .expect("ok");
    drop(doomed);

    // Then a wall of entries delivered to a *live* consumer and left pending. Under the old window these
    // would fill the first `count` slots and hide the abandoned one.
    for i in 0..10u32 {
        backend
            .stream_publish(&topic, format!("fresh-{i}").as_bytes())
            .await
            .expect("publish");
    }
    let mut live = backend
        .stream_subscribe(&topic, group, "live")
        .await
        .expect("subscribe");
    for _ in 0..10 {
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            futures::StreamExt::next(&mut live.receiver),
        )
        .await
        .expect("delivered")
        .expect("open")
        .expect("ok");
    }

    // A window of two, against a pending list of eleven. `min_idle_ms` of 0 makes everything eligible, so
    // the test instead relies on the *order*: with IDLE filtering the oldest eligible entries come first,
    // and the abandoned one is the oldest.
    let claimed = backend
        .stream_claim(&topic, group, "rescuer", 0, 2)
        .await
        .expect("claim a small window");
    let ids: Vec<&str> = claimed.iter().map(|m| m.id.as_str()).collect();
    assert!(
        ids.contains(&abandoned.as_str()),
        "the oldest abandoned entry must be reachable through a window smaller than the backlog: \
         got {ids:?}"
    );
}

/// An entry that fails every delivery is retried behind fresher work, and is never discarded.
///
/// Claiming resets an entry's idle time, so an entry that fails each time becomes eligible again after the
/// idle threshold and - being the oldest - refills the recovery window. With a window of N and N such
/// entries, nothing behind them is ever examined.
///
/// The fix must not be to acknowledge it. A delivery counter is evidence about the *system* that keeps
/// failing to store the payload, not about the payload: ten failures is what a minute of analytics downtime
/// looks like, and the payload was already answered 200. So this asserts both halves - the chronic entry
/// survives every attempt, *and* the entry behind it is reached anyway.
#[tokio::test]
async fn a_chronically_failing_entry_never_starves_the_others_and_is_never_discarded() {
    let Some((backend, topic)) = backend("poison").await else {
        return;
    };
    let group = "traces";
    backend
        .ensure_group_for_test(&topic, group)
        .await
        .expect("group");

    let poison = backend
        .stream_publish(&topic, b"poison")
        .await
        .expect("publish");
    let behind = backend
        .stream_publish(&topic, b"behind")
        .await
        .expect("publish");

    // Deliver both and abandon them, so both are pending and eligible.
    let mut subscription = backend
        .stream_subscribe(&topic, group, "doomed")
        .await
        .expect("subscribe");
    for _ in 0..2 {
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            futures::StreamExt::next(&mut subscription.receiver),
        )
        .await
        .expect("delivered")
        .expect("open")
        .expect("ok");
    }
    drop(subscription);

    // Claim repeatedly without acknowledging, as a consumer that keeps failing does. A window of one is what
    // makes the starvation visible: only one entry can be handed out per pass, so if the chronic entry always
    // won, the one behind it would never appear.
    let mut saw_poison = 0;
    let mut saw_behind = 0;
    for _ in 0..15 {
        let claimed = backend
            .stream_claim(&topic, group, "retrier", 0, 1)
            .await
            .expect("claim");
        if claimed.iter().any(|m| m.id == poison) {
            saw_poison += 1;
        }
        if claimed.iter().any(|m| m.id == behind) {
            saw_behind += 1;
        }
    }
    assert!(
        saw_poison > 0,
        "the failing entry must have been retried at all, or the test proves nothing"
    );
    assert!(
        saw_behind > 0,
        "the entry behind a chronically-failing one must still be reached through a window of one"
    );

    // Both are still pending: nothing was discarded on the strength of a retry counter, and each payload was
    // answered 200 by the ingest that queued it.
    let stats = backend
        .stream_stats(&topic, group)
        .await
        .expect("stream stats");
    assert_eq!(
        stats.pending, 2,
        "an entry that keeps failing must be kept, not acknowledged: pending={}",
        stats.pending
    );

    // And it is still claimable, so retrying it continues rather than stopping at a limit.
    let mut still_offered = false;
    for _ in 0..12 {
        let claimed = backend
            .stream_claim(&topic, group, "retrier", 0, 2)
            .await
            .expect("claim a wider window");
        if claimed.iter().any(|m| m.id == poison) {
            still_offered = true;
            break;
        }
    }
    assert!(
        still_offered,
        "a chronically-failing entry must keep being retried, not be dropped after a fixed count"
    );
}

/// An entry beyond one scan window is still reached, however many failing entries sit in front of it.
///
/// This is the half that sorting cannot do. Preferring the fewest deliveries only orders what was *scanned*,
/// and a scan is bounded - so with a full window of equally-failing entries at the front of the pending list,
/// the entry behind them is not merely deprioritised, it is invisible. A rotating start is what makes the
/// whole pending list reachable in a bounded number of passes, which is what lets a chronic failure be kept
/// rather than acknowledged.
#[tokio::test]
async fn an_entry_beyond_one_scan_window_is_still_reached() {
    let Some((backend, topic)) = backend("rotation").await else {
        return;
    };
    let group = "traces";
    backend
        .ensure_group_for_test(&topic, group)
        .await
        .expect("group");

    // More entries than one window holds. The window is `count * PENDING_SCAN_FACTOR`, so with a claim of
    // one the last of these lies past it and can only be found by rotating.
    let mut ids = Vec::new();
    for i in 0..12u32 {
        ids.push(
            backend
                .stream_publish(&topic, format!("entry-{i}").as_bytes())
                .await
                .expect("publish"),
        );
    }
    let last = ids.last().expect("published").clone();

    // Deliver all of them and abandon them, so the whole list is pending and equally eligible - equal
    // delivery counts, which is what removes sorting as a way through.
    let mut subscription = backend
        .stream_subscribe(&topic, group, "doomed")
        .await
        .expect("subscribe");
    for _ in 0..ids.len() {
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            futures::StreamExt::next(&mut subscription.receiver),
        )
        .await
        .expect("delivered")
        .expect("open")
        .expect("ok");
    }
    drop(subscription);

    // Claim one at a time and never acknowledge, as a consumer failing on every entry does.
    let mut saw_last = false;
    for _ in 0..24 {
        let claimed = backend
            .stream_claim(&topic, group, "retrier", 0, 1)
            .await
            .expect("claim");
        if claimed.iter().any(|m| m.id == last) {
            saw_last = true;
            break;
        }
    }
    assert!(
        saw_last,
        "the entry past one scan window must be reached by rotation, not hidden behind the front of the \
         pending list"
    );

    // And nothing was discarded to achieve it.
    let stats = backend
        .stream_stats(&topic, group)
        .await
        .expect("stream stats");
    assert_eq!(
        stats.pending as usize,
        ids.len(),
        "reaching a later entry must not cost an earlier one"
    );
}

/// An entry whose payload cannot be read is preserved on the dead-letter stream before it is acknowledged.
///
/// This one *is* evidence about the entry itself: nothing can ever process a payload that cannot be read, and
/// leaving it pending holds the trim boundary forever, so it has to leave the main stream. It was also
/// answered 200, so its bytes must survive somewhere an operator can find them - which is the difference
/// between dead-lettering and the deletion this replaced.
#[tokio::test]
async fn an_unreadable_entry_is_preserved_before_it_is_acknowledged() {
    let Some((backend, topic)) = backend("deadletter").await else {
        return;
    };
    let group = "traces";
    backend
        .ensure_group_for_test(&topic, group)
        .await
        .expect("group");

    // An entry with no `payload` field: the shape nothing downstream can act on.
    let id = backend
        .publish_raw_field_for_test(&topic, "not_payload", b"unreadable")
        .await
        .expect("publish a fieldless entry");

    // Deliver it so it becomes pending, then abandon it.
    let mut subscription = backend
        .stream_subscribe(&topic, group, "doomed")
        .await
        .expect("subscribe");
    let _ = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        futures::StreamExt::next(&mut subscription.receiver),
    )
    .await;
    drop(subscription);

    // Claiming it finds the payload unreadable, so it is dead-lettered and acknowledged.
    let claimed = backend
        .stream_claim(&topic, group, "rescuer", 0, 4)
        .await
        .expect("claim");
    assert!(
        !claimed.iter().any(|m| m.id == id),
        "an unreadable entry must not be handed to a consumer"
    );

    let dead = backend
        .dead_letter_entries_for_test(&topic)
        .await
        .expect("read the dead-letter stream");
    assert!(
        dead.iter().any(|entry| entry.contains(&id)),
        "the unreadable entry's bytes must be preserved on the dead-letter stream, not deleted: {dead:?}"
    );

    let stats = backend
        .stream_stats(&topic, group)
        .await
        .expect("stream stats");
    assert_eq!(
        stats.pending, 0,
        "once preserved, the entry must be acknowledged so it stops holding the trim boundary"
    );
}

/// A required replica acknowledgement is refused against a Redis with no replica to fsync it.
///
/// The point of `min_replica_acks` is that a 200 survives a failover, and `WAITAOF` is what makes that
/// honest: it blocks on the append-only file being fsynced locally *and* on that many replicas. The test
/// Redis is standalone, so a publish demanding one replica ack must fail rather than report success - the
/// exact false-success `WAIT` (in-memory receipt only) could give, and the one this refusal removes. Local
/// fsync alone is not enough, because that is what the *non*-replicated durability already promises.
#[tokio::test]
async fn a_required_replica_ack_is_refused_without_a_replica() {
    let Ok(url) = std::env::var(URL_ENV) else {
        eprintln!("redis stream tests: skipped - set {URL_ENV} (or run `make test-redis`)");
        return;
    };
    // One replica required, against a standalone Redis: WAITAOF can fsync locally but never reach a replica.
    let backend = RedisTopicBackend::with_replica_acks(&url, 1)
        .await
        .expect("connect to the test Redis");
    let topic = format!("waitaof-{}", uuid::Uuid::new_v4());

    let result = backend.stream_publish(&topic, b"needs a replica").await;
    match result {
        Err(TopicError::Stream(msg)) => assert!(
            msg.contains("replica"),
            "the refusal must name the replica shortfall, got: {msg}"
        ),
        other => panic!(
            "a publish demanding a replica ack must be refused on a standalone Redis, got: {other:?}"
        ),
    }
}

/// `stream_dead_letter` preserves a payload on `<stream>:dead` for the caller to ack afterward.
///
/// This is the path the pipeline uses for a payload that decoded as a stream entry but not as the trace
/// export it should be - a corrupt or version-incompatible protobuf. `stream_claim`'s structural
/// dead-lettering never sees it (the entry's fields are readable), so without this the accepted bytes would
/// be acked away silently.
#[tokio::test]
async fn a_decodable_but_invalid_payload_can_be_dead_lettered() {
    let Some((backend, topic)) = backend("dead-invalid").await else {
        return;
    };
    let group = "traces";
    backend
        .ensure_group_for_test(&topic, group)
        .await
        .expect("group");

    let id = backend
        .stream_publish(&topic, b"not-a-valid-protobuf")
        .await
        .expect("publish");

    backend
        .stream_dead_letter(
            &topic,
            group,
            &id,
            "invalid_protobuf",
            b"not-a-valid-protobuf",
        )
        .await
        .expect("dead-letter the payload");

    let dead = backend
        .dead_letter_entries_for_test(&topic)
        .await
        .expect("read dead-letter stream");
    assert!(
        dead.iter().any(|e| e.contains("invalid_protobuf")),
        "the payload must be preserved with its reason on the dead-letter stream: {dead:?}"
    );
    let _ = id;
}

/// Rotation survives a process restart, because the cursor is in Redis rather than in the process.
///
/// The ephemeral-instance case: as a process-local map the cursor reset to the front on every restart, so an
/// instance replaced faster than a full rotation always re-scanned the same prefix and entries behind a
/// chronically-failing run starved indefinitely. Replicas also each kept their own position, so there was no
/// global rotation at all. A *fresh backend on the same Redis* stands in for a restarted instance here.
#[tokio::test]
async fn the_scan_cursor_survives_a_restart() {
    let Some((backend, topic)) = backend("cursor-restart").await else {
        return;
    };
    let group = "traces";
    backend
        .ensure_group_for_test(&topic, group)
        .await
        .expect("group");

    // A pending list longer than one claim window, all abandoned and equally eligible.
    let mut ids = Vec::new();
    for i in 0..6u32 {
        ids.push(
            backend
                .stream_publish(&topic, format!("entry-{i}").as_bytes())
                .await
                .expect("publish"),
        );
    }
    let mut subscription = backend
        .stream_subscribe(&topic, group, "doomed")
        .await
        .expect("subscribe");
    for _ in 0..ids.len() {
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            futures::StreamExt::next(&mut subscription.receiver),
        )
        .await
        .expect("delivered")
        .expect("open")
        .expect("ok");
    }
    drop(subscription);

    // One pass on the first instance, claiming two - the cursor should now sit past them.
    let first = backend
        .stream_claim(&topic, group, "before-restart", 0, 2)
        .await
        .expect("claim");
    assert_eq!(
        first.len(),
        2,
        "the first pass should claim its whole window"
    );

    // A brand-new backend against the same Redis: a restarted instance, with an empty process.
    let url = std::env::var(URL_ENV).expect("checked above");
    let restarted = std::sync::Arc::new(
        RedisTopicBackend::new(&url)
            .await
            .expect("reconnect after restart"),
    );
    let after = restarted
        .stream_claim(&topic, group, "after-restart", 0, 2)
        .await
        .expect("claim after restart");

    let first_ids: Vec<&str> = first.iter().map(|m| m.id.as_str()).collect();
    let after_ids: Vec<&str> = after.iter().map(|m| m.id.as_str()).collect();
    assert!(
        !after_ids.iter().any(|id| first_ids.contains(id)),
        "a restarted instance must resume past what the previous pass took, not re-scan from the front: \
         first={first_ids:?} after={after_ids:?}"
    );
}

/// A stalled replica's cursor write is refused once another replica has moved it, so nothing is skipped.
///
/// The interleaving a plain `SET` allowed: replica A claims a tail page and stalls before writing; replica B
/// finds the list exhausted past its own position, wraps, claims the front and advances the cursor there;
/// A's delayed write then jumps the cursor back out to its tail position, past the front entries B did not
/// claim, and with sustained traffic beyond that point they starve. Conditioning the write on the value the
/// scan read makes A's stale update a no-op - A loses only its own progress.
///
/// Driven at the Redis level, because the race is in the cursor write rather than in the claim: two backends
/// share one cursor key, so a write conditioned on a stale read must not land.
#[tokio::test]
async fn a_stale_cursor_write_is_refused() {
    let Some((backend, topic)) = backend("cursor-cas").await else {
        return;
    };
    let group = "traces";
    backend
        .ensure_group_for_test(&topic, group)
        .await
        .expect("group");

    // Both instances read the same (absent) cursor, then one writes.
    let observed = backend
        .read_scan_cursor_for_test(&topic, group)
        .await
        .expect("read cursor");
    assert!(observed.is_none(), "a fresh group has no cursor");

    backend
        .write_scan_cursor_for_test(&topic, group, observed.clone(), Some("50-0"))
        .await
        .expect("the first write applies");
    // Stored as `<position>|<end>`, so the position is what identifies it.
    let after_first = backend
        .read_scan_cursor_for_test(&topic, group)
        .await
        .expect("read")
        .expect("a cursor was written");
    assert!(
        after_first.starts_with("50-0|"),
        "the first write should have set position 50-0, got {after_first}"
    );

    // The stalled instance now writes, still expecting the value it read before (absent). It must be refused.
    backend
        .write_scan_cursor_for_test(&topic, group, observed, Some("900-0"))
        .await
        .expect("the stale write must not error, only be refused");
    assert_eq!(
        backend
            .read_scan_cursor_for_test(&topic, group)
            .await
            .expect("read"),
        Some(after_first),
        "a write conditioned on a stale read must not move the cursor - it would skip everything between"
    );
}

/// Rotation reaches later entries even while a peer keeps re-claiming the first one.
///
/// The failure the per-entry hold produced, and the reason it was removed: holding the cursor at an entry that
/// was scanned but not claimed lets a peer that repeatedly claims and abandons that entry keep it perpetually
/// too fresh, and everything after it starves. Rotation over a fixed endpoint cannot be pinned that way - the
/// sweep advances over everything it examines.
///
/// The adversary is a **direct `XCLAIM` of the first id**, not another rotating claim: the rotating one would
/// simply advance like any consumer, so the earlier version of this test staged no adversary at all.
#[tokio::test]
async fn rotation_reaches_later_entries_past_a_repeatedly_reclaimed_one() {
    let Some((backend, topic)) = backend("rotation-past").await else {
        return;
    };
    let group = "traces";
    backend
        .ensure_group_for_test(&topic, group)
        .await
        .expect("group");

    let mut ids = Vec::new();
    for i in 0..4u32 {
        ids.push(
            backend
                .stream_publish(&topic, format!("entry-{i}").as_bytes())
                .await
                .expect("publish"),
        );
    }
    let mut subscription = backend
        .stream_subscribe(&topic, group, "doomed")
        .await
        .expect("subscribe");
    for _ in 0..ids.len() {
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            futures::StreamExt::next(&mut subscription.receiver),
        )
        .await
        .expect("delivered")
        .expect("open")
        .expect("ok");
    }
    drop(subscription);

    let pinned = ids[0].clone();
    let last = ids[ids.len() - 1].clone();

    // Each pass: the peer re-claims the first entry directly (as a crash-loop would, resetting its idle
    // time and its ownership), then this consumer takes a window of one.
    let mut saw_last = false;
    for _ in 0..12 {
        backend
            .claim_specific_for_test(&topic, group, "flapping-peer", &pinned)
            .await
            .expect("peer re-claims the first entry");
        let claimed = backend
            .stream_claim(&topic, group, "rescuer", 0, 1)
            .await
            .expect("claim");
        if claimed.iter().any(|m| m.id == last) {
            saw_last = true;
            break;
        }
    }
    assert!(
        saw_last,
        "rotation must reach the last entry while a peer keeps re-claiming {pinned}; a cursor pinned there \
         would starve everything after it"
    );
}

/// A malformed cursor value is replaced, not treated as an immovable obstacle.
///
/// The empty string was the sharp case: absence and "holds an empty value" were both encoded as the CAS
/// absence sentinel, so a key holding `""` parsed as malformed (start a fresh rotation) but could never be
/// written - the CAS demanded the key be absent, which it never was. Every pass then rescanned the first page
/// and everything behind it starved. Absence is an explicit flag now.
#[tokio::test]
async fn a_malformed_cursor_value_is_replaced() {
    let Some((backend, topic)) = backend("cursor-malformed").await else {
        return;
    };
    let group = "traces";
    backend
        .ensure_group_for_test(&topic, group)
        .await
        .expect("group");

    // More entries than one page, so a stuck cursor would visibly starve the tail.
    let mut ids = Vec::new();
    for i in 0..4u32 {
        ids.push(
            backend
                .stream_publish(&topic, format!("entry-{i}").as_bytes())
                .await
                .expect("publish"),
        );
    }
    let mut subscription = backend
        .stream_subscribe(&topic, group, "doomed")
        .await
        .expect("subscribe");
    for _ in 0..ids.len() {
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            futures::StreamExt::next(&mut subscription.receiver),
        )
        .await
        .expect("delivered")
        .expect("open")
        .expect("ok");
    }
    drop(subscription);

    for malformed in ["", "not-a-rotation", "50-0", "|", "50-0|"] {
        backend
            .set_raw_scan_cursor_for_test(&topic, group, malformed)
            .await
            .expect("stage the malformed cursor");

        // One pass with a window of one: it must both do work and move the cursor off the bad value.
        let claimed = backend
            .stream_claim(&topic, group, "rescuer", 0, 1)
            .await
            .expect("claim");
        assert_eq!(
            claimed.len(),
            1,
            "a pass starting from a malformed cursor ({malformed:?}) must still claim"
        );
        let after = backend
            .read_scan_cursor_for_test(&topic, group)
            .await
            .expect("read cursor");
        assert_ne!(
            after.as_deref(),
            Some(malformed),
            "the malformed cursor {malformed:?} must be replaced, or rotation is stuck on it forever"
        );
    }
}
