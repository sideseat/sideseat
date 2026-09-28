use super::*;
use futures::StreamExt;

/// The ids currently retained, in order. Read from the state rather than through a subscription, because
/// the property under test is what the queue *kept*, not what a consumer managed to see.
fn retained_ids(backend: &MemoryTopicBackend, topic: &str) -> Vec<u64> {
    let streams = backend.state.streams.read();
    streams
        .get(topic)
        .map(|s| s.messages.iter().map(|e| e.id).collect())
        .unwrap_or_default()
}

fn retained_bytes(backend: &MemoryTopicBackend, topic: &str) -> u64 {
    let streams = backend.state.streams.read();
    streams.get(topic).map_or(0, |s| s.retained_bytes)
}

/// A full queue refuses the next publish and keeps everything it already accepted.
///
/// The two halves are one assertion: refusing is only correct *because* nothing was dropped to make room,
/// and a bound that trims would satisfy the first half while failing the second silently.
#[tokio::test]
async fn a_full_queue_refuses_and_loses_nothing() {
    // Room for exactly three entries of this size, so the fourth has to be refused.
    let payload = vec![b'x'; 1024];
    let budget = StreamEntry::budget_cost(payload.len()) * 3;
    let backend = MemoryTopicBackend::with_stream_budget(budget);

    let mut accepted = Vec::new();
    for _ in 0..3 {
        let id = backend
            .stream_publish("t", "k", &payload)
            .await
            .expect("within budget");
        accepted.push(id.parse::<u64>().expect("numeric id"));
    }

    let refused = backend.stream_publish("t", "k", &payload).await;
    assert!(
        matches!(refused, Err(TopicError::BufferFull)),
        "a publish past the budget must be refused, not made room for; got {refused:?}"
    );

    assert_eq!(
        retained_ids(&backend, "t"),
        accepted,
        "every accepted entry is still here - nothing was deleted to admit anything"
    );
}

/// A delivered-but-unacknowledged entry survives a full queue.
#[tokio::test]
async fn an_unacknowledged_entry_is_never_dropped_to_make_room() {
    let payload = vec![b'y'; 512];
    // Two entries plus the one pending record the delivery below creates. The pending charge is part of the
    // budget - a group holds one record per delivered-and-unacked entry, and leaving that out of the bound
    // made it multiplicative in groups - so a budget sized for entries alone would refuse the second
    // publish and this test would pass for the wrong reason.
    let budget = StreamEntry::budget_cost(payload.len()) * 2 + STREAM_PENDING_RECORD_OVERHEAD_BYTES;
    let backend = MemoryTopicBackend::with_stream_budget(budget);

    backend
        .stream_publish("t", "k", &payload)
        .await
        .expect("first");

    // Take it, and do not acknowledge it.
    let sub = backend
        .stream_subscribe("t", "g", "c")
        .await
        .expect("subscribe");
    let mut receiver = sub.receiver;
    let held = tokio::time::timeout(std::time::Duration::from_millis(500), receiver.next())
        .await
        .expect("delivered")
        .expect("some")
        .expect("ok");
    assert_eq!(held.id, "1");

    backend
        .stream_publish("t", "k", &payload)
        .await
        .expect("second fits");
    let refused = backend.stream_publish("t", "k", &payload).await;
    assert!(
        matches!(refused, Err(TopicError::BufferFull)),
        "the queue is full of work that is still owed, so the publish is refused"
    );

    assert!(
        retained_ids(&backend, "t").contains(&1),
        "the entry the consumer is holding must still exist"
    );
    let streams = backend.state.streams.read();
    assert!(
        streams["t"].groups["g"].pending.contains_key(&1),
        "and the group must still record that it owes the work"
    );
}

/// Acknowledging frees the budget, so a refusal is transient rather than terminal.
///
/// Without this the refusal would be a deadlock: the queue fills once and never accepts again.
#[tokio::test]
async fn acknowledging_frees_the_budget() {
    let payload = vec![b'z'; 256];
    let budget = StreamEntry::budget_cost(payload.len()) * 2;
    let backend = MemoryTopicBackend::with_stream_budget(budget);

    for _ in 0..2 {
        backend
            .stream_publish("t", "k", &payload)
            .await
            .expect("fits");
    }
    assert!(matches!(
        backend.stream_publish("t", "k", &payload).await,
        Err(TopicError::BufferFull)
    ));

    let sub = backend
        .stream_subscribe("t", "g", "c")
        .await
        .expect("subscribe");
    let mut receiver = sub.receiver;
    for expected in ["1", "2"] {
        let msg = tokio::time::timeout(std::time::Duration::from_millis(500), receiver.next())
            .await
            .expect("delivered")
            .expect("some")
            .expect("ok");
        assert_eq!(msg.id, expected);
        backend.stream_ack("t", "g", &msg.id).await.expect("ack");
    }

    // The next publish reclaims what was acknowledged and is admitted.
    let id = backend
        .stream_publish("t", "k", &payload)
        .await
        .expect("the budget freed up once the work was acknowledged");
    assert_eq!(id, "3");
    assert_eq!(
        retained_ids(&backend, "t"),
        vec![3],
        "the two acknowledged entries were reclaimed and only the new one is retained"
    );
}

/// Nothing is trimmed while no consumer group exists.
///
/// Nobody has read the stream, so every entry is still needed - and a publisher that outruns a consumer
/// that has not arrived yet is told to wait rather than having its backlog quietly deleted.
#[tokio::test]
async fn a_stream_with_no_consumer_group_is_never_trimmed() {
    let payload = vec![b'w'; 128];
    let budget = StreamEntry::budget_cost(payload.len()) * 2;
    let backend = MemoryTopicBackend::with_stream_budget(budget);

    backend
        .stream_publish("t", "k", &payload)
        .await
        .expect("first");
    backend
        .stream_publish("t", "k", &payload)
        .await
        .expect("second");
    assert!(matches!(
        backend.stream_publish("t", "k", &payload).await,
        Err(TopicError::BufferFull)
    ));
    assert_eq!(retained_ids(&backend, "t"), vec![1, 2]);
    assert_eq!(
        backend.stream_trim_consumed("t").await.expect("trim"),
        0,
        "an unread stream has nothing consumed to reclaim"
    );
}

/// Two consumers of one group split the stream instead of both replaying it.
///
/// With a cursor per consumer, the second consumer's cursor sat behind the first's, so an entry the first
/// took *and acknowledged* looked undelivered to the second and was processed twice. Ingestion is
/// idempotent by span id, so the cost was duplicated work rather than corruption - and it is still not
/// what a consumer group means, and it is what made "consumed" undefinable.
#[tokio::test]
async fn two_consumers_of_one_group_split_the_stream() {
    let backend = MemoryTopicBackend::new();
    for n in 0..4u8 {
        backend
            .stream_publish("t", "k", &[n])
            .await
            .expect("publish");
    }

    let mut seen: Vec<String> = Vec::new();
    for consumer in ["c1", "c2"] {
        let sub = backend
            .stream_subscribe("t", "g", consumer)
            .await
            .expect("subscribe");
        let mut receiver = sub.receiver;
        for _ in 0..2 {
            let msg = tokio::time::timeout(std::time::Duration::from_millis(500), receiver.next())
                .await
                .expect("delivered")
                .expect("some")
                .expect("ok");
            backend.stream_ack("t", "g", &msg.id).await.expect("ack");
            seen.push(msg.id);
        }
    }

    seen.sort();
    assert_eq!(
        seen,
        vec!["1", "2", "3", "4"],
        "each entry goes to exactly one consumer of the group"
    );
}

/// Many consumer groups holding the same entries are charged for their own state.
///
/// The bound counted each entry once, so it was blind to a cost that *multiplies*: every group holds a
/// pending record per delivered-and-unacknowledged entry. Ten thousand entries against a thousand abandoned
/// groups is ten million records the budget did not see, and the queue reported itself comfortably inside
/// 128 MB while holding gigabytes. Retaining a group's unread entries is correct; omitting the group's own
/// state from the bound is not.
#[tokio::test]
async fn group_state_is_charged_against_the_budget() {
    let payload = vec![b'g'; 64];
    // Room for two entries and nothing else, so the pending records are what tips it over.
    let budget = StreamEntry::budget_cost(payload.len()) * 2;
    let backend = MemoryTopicBackend::with_stream_budget(budget);

    backend
        .stream_publish("t", "k", &payload)
        .await
        .expect("first");

    // Several groups take it and none acknowledges. Each holds its own pending record.
    let mut receivers = Vec::new();
    for group in ["g1", "g2", "g3", "g4"] {
        let sub = backend
            .stream_subscribe("t", group, "c")
            .await
            .expect("subscribe");
        let mut receiver = sub.receiver;
        let msg = tokio::time::timeout(std::time::Duration::from_millis(500), receiver.next())
            .await
            .expect("delivered")
            .expect("some")
            .expect("ok");
        assert_eq!(msg.id, "1");
        receivers.push(receiver);
    }

    let refused = backend.stream_publish("t", "k", &payload).await;
    assert!(
        matches!(refused, Err(TopicError::BufferFull)),
        "four groups each holding a pending record cost real memory the budget has to see; got {refused:?}"
    );
}

/// The number of consumer groups is bounded, because their pending state is not bounded by bytes.
///
/// Charging pending records at publish is only half a bound: group state grows at *delivery*, and delivery
/// cannot refuse without stalling a consumer. So one retained entry delivered to unboundedly many groups is
/// unbounded memory the byte budget cannot see, and the group count is where that closes.
#[tokio::test]
async fn the_consumer_group_count_is_bounded() {
    let backend = MemoryTopicBackend::new();
    backend
        .stream_publish("t", "k", b"payload")
        .await
        .expect("publish");

    for n in 0..STREAM_MAX_CONSUMER_GROUPS {
        backend
            .stream_subscribe("t", &format!("g{n}"), "c")
            .await
            .unwrap_or_else(|e| panic!("group {n} is within the cap: {e}"));
    }

    let refused = backend.stream_subscribe("t", "one-too-many", "c").await;
    assert!(
        matches!(refused, Err(TopicError::ConsumerGroup(_))),
        "a group past the cap must be refused with a reason; got {:?}",
        refused.map(|_| "subscribed")
    );

    // An existing group re-subscribing is not a new group, so it is never refused - which is what a
    // reconnecting consumer does.
    backend
        .stream_subscribe("t", "g0", "c2")
        .await
        .expect("re-subscribing to an existing group is not a new group");
}

/// A group's remembered consumer names are bounded, whatever a reconnecting client does.
///
/// The group cap does not reach this: a client that reconnects under a fresh name keeps the group count at one
/// while adding a name per reconnect, and every entry and pending record is reclaimed - so nothing else in
/// the queue's accounting notices.
#[tokio::test]
async fn remembered_consumer_names_are_bounded() {
    let backend = MemoryTopicBackend::new();

    for n in 0..(STREAM_MAX_REMEMBERED_CONSUMERS * 3) {
        backend
            .stream_publish("t", "k", b"x")
            .await
            .expect("publish");
        let sub = backend
            .stream_subscribe("t", "g", &format!("consumer-{n}"))
            .await
            .expect("subscribe");
        let mut receiver = sub.receiver;
        let msg = tokio::time::timeout(std::time::Duration::from_millis(500), receiver.next())
            .await
            .expect("delivered")
            .expect("some")
            .expect("ok");
        backend.stream_ack("t", "g", &msg.id).await.expect("ack");
    }

    let remembered = {
        let streams = backend.state.streams.read();
        streams["t"].groups["g"].consumers.len()
    };
    assert!(
        remembered <= STREAM_MAX_REMEMBERED_CONSUMERS,
        "a group remembered {remembered} consumer names, past the {STREAM_MAX_REMEMBERED_CONSUMERS} cap"
    );

    // And the stat still reports something useful rather than collapsing to one.
    let stats = backend.stream_stats("t", "g").await.expect("stats");
    assert!(
        stats.consumers > 0 && stats.consumers as usize <= STREAM_MAX_REMEMBERED_CONSUMERS,
        "the consumer count stays within the cap and is not zero: {}",
        stats.consumers
    );
}

/// The incremental byte counter equals the entries it claims to describe.
///
/// The counter is maintained in two places - incremented on publish, decremented on trim - and a counter
/// maintained in two places is a counter that drifts. A drift downward would let the queue admit past its
/// budget; upward, it would refuse a queue that is nearly empty.
#[tokio::test]
async fn retained_bytes_are_the_sum_of_the_entries() {
    let backend = MemoryTopicBackend::new();
    for size in [10usize, 5_000, 1, 900] {
        backend
            .stream_publish("t", "k", &vec![b'q'; size])
            .await
            .expect("publish");
    }

    let expected: u64 = {
        let streams = backend.state.streams.read();
        streams["t"]
            .messages
            .iter()
            .map(|e| StreamEntry::budget_cost(e.payload.len()))
            .sum()
    };
    assert_eq!(retained_bytes(&backend, "t"), expected, "after publishing");

    // Consume half, then check the counter followed the trim rather than only the publishes.
    let sub = backend
        .stream_subscribe("t", "g", "c")
        .await
        .expect("subscribe");
    let mut receiver = sub.receiver;
    for _ in 0..2 {
        let msg = tokio::time::timeout(std::time::Duration::from_millis(500), receiver.next())
            .await
            .expect("delivered")
            .expect("some")
            .expect("ok");
        backend.stream_ack("t", "g", &msg.id).await.expect("ack");
    }
    backend.stream_trim_consumed("t").await.expect("trim");

    let expected: u64 = {
        let streams = backend.state.streams.read();
        streams["t"]
            .messages
            .iter()
            .map(|e| StreamEntry::budget_cost(e.payload.len()))
            .sum()
    };
    assert_eq!(retained_bytes(&backend, "t"), expected, "after trimming");
}

/// A single payload larger than the whole budget is refused, not admitted and then deleted.
///
/// The boundary case, and the one where "trim to fit" and "refuse" differ most: there is no set of other
/// entries whose removal would make room, so a length-driven bound admits it and then empties the queue
/// around it.
#[tokio::test]
async fn a_payload_larger_than_the_budget_is_refused() {
    let backend = MemoryTopicBackend::with_stream_budget(1024);
    let refused = backend.stream_publish("t", "k", &vec![b'!'; 4096]).await;
    assert!(
        matches!(refused, Err(TopicError::BufferFull)),
        "got {refused:?}"
    );
    assert!(
        retained_ids(&backend, "t").is_empty(),
        "and nothing was stored for it"
    );
}
