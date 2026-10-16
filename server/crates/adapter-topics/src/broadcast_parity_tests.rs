//! Broadcast delivery on every backend, on the channels the server really uses.
//!
//! SSE, presence, agent invocation and connection control each publish on a channel of a fixed kind, keyed by a
//! project, a request or an instance. A backend must deliver each channel to its subscribers, keep two keys apart -
//! also when they share a Redpanda partition - and give a new subscriber no history. Redpanda once refused every
//! channel name (`InvalidTopic`), once replayed a topic's history to each new viewer, and once created a topic per
//! key; no test covered broadcast outside the in-process backend. The memory backend always runs; Redis and Redpanda
//! run when `SIDESEAT_TEST_REDIS_URL` or `SIDESEAT_TEST_REDPANDA_BROKERS` is set (`make test-redis`,
//! `make test-redpanda`).

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use rdkafka::config::ClientConfig;
use rdkafka::consumer::{BaseConsumer, Consumer};
use sideseat_core::config::RedpandaConfig;
use sideseat_ports::queue::broadcast_kinds::{
    AGENT_REQUEST, CONNECTION_CONTROL, PRESENCE, SSE_SPANS,
};
use sideseat_ports::queue::{BroadcastChannel, BroadcastSubscription, TopicBackend};

use crate::memory::MemoryTopicBackend;
use crate::redis::RedisTopicBackend;
use crate::redpanda::{RedpandaTopicBackend, broadcast_partition, kafka_topic};

const KINDS: [&str; 4] = [SSE_SPANS, PRESENCE, AGENT_REQUEST, CONNECTION_CONTROL];

/// A key unique to this run, so reruns against a shared broker never see an earlier run's messages.
fn fresh_key(tag: &str) -> String {
    format!("{tag}{}", uuid::Uuid::new_v4().simple())
}

/// A channel of each kind the server publishes on, with keys shaped as the server's are.
fn server_channels() -> Vec<BroadcastChannel> {
    vec![
        BroadcastChannel::new(SSE_SPANS, fresh_key("p")),
        BroadcastChannel::new(PRESENCE, fresh_key("p")),
        BroadcastChannel::new(AGENT_REQUEST, uuid::Uuid::new_v4().to_string()),
        BroadcastChannel::new(CONNECTION_CONTROL, fresh_key("instance-")),
    ]
}

/// Two keys of `kind` that Redpanda puts on one partition, so only the key match keeps them apart.
fn colliding_channels(kind: &'static str) -> (BroadcastChannel, BroadcastChannel) {
    let first = BroadcastChannel::new(kind, fresh_key("a"));
    loop {
        let second = BroadcastChannel::new(kind, fresh_key("b"));
        if broadcast_partition(&second) == broadcast_partition(&first) {
            return (first, second);
        }
    }
}

/// The next message, or `None` if none arrives within `wait`.
async fn next_within(subscription: &mut BroadcastSubscription, wait: Duration) -> Option<Vec<u8>> {
    tokio::time::timeout(wait, subscription.receiver.next())
        .await
        .ok()
        .flatten()
        .map(|message| message.expect("receive broadcast"))
}

/// Publish until the subscriber sees the message: a subscription can need a moment before it receives (a Redis
/// bridge subscribes), and broadcast promises delivery only to a live subscriber.
async fn publish_until_received(
    backend: &dyn TopicBackend,
    channel: &BroadcastChannel,
    subscription: &mut BroadcastSubscription,
    payload: &[u8],
) {
    for _ in 0..60 {
        backend
            .publish(channel, payload)
            .await
            .expect("publish broadcast");
        if let Some(message) = next_within(subscription, Duration::from_millis(500)).await {
            assert_eq!(
                message, payload,
                "{channel:?}: delivered a different message"
            );
            return;
        }
    }
    panic!("{channel:?}: nothing published there reached its subscriber in 30 s");
}

/// A message on `first` never reaches `second`'s subscriber.
async fn assert_apart(
    backend: &dyn TopicBackend,
    first: &BroadcastChannel,
    second: &BroadcastChannel,
) {
    let mut first_viewer = backend.subscribe(first).await.expect("subscribe");
    let mut second_viewer = backend.subscribe(second).await.expect("subscribe");
    publish_until_received(backend, second, &mut second_viewer, b"warm").await;
    publish_until_received(backend, first, &mut first_viewer, b"first").await;
    // Drain what the warm-up loop may have published more than once, then check nothing of the first channel's
    // came through.
    while let Some(message) = next_within(&mut second_viewer, Duration::from_secs(2)).await {
        assert_eq!(
            message, b"warm",
            "{second:?} received a message published on {first:?}"
        );
    }
}

/// Every server channel is delivered; two keys never mix, also when Redpanda puts them on one partition.
async fn assert_server_channels_deliver_and_stay_apart(backend: Arc<dyn TopicBackend>) {
    for channel in server_channels() {
        let mut subscription = backend.subscribe(&channel).await.expect("subscribe");
        publish_until_received(
            backend.as_ref(),
            &channel,
            &mut subscription,
            channel.key().as_bytes(),
        )
        .await;
    }
    let first = BroadcastChannel::new(SSE_SPANS, fresh_key("a"));
    let second = BroadcastChannel::new(SSE_SPANS, fresh_key("b"));
    assert_apart(backend.as_ref(), &first, &second).await;
    let (first, second) = colliding_channels(SSE_SPANS);
    assert_apart(backend.as_ref(), &first, &second).await;
    // The same key under another kind is another channel.
    let presence = BroadcastChannel::new(PRESENCE, first.key());
    assert_apart(backend.as_ref(), &first, &presence).await;
}

/// A subscriber receives what is published after it subscribes, and nothing published before: a viewer opening a
/// live stream must not first replay the channel's history.
async fn assert_a_new_subscriber_sees_no_history(backend: Arc<dyn TopicBackend>) {
    let channel = BroadcastChannel::new(SSE_SPANS, fresh_key("h"));
    // An earlier viewer keeps the channel live, as one would in production, while the history is published.
    let mut earlier = backend.subscribe(&channel).await.expect("subscribe");
    publish_until_received(backend.as_ref(), &channel, &mut earlier, b"history").await;
    for _ in 0..20 {
        backend
            .publish(&channel, b"history")
            .await
            .expect("publish history");
    }
    // Delivery can be asynchronous (Redis Pub/Sub reaches the process through a bridge task), so a message still
    // in flight when a viewer subscribes may reach it. That is not history. Once the earlier viewer has all twenty,
    // none is in flight, and anything the new viewer receives of them would be a replay.
    for received in 0..20 {
        let message = next_within(&mut earlier, Duration::from_secs(15)).await;
        assert_eq!(
            message.as_deref(),
            Some(&b"history"[..]),
            "history message {received} never arrived"
        );
    }
    let mut viewer = backend.subscribe(&channel).await.expect("subscribe");
    publish_until_received(backend.as_ref(), &channel, &mut viewer, b"live").await;
    while let Some(message) = next_within(&mut viewer, Duration::from_secs(2)).await {
        assert_eq!(
            message, b"live",
            "a new viewer of {channel:?} received what was published before it"
        );
    }
}

#[tokio::test]
async fn memory_delivers_every_server_topic_and_keeps_projects_apart() {
    assert_server_channels_deliver_and_stay_apart(Arc::new(MemoryTopicBackend::new())).await;
}

#[tokio::test]
async fn memory_gives_a_new_subscriber_no_history() {
    assert_a_new_subscriber_sees_no_history(Arc::new(MemoryTopicBackend::new())).await;
}

#[tokio::test]
async fn redis_delivers_every_server_topic_and_keeps_projects_apart() {
    let Ok(url) = std::env::var("SIDESEAT_TEST_REDIS_URL") else {
        eprintln!(
            "redis broadcast parity: skipped - set SIDESEAT_TEST_REDIS_URL (or run `make test-redis`)"
        );
        return;
    };
    let backend = Arc::new(
        RedisTopicBackend::new(&url)
            .await
            .expect("connect to Redis"),
    );
    assert_server_channels_deliver_and_stay_apart(backend.clone()).await;
    assert_a_new_subscriber_sees_no_history(backend).await;
}

/// The broadcast topics a broker holds.
fn broadcast_topics(brokers: &str) -> Vec<String> {
    let client: BaseConsumer = ClientConfig::new()
        .set("bootstrap.servers", brokers)
        .create()
        .expect("metadata client");
    let metadata = client
        .fetch_metadata(None, Duration::from_secs(10))
        .expect("fetch metadata");
    let mut topics: Vec<String> = metadata
        .topics()
        .iter()
        .map(|topic| topic.name().to_string())
        .filter(|name| name.starts_with("sideseat_broadcast_"))
        .collect();
    topics.sort();
    topics
}

#[tokio::test]
async fn redpanda_delivers_every_server_topic_and_keeps_projects_apart() {
    let Ok(brokers) = std::env::var("SIDESEAT_TEST_REDPANDA_BROKERS") else {
        eprintln!(
            "redpanda broadcast parity: skipped - set SIDESEAT_TEST_REDPANDA_BROKERS (or run `make test-redpanda`)"
        );
        return;
    };
    let backend = Arc::new(
        RedpandaTopicBackend::new(&RedpandaConfig {
            brokers: brokers.clone(),
            partitions: 3,
            replication_factor: 1,
            retention_ms: 10 * 60 * 1_000,
            retention_warning_ms: 60 * 1_000,
        })
        .await
        .expect("connect to RedPanda"),
    );
    assert_server_channels_deliver_and_stay_apart(backend.clone()).await;
    assert_a_new_subscriber_sees_no_history(backend.clone()).await;

    // Exactly once, with no retry: a message published as soon as `subscribe` returns is delivered - the
    // subscription starts at the partition's high watermark, not when a group's assignment arrives.
    for round in 0..5 {
        let channel = BroadcastChannel::new(SSE_SPANS, fresh_key("o"));
        let mut viewer = backend.subscribe(&channel).await.expect("subscribe");
        backend.publish(&channel, b"first").await.expect("publish");
        let message = next_within(&mut viewer, Duration::from_secs(15)).await;
        assert_eq!(
            message.as_deref(),
            Some(&b"first"[..]),
            "round {round}: the first message after subscribe was lost"
        );
    }

    // One topic per kind, whatever the number of keys: a hundred more projects add none.
    let before = broadcast_topics(&brokers);
    for _ in 0..100 {
        let channel = BroadcastChannel::new(SSE_SPANS, fresh_key("many"));
        backend.publish(&channel, b"x").await.expect("publish");
        drop(backend.subscribe(&channel).await.expect("subscribe"));
    }
    let after = broadcast_topics(&brokers);
    assert_eq!(before, after, "keys created topics");
    assert!(
        after.len() <= KINDS.len(),
        "more broadcast topics than kinds: {after:?}"
    );
}

/// Kafka's own rule for a topic name.
fn valid_kafka_name(name: &str) -> bool {
    (1..=249).contains(&name.len())
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

#[test]
fn queue_topic_names_keep_their_kafka_names() {
    // The durable queue's topics hold acknowledged exports: renaming them would strand what is queued.
    for name in ["traces", "metrics", "logs", "compile-check"] {
        assert_eq!(kafka_topic(name).unwrap(), name);
    }
}

#[test]
fn every_broadcast_kind_has_a_valid_kafka_topic() {
    for kind in KINDS {
        let topic =
            crate::redpanda::broadcast_topic(&BroadcastChannel::new(kind, "any key: at all"))
                .unwrap();
        assert!(
            valid_kafka_name(&topic),
            "{kind} became {topic}, which Kafka refuses"
        );
        assert_eq!(
            topic,
            format!("sideseat_broadcast_{kind}"),
            "a kind's topic carries no key"
        );
    }
    assert_eq!(kafka_topic("sse_spans:abc").unwrap(), "sse_spans.3Aabc");
}

#[test]
fn a_key_always_maps_to_one_partition_within_range() {
    for key in [
        "",
        "p1",
        "default",
        "f3814386-6074-41f8-aecd-86fc0c36a8c2",
        "é",
    ] {
        let channel = BroadcastChannel::new(SSE_SPANS, key);
        let partition = broadcast_partition(&channel);
        assert!((0..32).contains(&partition), "{key}: {partition}");
        assert_eq!(
            partition,
            broadcast_partition(&BroadcastChannel::new(PRESENCE, key))
        );
    }
    // Pinned: a subscriber and a publisher of different releases must agree on a key's partition.
    assert_eq!(
        broadcast_partition(&BroadcastChannel::new(SSE_SPANS, "default")),
        {
            let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
            for byte in b"default" {
                hash ^= u64::from(*byte);
                hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
            }
            (hash % 32) as i32
        }
    );
}

#[test]
fn distinct_topics_never_share_a_kafka_name() {
    let names = [
        "a:b",
        "a.b",
        "a_b",
        "a-b",
        "a/b",
        "a b",
        "a.3Ab",
        "a.3ab",
        "a:3A",
        "a::b",
        "a.3A.3Ab",
        "é",
        "e\u{301}",
        "a:b:c",
        "a.3Ab.3Ac",
        "sse_spans:x",
        "sse_spans.3Ax",
        "sse_spans_x",
    ];
    let mut seen = std::collections::BTreeMap::new();
    for name in names {
        let topic = kafka_topic(name).unwrap();
        assert!(valid_kafka_name(&topic), "{name} became {topic}");
        if let Some(earlier) = seen.insert(topic.clone(), name) {
            panic!("{earlier:?} and {name:?} both became {topic}");
        }
    }
}

#[test]
fn no_topic_takes_a_dead_letter_name() {
    // The adapter names a topic's dead letters `{kafka name}.dead-letter`; an escape is `.` and two uppercase hex
    // digits, so no encoded name contains `.` followed by a lowercase letter.
    for name in ["x.dead-letter", "x.dead", "\u{de}ad-letter", "x\u{de}"] {
        let topic = kafka_topic(name).unwrap();
        let lowercase_after_dot = topic
            .as_bytes()
            .windows(2)
            .any(|pair| pair[0] == b'.' && pair[1].is_ascii_lowercase());
        assert!(!lowercase_after_dot, "{name} became {topic}");
    }
}

#[test]
fn a_name_kafka_cannot_carry_is_refused_not_shortened() {
    assert!(kafka_topic("").is_err());
    assert!(kafka_topic(&"a".repeat(249)).is_ok());
    assert!(kafka_topic(&"a".repeat(250)).is_err());
    assert!(
        kafka_topic(&":".repeat(84)).is_err(),
        "84 escapes are 252 bytes"
    );
}
