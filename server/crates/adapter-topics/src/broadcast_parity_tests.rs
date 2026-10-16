//! Broadcast delivery on every backend, with the topic names the server really uses.
//!
//! SSE, presence, agent invocation and connection control each publish on a per-project or per-request topic whose
//! name carries a `:` (`sse_spans:<project>`). A backend must deliver each such topic to its subscribers and keep
//! two projects' topics apart. Redpanda once refused every one of these names (`InvalidTopic`), so live events never
//! reached a viewer there, and no test covered broadcast outside the in-process backend. The memory backend always
//! runs; Redis and Redpanda run when `SIDESEAT_TEST_REDIS_URL` or `SIDESEAT_TEST_REDPANDA_BROKERS` is set
//! (`make test-redis`, `make test-redpanda`).

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use sideseat_core::config::RedpandaConfig;
use sideseat_ports::queue::{BroadcastSubscription, TopicBackend};

use crate::memory::MemoryTopicBackend;
use crate::redis::RedisTopicBackend;
use crate::redpanda::{RedpandaTopicBackend, kafka_topic};

/// A topic name of each shape the server publishes on, unique to this run so reruns against a shared broker never
/// see an earlier run's messages.
fn server_topic_names() -> Vec<String> {
    let run = uuid::Uuid::new_v4().simple().to_string();
    vec![
        format!("sse_spans:p{}", &run[..23]),
        format!("presence:p{}", &run[..23]),
        format!("agent_request:{}", uuid::Uuid::new_v4()),
        format!("connection_control:{run}"),
    ]
}

/// The next message, or `None` if none arrives within `wait`.
async fn next_within(subscription: &mut BroadcastSubscription, wait: Duration) -> Option<Vec<u8>> {
    tokio::time::timeout(wait, subscription.receiver.next())
        .await
        .ok()
        .flatten()
        .map(|message| message.expect("receive broadcast"))
}

/// Publish until the subscriber sees the message: a subscription can need a moment before it receives (a Kafka
/// consumer joins its group, a Redis bridge subscribes), and broadcast promises delivery only to a live subscriber.
async fn publish_until_received(
    backend: &dyn TopicBackend,
    topic: &str,
    subscription: &mut BroadcastSubscription,
    payload: &[u8],
) {
    for _ in 0..60 {
        backend
            .publish(topic, payload)
            .await
            .expect("publish broadcast");
        if let Some(message) = next_within(subscription, Duration::from_millis(500)).await {
            assert_eq!(message, payload, "{topic}: delivered a different message");
            return;
        }
    }
    panic!("{topic}: nothing published there reached its subscriber in 30 s");
}

/// Every server topic shape is delivered, and a message on one project's topic never reaches another's.
async fn assert_server_topics_deliver_and_stay_apart(backend: Arc<dyn TopicBackend>) {
    let names = server_topic_names();
    for name in &names {
        let mut subscription = backend.subscribe(name).await.expect("subscribe");
        publish_until_received(backend.as_ref(), name, &mut subscription, name.as_bytes()).await;
    }

    let run = uuid::Uuid::new_v4().simple().to_string();
    let (first, second) = (format!("sse_spans:a{run}"), format!("sse_spans:b{run}"));
    let mut first_viewer = backend.subscribe(&first).await.expect("subscribe");
    let mut second_viewer = backend.subscribe(&second).await.expect("subscribe");
    publish_until_received(backend.as_ref(), &second, &mut second_viewer, b"warm").await;
    publish_until_received(backend.as_ref(), &first, &mut first_viewer, b"first").await;
    // Drain what the warm-up loop may have published more than once, then check nothing of the first project's
    // came through.
    while let Some(message) = next_within(&mut second_viewer, Duration::from_secs(2)).await {
        assert_eq!(
            message, b"warm",
            "{second} received a message published on {first}"
        );
    }
}

/// A subscriber receives what is published after it subscribes, and nothing published before: a viewer opening a
/// live stream must not first replay the topic's history (Redpanda's broadcast once joined a fresh consumer group at
/// the earliest offset and did exactly that).
async fn assert_a_new_subscriber_sees_no_history(backend: Arc<dyn TopicBackend>) {
    let topic = format!("sse_spans:h{}", uuid::Uuid::new_v4().simple());
    // An earlier viewer keeps the topic live, as one would in production, while the history is published.
    let mut earlier = backend.subscribe(&topic).await.expect("subscribe");
    publish_until_received(backend.as_ref(), &topic, &mut earlier, b"history").await;
    for _ in 0..20 {
        backend
            .publish(&topic, b"history")
            .await
            .expect("publish history");
    }
    let mut viewer = backend.subscribe(&topic).await.expect("subscribe");
    publish_until_received(backend.as_ref(), &topic, &mut viewer, b"live").await;
    while let Some(message) = next_within(&mut viewer, Duration::from_secs(2)).await {
        assert_eq!(
            message, b"live",
            "a new viewer of {topic} received what was published before it"
        );
    }
}

#[tokio::test]
async fn memory_delivers_every_server_topic_and_keeps_projects_apart() {
    assert_server_topics_deliver_and_stay_apart(Arc::new(MemoryTopicBackend::new())).await;
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
    let backend = RedisTopicBackend::new(&url)
        .await
        .expect("connect to Redis");
    assert_server_topics_deliver_and_stay_apart(Arc::new(backend)).await;
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
            brokers,
            partitions: 3,
            replication_factor: 1,
            retention_ms: 10 * 60 * 1_000,
            retention_warning_ms: 60 * 1_000,
        })
        .await
        .expect("connect to RedPanda"),
    );
    assert_server_topics_deliver_and_stay_apart(backend.clone()).await;
    assert_a_new_subscriber_sees_no_history(backend.clone()).await;

    // Exactly once, with no retry: a message published as soon as `subscribe` returns is delivered, on whichever
    // partition it lands - the subscription starts at each partition's high watermark, not when a group's
    // assignment arrives.
    for round in 0..5 {
        let topic = format!("sse_spans:o{}", uuid::Uuid::new_v4().simple());
        let mut viewer = backend.subscribe(&topic).await.expect("subscribe");
        backend.publish(&topic, b"first").await.expect("publish");
        let message = next_within(&mut viewer, Duration::from_secs(15)).await;
        assert_eq!(
            message.as_deref(),
            Some(&b"first"[..]),
            "round {round}: the first message after subscribe was lost"
        );
    }
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
fn every_server_topic_has_a_valid_kafka_name() {
    for name in server_topic_names() {
        let topic = kafka_topic(&name).unwrap();
        assert!(
            valid_kafka_name(&topic),
            "{name} became {topic}, which Kafka refuses"
        );
    }
    assert_eq!(kafka_topic("sse_spans:abc").unwrap(), "sse_spans.3Aabc");
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
