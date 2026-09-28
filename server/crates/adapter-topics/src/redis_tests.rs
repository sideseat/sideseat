use super::*;

#[test]
fn test_key_prefixes() {
    // Test key generation using constants directly
    let topic = "test";
    let stream_key = format!("{}{}", STREAM_PREFIX, topic);
    let pubsub_channel = format!("{}{}", PUBSUB_PREFIX, topic);

    assert_eq!(stream_key, "{sideseat}:stream:test");
    assert_eq!(pubsub_channel, "{sideseat}:pubsub:test");
}

/// Stream ids order numerically, not lexicographically.
///
/// The trim boundary is a minimum over consumer groups, and `"9-0" < "10-0"` is false as text. A
/// string comparison would therefore pick a boundary *later* than the oldest entry a group still
/// needs and `XTRIM MINID` would delete unread work - the very failure the trim exists to avoid.
#[test]
fn stream_ids_order_numerically() {
    let nine = StreamId::parse("9-0").expect("parses");
    let ten = StreamId::parse("10-0").expect("parses");
    assert!(
        nine < ten,
        "9-0 must precede 10-0, however it sorts as text"
    );
    assert!(
        "9-0" > "10-0",
        "the lexicographic order really is the wrong one"
    );

    // The sequence orders within one millisecond.
    assert!(StreamId::parse("5-2").unwrap() < StreamId::parse("5-10").unwrap());

    // Round trips, because the boundary is sent back to Redis as a string.
    assert_eq!(
        StreamId::parse("1700000000000-3").unwrap().to_string(),
        "1700000000000-3"
    );
    assert!(StreamId::parse("not-an-id").is_none());
    assert!(StreamId::parse("12345").is_none());
}

/// The entry after the last delivered one is the oldest a caught-up group still needs.
#[test]
fn the_next_id_follows_its_own_id() {
    let id = StreamId::parse("100-4").unwrap();
    assert!(id < id.next());
    assert_eq!(id.next().to_string(), "100-5");
    // A saturated sequence rolls into the next millisecond rather than wrapping backwards.
    let saturated = StreamId {
        millis: 100,
        sequence: u64::MAX,
    };
    assert!(saturated < saturated.next());
}

/// `XINFO GROUPS` entries are flat key/value arrays, and the fields are read by name.
#[test]
fn group_fields_are_read_by_name() {
    let group = RedisValue::Array(vec![
        RedisValue::BulkString(b"name".to_vec()),
        RedisValue::BulkString(b"traces".to_vec()),
        RedisValue::BulkString(b"last-delivered-id".to_vec()),
        RedisValue::BulkString(b"42-1".to_vec()),
        RedisValue::BulkString(b"pending".to_vec()),
        RedisValue::Int(3),
    ]);
    assert_eq!(group_field(&group, "name").as_deref(), Some("traces"));
    assert_eq!(
        group_field(&group, "last-delivered-id").as_deref(),
        Some("42-1")
    );
    assert_eq!(group_field(&group, "pending").as_deref(), Some("3"));
    // An absent field is absent, not the next value along.
    assert_eq!(group_field(&group, "entries-read"), None);
    assert_eq!(group_field(&RedisValue::Nil, "name"), None);
}

#[test]
fn stream_fields_preserve_the_publish_partition_key() {
    let fields = vec![
        RedisValue::BulkString(b"partition_key".to_vec()),
        RedisValue::BulkString(b"trace-42".to_vec()),
        RedisValue::BulkString(b"payload".to_vec()),
        RedisValue::BulkString(b"bytes".to_vec()),
    ];
    let (payload, partition) = extract_message_fields(&fields).expect("stream fields");
    assert_eq!(payload, b"bytes");
    assert_eq!(partition, crate::virtual_partition("trace-42"));
}

#[test]
fn test_sanitize_redis_url() {
    assert_eq!(
        sanitize_redis_url("redis://localhost:6379"),
        "redis://localhost:6379"
    );
    assert_eq!(
        sanitize_redis_url("redis://user:pass@localhost:6379"),
        "redis://user:***@localhost:6379"
    );
}
