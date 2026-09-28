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
