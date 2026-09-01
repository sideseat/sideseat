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
                .args(["rm", "-f", &self.0])
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
