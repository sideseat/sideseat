use deadpool_redis::redis::Value as RedisValue;
use sideseat_ports::queue::StreamMessage;

/// Parse XREADGROUP response to extract messages
pub(super) fn parse_xreadgroup_response(value: RedisValue) -> Option<Vec<StreamMessage>> {
    // Response format: [[stream_name, [[id, [field, value, ...]], ...]]]
    let streams = match value {
        RedisValue::Array(arr) => arr,
        _ => return None,
    };

    let mut messages = Vec::new();

    for stream_data in streams {
        let RedisValue::Array(parts) = stream_data else {
            continue;
        };
        if parts.len() < 2 {
            continue;
        }
        // parts[0] = stream name, parts[1] = messages array
        let RedisValue::Array(msg_list) = &parts[1] else {
            continue;
        };
        for msg in msg_list {
            if let RedisValue::Array(msg_parts) = msg
                && msg_parts.len() >= 2
                && let (RedisValue::BulkString(id_bytes), RedisValue::Array(fields)) =
                    (&msg_parts[0], &msg_parts[1])
                && let Ok(id) = String::from_utf8(id_bytes.clone())
                && let Some((payload, partition)) = extract_message_fields(fields)
            {
                messages.push(StreamMessage {
                    id,
                    partition,
                    payload,
                });
            }
        }
    }

    if messages.is_empty() {
        None
    } else {
        Some(messages)
    }
}

/// Extract the payload and stable virtual partition from Redis stream entry fields.
pub(super) fn extract_message_fields(fields: &[RedisValue]) -> Option<(Vec<u8>, u32)> {
    // Fields are [field1, value1, field2, value2, ...]
    let mut payload = None;
    let mut partition_key = None;
    let mut iter = fields.iter();
    while let Some(field) = iter.next() {
        let value = iter.next();
        if let (RedisValue::BulkString(field_name), Some(RedisValue::BulkString(value))) =
            (field, value)
        {
            match field_name.as_slice() {
                b"payload" => payload = Some(value.clone()),
                b"partition_key" => {
                    partition_key = std::str::from_utf8(value).ok().map(str::to_owned);
                }
                _ => {}
            }
        }
    }
    payload.map(|payload| {
        (
            payload,
            crate::virtual_partition(partition_key.as_deref().unwrap_or_default()),
        )
    })
}
