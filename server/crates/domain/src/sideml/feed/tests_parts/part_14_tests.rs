// ============================================================================
// Declared feed vocabulary against the retired Rust tables
// ============================================================================

/// The event directions the assets declare are exactly the retired output and input lists.
#[test]
fn the_declared_directions_are_the_lists_they_replace() {
    use crate::rules::schema::MessageDirection;
    let declared = |direction: MessageDirection| -> std::collections::BTreeSet<&str> {
        crate::rules::ruleset()
            .event_roles
            .iter()
            .filter(|(_, declared)| declared.direction == Some(direction))
            .map(|(name, _)| name.as_str())
            .collect()
    };
    assert_eq!(
        declared(MessageDirection::Output),
        GENAI_OUTPUT_EVENTS.iter().copied().collect()
    );
    assert_eq!(
        declared(MessageDirection::Input),
        GENAI_INPUT_EVENTS.iter().copied().collect()
    );
    // And the one input event that states the assistant's reply is the one the promotion used to name.
    let assistant_inputs: Vec<&str> = crate::rules::ruleset()
        .event_roles
        .iter()
        .filter(|(_, declared)| {
            declared.direction == Some(MessageDirection::Input)
                && declared.role_on(false) == Some(crate::sideml::ChatRole::Assistant)
        })
        .map(|(name, _)| name.as_str())
        .collect();
    assert_eq!(assistant_inputs, vec!["gen_ai.assistant.message"]);
}

/// The declared synthetic call id reads a name out exactly as the retired parser did.
#[test]
fn the_declared_synthetic_call_id_parses_as_the_reader_it_replaced() {
    fn retired(id: &str) -> Option<&str> {
        let (name, index) = id.rsplit_once('_')?;
        (!name.is_empty() && !index.is_empty() && index.bytes().all(|byte| byte.is_ascii_digit()))
            .then_some(name)
    }
    for id in [
        "get_weather_0",
        "get_weather_12",
        "weather_",
        "_3",
        "weather_x1",
        "weather",
        "a_b_c_7",
        "toolu_bdrk_01",
        "",
        "name_-1",
        "名前_4",
    ] {
        assert_eq!(
            super::correlate::fallback_tool_name_for_test(id),
            retired(id),
            "{id:?}"
        );
    }
}

/// The declared control-block and value-wrapper members answer as the retired literals did.
#[test]
fn the_declared_control_and_wrapper_members_match_the_literals_they_replace() {
    let members = &crate::rules::ruleset().message_members;
    let control = |data: &serde_json::Value| {
        data.as_object().is_some_and(|object| {
            object.len() == 1
                && object
                    .iter()
                    .all(|(member, value)| value.is_object() && members.marks_control_block(member))
        })
    };
    let retired_control = |data: &serde_json::Value| {
        data.as_object().is_some_and(|object| {
            object.len() == 1
                && object
                    .get("cachePoint")
                    .is_some_and(serde_json::Value::is_object)
        })
    };
    for data in [
        json!({"cachePoint": {"type": "default"}}),
        json!({"cachePoint": "default"}),
        json!({"cachePoint": {}, "text": "x"}),
        json!({"cache_point": {}}),
        json!({}),
        json!([{"cachePoint": {}}]),
    ] {
        assert_eq!(control(&data), retired_control(&data), "{data}");
    }
    let wrapped = |object: &serde_json::Value| {
        members
            .structured_value_wrapper()
            .find_map(|member| object.get(member))
            .cloned()
    };
    for object in [
        json!({"json": {"a": 1}}),
        json!({"json": null, "other": 1}),
        json!({"Json": 1}),
        json!({"data": {"json": 1}}),
        json!([1]),
    ] {
        assert_eq!(wrapped(&object), object.get("json").cloned(), "{object}");
    }
}
