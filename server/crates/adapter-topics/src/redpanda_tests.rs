//! Queue semantics against a real RedPanda broker.

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use sideseat_core::core::config::RedpandaConfig;
use sideseat_ports::queue::TopicBackend;

use super::redpanda::RedpandaTopicBackend;

const URL_ENV: &str = "SIDESEAT_TEST_REDPANDA_BROKERS";

async fn backend() -> Option<Arc<RedpandaTopicBackend>> {
    let brokers = match std::env::var(URL_ENV) {
        Ok(value) => value,
        Err(_) => {
            eprintln!(
                "redpanda tests: skipped - set {URL_ENV} to broker addresses (or run `make test-redpanda`)"
            );
            return None;
        }
    };
    Some(Arc::new(
        RedpandaTopicBackend::new(&RedpandaConfig {
            brokers,
            partitions: 3,
            replication_factor: 1,
            retention_ms: 10 * 60 * 1_000,
            retention_warning_ms: 60 * 1_000,
        })
        .await
        .expect("connect to RedPanda"),
    ))
}

async fn receive(
    subscription: &mut sideseat_ports::queue::StreamSubscription,
) -> sideseat_ports::queue::StreamMessage {
    tokio::time::timeout(Duration::from_secs(15), subscription.receiver.next())
        .await
        .expect("message timed out")
        .expect("subscription ended")
        .expect("consume message")
}

#[tokio::test]
async fn keyed_delivery_uses_contiguous_commits_and_kafka_recovery_semantics() {
    let Some(backend) = backend().await else {
        return;
    };
    let suffix = uuid::Uuid::new_v4();
    let topic = format!("sideseat-redpanda-{suffix}");
    let group = format!("group-{suffix}");
    let mut subscription = backend
        .stream_subscribe(&topic, &group, "consumer-a")
        .await
        .expect("subscribe");

    let mut published = Vec::new();
    for payload in [b"zero".as_slice(), b"one".as_slice(), b"two".as_slice()] {
        published.push(
            backend
                .stream_publish(&topic, "one-trace", payload)
                .await
                .expect("publish"),
        );
    }
    let partitions: Vec<_> = published
        .iter()
        .map(|id| id.split_once(':').unwrap().0)
        .collect();
    assert!(
        partitions.windows(2).all(|pair| pair[0] == pair[1]),
        "one partition key must stay on one partition"
    );

    let first = receive(&mut subscription).await;
    let second = receive(&mut subscription).await;
    let third = receive(&mut subscription).await;
    let delivered_partition = first.id.split_once(':').unwrap().0.parse::<u32>().unwrap();
    assert_eq!(first.partition, delivered_partition);
    assert_eq!(second.partition, delivered_partition);
    assert_eq!(third.partition, delivered_partition);
    assert_eq!(first.payload, b"zero");
    assert_eq!(second.payload, b"one");
    assert_eq!(third.payload, b"two");

    backend
        .stream_ack(&topic, &group, &second.id)
        .await
        .expect("ack later record");
    assert_eq!(
        backend
            .stream_stats(&topic, &group)
            .await
            .expect("stats")
            .pending,
        3,
        "a later success must not commit past the earlier gap"
    );

    backend
        .stream_ack(&topic, &group, &first.id)
        .await
        .expect("close gap");
    assert_eq!(
        backend
            .stream_stats(&topic, &group)
            .await
            .expect("stats")
            .pending,
        1
    );
    backend
        .stream_ack(&topic, &group, &third.id)
        .await
        .expect("ack final record");
    assert_eq!(
        backend
            .stream_stats(&topic, &group)
            .await
            .expect("stats")
            .pending,
        0
    );

    assert!(
        backend
            .stream_claim(&topic, &group, "consumer-b", 0, 10)
            .await
            .expect("claim is a no-op")
            .is_empty()
    );
    assert_eq!(
        backend
            .stream_trim_consumed(&topic)
            .await
            .expect("trim is a no-op"),
        0
    );
    backend.health_check().await.expect("healthy");
}
