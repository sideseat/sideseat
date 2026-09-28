use super::*;
use crate::types::ContentBlock;

#[test]
fn node_content_serde_round_trip_user_message() {
    let content = NodeContent::UserMessage {
        content: vec![ContentBlock::text("hello")],
        name: Some("alice".into()),
    };
    let json = serde_json::to_string(&content).unwrap();
    let parsed: NodeContent = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.content_type_str(), "user_message");
}

#[test]
fn node_content_serde_round_trip_assistant_message() {
    let content = NodeContent::AssistantMessage {
        content: vec![ContentBlock::text("hi")],
        stop_reason: Some(StopReason::EndTurn),
        variant_index: Some(0),
    };
    let json = serde_json::to_string(&content).unwrap();
    let parsed: NodeContent = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.content_type_str(), "assistant_message");
}

#[test]
fn node_content_unknown_forward_compat() {
    let json = r#"{"type": "future_type", "foo": "bar"}"#;
    let parsed: NodeContent = serde_json::from_str(json).unwrap();
    match &parsed {
        NodeContent::Unknown { kind, data } => {
            assert_eq!(kind, "future_type");
            assert_eq!(data["foo"], "bar");
        }
        other => panic!("Expected Unknown, got {}", other.content_type_str()),
    }
}

#[test]
fn id_display_and_deref() {
    let id = NodeId::from_string("test-123");
    assert_eq!(id.as_str(), "test-123");
    assert_eq!(format!("{id}"), "test-123");
    assert_eq!(&*id, "test-123");
}

#[test]
fn conversation_patch_maybe_cleared_round_trip() {
    let patch = ConversationPatch {
        instructions: Some(MaybeCleared::Set("do x".into())),
        ..Default::default()
    };
    let json = serde_json::to_string(&patch).unwrap();
    let back: ConversationPatch = serde_json::from_str(&json).unwrap();
    match back.instructions {
        Some(MaybeCleared::Set(s)) => assert_eq!(s, "do x"),
        other => panic!("Expected Set, got {other:?}"),
    }

    let clear_patch = ConversationPatch {
        instructions: Some(MaybeCleared::Clear),
        ..Default::default()
    };
    let json2 = serde_json::to_string(&clear_patch).unwrap();
    let back2: ConversationPatch = serde_json::from_str(&json2).unwrap();
    assert!(matches!(back2.instructions, Some(MaybeCleared::Clear)));
}

#[test]
fn conversation_apply_patch() {
    let mut conv = Conversation::new("hello");
    conv.apply_patch(&ConversationPatch {
        title: Some("new title".into()),
        instructions: Some(MaybeCleared::Set("be helpful".into())),
        ..Default::default()
    });
    assert_eq!(conv.title.as_deref(), Some("new title"));
    assert_eq!(conv.instructions.as_deref(), Some("be helpful"));

    conv.apply_patch(&ConversationPatch {
        instructions: Some(MaybeCleared::Clear),
        ..Default::default()
    });
    assert!(conv.instructions.is_none());
}
