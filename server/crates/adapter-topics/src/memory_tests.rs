use super::*;
use futures::StreamExt;

#[tokio::test]
async fn test_broadcast_publish_subscribe() {
    let backend = MemoryTopicBackend::new();

    // Subscribe first
    let sub = backend.subscribe("test").await.unwrap();
    let mut receiver = sub.receiver;

    // Publish
    backend.publish("test", b"hello").await.unwrap();

    // Receive with timeout
    let msg = tokio::time::timeout(tokio::time::Duration::from_millis(100), receiver.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    assert_eq!(msg, b"hello");
}

#[tokio::test]
async fn test_stream_publish_subscribe_ack() {
    let backend = MemoryTopicBackend::new();

    // Publish first
    let id = backend
        .stream_publish("stream", "test-key", b"msg1")
        .await
        .unwrap();
    assert_eq!(id, "1");

    // Subscribe
    let sub = backend
        .stream_subscribe("stream", "group1", "consumer1")
        .await
        .unwrap();
    let mut receiver = sub.receiver;

    // Receive
    let msg = tokio::time::timeout(tokio::time::Duration::from_millis(500), receiver.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    assert_eq!(msg.id, "1");
    assert_eq!(msg.partition, crate::virtual_partition("test-key"));
    assert_eq!(msg.payload, b"msg1");

    // Ack
    backend
        .stream_ack("stream", "group1", &msg.id)
        .await
        .unwrap();

    // Check stats
    let stats = backend.stream_stats("stream", "group1").await.unwrap();
    assert_eq!(stats.length, 1);
    assert_eq!(stats.pending, 0);
}

#[tokio::test]
async fn test_stream_stats() {
    let backend = MemoryTopicBackend::new();

    // Publish messages
    backend
        .stream_publish("stream", "test-key", b"msg1")
        .await
        .unwrap();
    backend
        .stream_publish("stream", "test-key", b"msg2")
        .await
        .unwrap();

    let stats = backend.stream_stats("stream", "group1").await.unwrap();
    assert_eq!(stats.length, 2);
    assert_eq!(stats.pending, 0);
}

#[test]
fn test_backend_name() {
    let backend = MemoryTopicBackend::new();
    assert_eq!(backend.backend_name(), "memory");
}

/// A claim takes the oldest idle messages, in their order. In the pending map's order, which messages a claim took
/// and the order a consumer replayed - and so wrote - them differed from one process to the next.
#[tokio::test]
async fn a_claim_takes_the_oldest_idle_messages_in_order() {
    let backend = MemoryTopicBackend::new();
    for n in 0..20 {
        backend
            .stream_publish("stream", &format!("key-{n}"), format!("msg{n}").as_bytes())
            .await
            .unwrap();
    }
    let sub = backend
        .stream_subscribe("stream", "group", "first")
        .await
        .unwrap();
    let mut receiver = sub.receiver;
    for _ in 0..20 {
        tokio::time::timeout(tokio::time::Duration::from_millis(500), receiver.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
    let claimed: Vec<String> = backend
        .stream_claim("stream", "group", "second", 0, 5)
        .await
        .unwrap()
        .into_iter()
        .map(|message| message.id)
        .collect();
    assert_eq!(claimed, ["1", "2", "3", "4", "5"]);
}
