use super::*;
use crate::context::backend::InMemoryContextBackend;

fn make_backend() -> Arc<InMemoryContextBackend> {
    Arc::new(InMemoryContextBackend::new())
}

#[tokio::test]
async fn new_creates_conversation_and_branch() {
    let backend = make_backend();
    let conv = Conversation::new("test");
    let conv_id = conv.id.clone();

    let mgr = ContextManager::new(backend.clone(), conv).await.unwrap();
    assert_eq!(mgr.active_branch_tip(), None);

    // Conversation registered in KV.
    let keys = backend.kv_list("conversations", "").await.unwrap();
    assert!(keys.iter().any(|k| k == conv_id.as_str()));
}

#[tokio::test]
async fn add_node_and_build_context() {
    let backend = make_backend();
    let conv = Conversation::new("ctx-test");

    let mgr = ContextManager::new(backend, conv).await.unwrap();

    mgr.add_system_message(vec![ContentBlock::text("You are helpful.")])
        .await
        .unwrap();
    mgr.add_user_message(vec![ContentBlock::text("Hello")], NodeParams::default())
        .await
        .unwrap();
    mgr.add_node(
        NodeContent::AssistantMessage {
            content: vec![ContentBlock::text("Hi!")],
            stop_reason: None,
            variant_index: None,
        },
        NodeParams::default(),
    )
    .await
    .unwrap();

    let result = mgr.build_context().await.unwrap();
    assert!(result.system.is_some());
    // System message removed from messages list when system is Some.
    assert_eq!(
        result
            .messages
            .iter()
            .filter(|m| m.role == Role::System)
            .count(),
        0
    );
    assert_eq!(result.messages.len(), 2);
    assert!(result.estimated_tokens > 0);
}

#[tokio::test]
async fn truncate_overflow() {
    let backend = make_backend();
    let conv = Conversation::new("trunc");

    let compression = CompressionConfig {
        max_tokens: 5,
        strategy: CompressionStrategy::Truncate,
        system_mode: SystemMode::None,
        pinned_node_ids: Vec::new(),
        summarizer: None,
    };

    let mgr = ContextManager::new(backend, conv)
        .await
        .unwrap()
        .with_compression(compression);

    for i in 0..10 {
        mgr.add_user_message(
            vec![ContentBlock::text(format!("message {i}"))],
            NodeParams::default(),
        )
        .await
        .unwrap();
    }

    let result = mgr.build_context().await.unwrap();
    assert!(result.messages.len() < 10);
}

#[tokio::test]
async fn fail_overflow() {
    let backend = make_backend();
    let conv = Conversation::new("fail");

    let compression = CompressionConfig {
        max_tokens: 1,
        strategy: CompressionStrategy::Fail,
        system_mode: SystemMode::None,
        pinned_node_ids: Vec::new(),
        summarizer: None,
    };

    let mgr = ContextManager::new(backend, conv)
        .await
        .unwrap()
        .with_compression(compression);

    mgr.add_user_message(vec![ContentBlock::text("hello")], NodeParams::default())
        .await
        .unwrap();

    let result = mgr.build_context().await;
    assert!(matches!(result, Err(CmError::ContextOverflow(_))));
}

#[tokio::test]
async fn sliding_window_overflow() {
    let backend = make_backend();
    let conv = Conversation::new("sliding");

    let compression = CompressionConfig {
        max_tokens: 10,
        strategy: CompressionStrategy::SlidingWindow { keep_last: 3 },
        system_mode: SystemMode::None,
        pinned_node_ids: Vec::new(),
        summarizer: None,
    };

    let mgr = ContextManager::new(backend, conv)
        .await
        .unwrap()
        .with_compression(compression);

    for i in 0..20 {
        mgr.add_user_message(
            vec![ContentBlock::text(format!("msg {i}"))],
            NodeParams::default(),
        )
        .await
        .unwrap();
    }

    let result = mgr.build_context().await.unwrap();
    assert_eq!(result.messages.len(), 3);
}

#[tokio::test]
async fn load_roundtrip() {
    let backend = make_backend();
    let conv = Conversation::new("load-test");
    let conv_id = conv.id.clone();

    let mgr = ContextManager::new(Arc::clone(&backend), conv)
        .await
        .unwrap();
    mgr.add_user_message(vec![ContentBlock::text("hello")], NodeParams::default())
        .await
        .unwrap();

    // Load from backend.
    let mgr2 = ContextManager::load(backend, &conv_id).await.unwrap();
    let msgs = mgr2.to_messages().await.unwrap();
    assert_eq!(msgs.len(), 1);
}

#[tokio::test]
async fn patch_conversation_applies() {
    let backend = make_backend();
    let conv = Conversation::new("patch-test");

    let mgr = ContextManager::new(backend, conv).await.unwrap();
    mgr.patch_conversation(ConversationPatch {
        title: Some("New Title".into()),
        ..Default::default()
    })
    .await
    .unwrap();

    let conv = mgr.conversation();
    assert_eq!(conv.title, Some("New Title".into()));
}

#[tokio::test]
async fn list_conversations_returns_registered() {
    let backend = make_backend();
    let conv1 = Conversation::new("c1");
    let conv2 = Conversation::new("c2");
    let id1 = conv1.id.clone();
    let id2 = conv2.id.clone();

    ContextManager::new(Arc::clone(&backend), conv1)
        .await
        .unwrap();
    ContextManager::new(Arc::clone(&backend), conv2)
        .await
        .unwrap();

    let ids = ContextManager::list_conversations(&backend).await.unwrap();
    assert!(ids.iter().any(|id| id == &id1));
    assert!(ids.iter().any(|id| id == &id2));
}

#[tokio::test]
async fn spawn_agent_no_nodes_returns_error() {
    let backend = make_backend();
    let conv = Conversation::new("spawn");

    let mgr = ContextManager::new(backend, conv).await.unwrap();
    let result = mgr.spawn_agent(AgentId::new(), None).await;
    assert!(matches!(result, Err(CmError::NoNodes)));
}

#[tokio::test]
async fn import_messages_roundtrip() {
    let backend = make_backend();
    let conv = Conversation::new("import");

    let mgr = ContextManager::new(backend, conv).await.unwrap();

    let messages = vec![
        Message {
            role: Role::User,
            content: vec![ContentBlock::text("Hello")],
            name: None,
            cache_control: None,
        },
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::text("Hi!")],
            name: None,
            cache_control: None,
        },
    ];
    let ids = mgr.import_messages(&messages).await.unwrap();
    assert_eq!(ids.len(), 2);

    let result = mgr.to_messages().await.unwrap();
    assert_eq!(result.len(), 2);
    assert_eq!(result[0].role, Role::User);
    assert_eq!(result[1].role, Role::Assistant);
}

#[tokio::test]
async fn project_node_to_message_covers_variants() {
    let conv_id = ConversationId::new();
    let branch_id = BranchId::new();

    let tool_node = Node {
        id: NodeId::new(),
        conversation_id: conv_id.clone(),
        branch_id: branch_id.clone(),
        parent_id: None,
        sequence: 0,
        created_at: 0,
        created_by: None,
        model: None,
        provider: None,
        content: NodeContent::ToolResult {
            tool_use_id: "tu_1".into(),
            content: vec![ContentBlock::text("result")],
            is_error: false,
            duration_ms: None,
        },
        usage: None,
        version: 0,
        is_final: true,
        streaming: None,
        deleted: false,
        agent_id: None,
        correlation_id: None,
        reply_to: None,
        eval_scores: Vec::new(),
        metadata: HashMap::new(),
        crdt_seq_watermark: None,
    };

    let msg = project_node_to_message(&tool_node).unwrap();
    assert_eq!(msg.role, Role::Tool);
}

#[tokio::test]
async fn deleted_nodes_filtered_in_build_context() {
    let backend = make_backend();
    let conv = Conversation::new("deleted-filter");

    let mgr = ContextManager::new(backend, conv).await.unwrap();
    mgr.add_user_message(vec![ContentBlock::text("keep")], NodeParams::default())
        .await
        .unwrap();
    let del_id = mgr
        .add_user_message(vec![ContentBlock::text("delete me")], NodeParams::default())
        .await
        .unwrap();

    mgr.backend.soft_delete_node(&del_id).await.unwrap();

    let result = mgr.build_context().await.unwrap();
    assert_eq!(result.messages.len(), 1);
    assert!(
        result.messages[0]
            .content
            .iter()
            .any(|b| b.as_text().is_some_and(|t| t == "keep"))
    );
}

#[tokio::test]
async fn server_compaction_flag() {
    let backend = make_backend();
    let conv = Conversation::new("compaction");

    let compression = CompressionConfig {
        max_tokens: 1,
        strategy: CompressionStrategy::ServerCompaction {
            compact_threshold: 10,
        },
        system_mode: SystemMode::None,
        pinned_node_ids: Vec::new(),
        summarizer: None,
    };

    let mgr = ContextManager::new(backend, conv)
        .await
        .unwrap()
        .with_compression(compression);

    mgr.add_user_message(vec![ContentBlock::text("hello")], NodeParams::default())
        .await
        .unwrap();

    let result = mgr.build_context().await.unwrap();
    assert!(result.server_compaction_needed);
    assert_eq!(result.messages.len(), 1); // all messages returned as-is
}

#[tokio::test]
async fn memory_source_always_injected() {
    let backend = make_backend();
    let conv = Conversation::new("mem-inject");
    let scope_id = crate::context::types::UserId::new();

    let mem_src = KvMemorySource::new(Arc::clone(&backend), scope_id.as_str());

    // Pre-populate a memory item.
    let item = MemoryItem {
        content: "User prefers short answers".into(),
        source: "test".into(),
        relevance_score: None,
        metadata: HashMap::new(),
    };
    mem_src.store(&ConversationId::new(), &item).await.unwrap();

    let mgr = ContextManager::new(Arc::clone(&backend), conv)
        .await
        .unwrap()
        .with_memory_source(Arc::new(KvMemorySource::new(
            Arc::clone(&backend),
            scope_id.as_str(),
        )));

    mgr.add_user_message(
        vec![ContentBlock::text("short answer")],
        NodeParams::default(),
    )
    .await
    .unwrap();

    let result = mgr.build_context().await.unwrap();
    // Memory is injected into system prompt regardless of system mode.
    let sys = result.system.unwrap_or_default();
    assert!(sys.contains("User prefers short answers"), "system: {sys}");
}

#[tokio::test]
async fn fork_checkout_restores_crdt_at_watermark() {
    let backend = make_backend();
    let conv = Conversation::new("fork-crdt");

    let mgr = ContextManager::new(Arc::clone(&backend), conv)
        .await
        .unwrap()
        .with_extension(Arc::new(CrdtExtension::new("main")));

    // Write CRDT entry BEFORE committing the node so the auto-push captures it.
    if let Some(crdt) = mgr.extension::<CrdtExtension>() {
        crdt.map_set("items", "k1", r#"{"id":"k1"}"#);
    }

    // Add a node — this auto-pushes CRDT and records the watermark.
    let node_id = mgr
        .add_user_message(vec![ContentBlock::text("root")], NodeParams::default())
        .await
        .unwrap();

    // Fork from the head node (which carries the CRDT watermark).
    let new_branch_id = mgr.fork(&node_id, "child").await.unwrap();

    // Checkout the child branch — restores CRDT to fork-point state.
    mgr.checkout(&new_branch_id).await.unwrap();

    // CRDT state should be visible on child.
    if let Some(crdt) = mgr.extension::<CrdtExtension>() {
        let entries = crdt.map_entries("items");
        assert!(
            entries.contains_key("k1"),
            "fork must inherit parent CRDT state"
        );
    }
}

#[tokio::test]
async fn load_crdt_checkout_roundtrip() {
    let backend = make_backend();
    let conv = Conversation::new("load-crdt");
    let conv_id = conv.id.clone();

    // Create, add CRDT state, add a node to record the watermark.
    let mgr = ContextManager::new(Arc::clone(&backend), conv)
        .await
        .unwrap()
        .with_extension(Arc::new(CrdtExtension::new("c1")));

    if let Some(crdt) = mgr.extension::<CrdtExtension>() {
        crdt.map_set("ns", "key", "value");
    }
    mgr.add_user_message(vec![ContentBlock::text("hello")], NodeParams::default())
        .await
        .unwrap();

    let active_branch = mgr.active_branch();

    // Load into a fresh ContextManager with a new CrdtExtension, then checkout.
    let mgr2 = ContextManager::load(Arc::clone(&backend), &conv_id)
        .await
        .unwrap()
        .with_extension(Arc::new(CrdtExtension::new("c2")));
    mgr2.checkout(&active_branch).await.unwrap();

    // CRDT state must be restored after checkout.
    let crdt2 = mgr2.extension::<CrdtExtension>().unwrap();
    let entries = crdt2.map_entries("ns");
    assert!(
        entries.contains_key("key"),
        "CRDT state must survive load+checkout"
    );
    assert_eq!(entries["key"], "value");
}

#[tokio::test]
async fn finalize_streaming_node_no_occ() {
    let backend = make_backend();
    let conv = Conversation::new("streaming");

    let mgr = ContextManager::new(backend, conv).await.unwrap();

    // start_streaming creates a non-final node with streaming state.
    let id = mgr
        .start_streaming(
            NodeContent::AssistantMessage {
                content: vec![ContentBlock::text("...")],
                stop_reason: None,
                variant_index: None,
            },
            NodeParams::default(),
        )
        .await
        .unwrap();

    let node = mgr.backend.get_node(&id).await.unwrap().unwrap();
    assert!(!node.is_final);
    assert!(node.streaming.is_some());

    // finalize_streaming_node must not fail regardless of version.
    mgr.finalize_streaming_node(
        &id,
        NodeContent::AssistantMessage {
            content: vec![ContentBlock::text("Full response.")],
            stop_reason: None,
            variant_index: None,
        },
        None,
    )
    .await
    .unwrap();

    let updated = mgr.backend.get_node(&id).await.unwrap().unwrap();
    assert!(updated.is_final);
    assert!(updated.streaming.is_none());
    // Content replaced
    if let NodeContent::AssistantMessage { content, .. } = &updated.content {
        assert!(content[0].as_text().unwrap().contains("Full response"));
    }
}

#[tokio::test]
async fn summarize_compression() {
    use std::sync::Arc as StdArc;

    struct EchoSummarizer;

    #[async_trait::async_trait]
    impl Summarizer for EchoSummarizer {
        async fn summarize(&self, messages: &[Message]) -> Result<String, CmError> {
            Ok(format!("summarized {} messages", messages.len()))
        }
    }

    let backend = make_backend();
    let conv = Conversation::new("summarize");

    let compression = CompressionConfig {
        max_tokens: 5,
        strategy: CompressionStrategy::Summarize,
        system_mode: SystemMode::None,
        pinned_node_ids: Vec::new(),
        summarizer: Some(StdArc::new(EchoSummarizer)),
    };

    let mgr = ContextManager::new(backend, conv)
        .await
        .unwrap()
        .with_compression(compression);

    for i in 0..10 {
        mgr.add_user_message(
            vec![ContentBlock::text(format!("message {i}"))],
            NodeParams::default(),
        )
        .await
        .unwrap();
    }

    let result = mgr.build_context().await.unwrap();
    // Summary is returned.
    assert!(result.summary.is_some());
    let s = result.summary.unwrap();
    assert!(s.contains("summarized"), "summary: {s}");
    // The summary is injected as the first message.
    assert!(!result.messages.is_empty());
}

#[test]
fn maybe_cleared_serde_roundtrip() {
    use crate::context::types::{ConversationPatch, MaybeCleared};

    // Set variant
    let patch_set = ConversationPatch {
        instructions: Some(MaybeCleared::Set("be helpful".into())),
        ..Default::default()
    };
    let json = serde_json::to_string(&patch_set).unwrap();
    let back: ConversationPatch = serde_json::from_str(&json).unwrap();
    match back.instructions.unwrap() {
        MaybeCleared::Set(s) => assert_eq!(s, "be helpful"),
        MaybeCleared::Clear => panic!("expected Set"),
    }

    // Clear variant — must be distinguishable from Set in JSON
    let patch_clear = ConversationPatch {
        instructions: Some(MaybeCleared::Clear),
        ..Default::default()
    };
    let json_clear = serde_json::to_string(&patch_clear).unwrap();
    let back_clear: ConversationPatch = serde_json::from_str(&json_clear).unwrap();
    assert!(matches!(back_clear.instructions, Some(MaybeCleared::Clear)));

    // The two JSON representations must differ
    assert_ne!(json, json_clear);
}

#[tokio::test]
async fn export_sft_jsonl_basic() {
    use crate::context::types::{DatasetEntry, DatasetEntryId, DatasetSplit};

    let backend = make_backend();
    let conv = Conversation::new("sft-test");
    let conv_id = conv.id.clone();

    let mgr = ContextManager::new(Arc::clone(&backend), conv)
        .await
        .unwrap();
    let input_id = mgr
        .add_user_message(
            vec![ContentBlock::text("What is 2+2?")],
            NodeParams::default(),
        )
        .await
        .unwrap();
    let output_id = mgr
        .add_node(
            NodeContent::AssistantMessage {
                content: vec![ContentBlock::text("4")],
                stop_reason: None,
                variant_index: None,
            },
            NodeParams::default(),
        )
        .await
        .unwrap();

    // Store a DatasetEntry in KV.
    let entry = DatasetEntry {
        id: DatasetEntryId::new(),
        conversation_id: conv_id.clone(),
        dataset_name: "my_dataset".into(),
        input_node_ids: vec![input_id],
        output_node_id: output_id,
        expected_output: None,
        split: DatasetSplit::Train,
        created_at: now_micros(),
        metadata: HashMap::new(),
    };
    let bytes = serde_json::to_vec(&entry).unwrap();
    backend
        .kv_put("dataset:my_dataset", entry.id.as_str(), &bytes)
        .await
        .unwrap();

    let mut buf = Vec::new();
    let count = mgr
        .export_sft_jsonl("my_dataset", None, &mut buf)
        .await
        .unwrap();

    assert_eq!(count, 1);
    let line: serde_json::Value = serde_json::from_slice(&buf[..buf.len() - 1]).unwrap();
    assert_eq!(line["completion"], "4");
    assert_eq!(line["split"], "train");
}

/// Sub-agent spawned from a child can immediately spawn its own sub-agent
/// (branch tip is initialized from the fork node).
#[tokio::test]
async fn sub_agent_can_spawn_sub_agent() {
    let backend = make_backend();
    let conv = Conversation::new("nested-agents");

    let root = ContextManager::new(backend, conv).await.unwrap();

    // Root needs at least one node before spawn_agent can fork.
    root.add_user_message(vec![ContentBlock::text("start")], NodeParams::default())
        .await
        .unwrap();

    // First level sub-agent.
    let child = root.spawn_agent(AgentId::new(), None).await.unwrap();
    child
        .add_user_message(
            vec![ContentBlock::text("child work")],
            NodeParams::default(),
        )
        .await
        .unwrap();

    // Second level: child spawns its own sub-agent — must not return NoNodes.
    let grandchild = child.spawn_agent(AgentId::new(), None).await;
    assert!(
        grandchild.is_ok(),
        "sub-agent must be able to spawn its own sub-agents; got: {:?}",
        grandchild.err()
    );
}

/// Sub-agent kanban/canvas updates are visible to the parent after merge_from_branch.
#[tokio::test]
async fn merge_from_branch_propagates_crdt() {
    let backend = make_backend();
    let conv = Conversation::new("merge-test");

    let parent = ContextManager::new(Arc::clone(&backend), conv)
        .await
        .unwrap()
        .with_extension(Arc::new(CrdtExtension::new("parent")));

    parent
        .add_user_message(vec![ContentBlock::text("task")], NodeParams::default())
        .await
        .unwrap();

    // Spawn child and equip it with its own CRDT extension.
    let child_branch = parent.active_branch();
    let child = parent
        .spawn_agent(AgentId::new(), None)
        .await
        .unwrap()
        .with_extension(Arc::new(CrdtExtension::new("agent")));
    child.checkout(&child.active_branch()).await.unwrap();

    // Agent writes a kanban update on its own branch.
    if let Some(crdt) = child.extension::<CrdtExtension>() {
        crdt.map_set(
            "kanban:board1:cpos",
            "card-1",
            r#"{"column_id":"done","position":0}"#,
        );
    }
    // Persist agent's CRDT to backend.
    if let Some(crdt) = child.extension::<CrdtExtension>() {
        let conv_id = child.conversation().id.clone();
        let branch_id = child.active_branch();
        crdt.push(&conv_id, &branch_id, child.backend().as_ref())
            .await
            .unwrap();
    }

    let agent_branch = child.active_branch();

    // Parent merges agent's branch — no parent push needed to read in-memory.
    parent.merge_from_branch(&agent_branch).await.unwrap();

    let parent_crdt = parent.extension::<CrdtExtension>().unwrap();
    let entries = parent_crdt.map_entries("kanban:board1:cpos");
    assert!(
        entries.contains_key("card-1"),
        "parent must see agent's kanban update after merge_from_branch"
    );
    let _ = child_branch; // suppress unused warning
}

/// Workspace supports multiple kanban boards.
#[test]
fn workspace_multiple_kanban_boards() {
    use super::types::{KanbanBoardId, Workspace};

    let mut ws = Workspace::default();
    assert!(ws.kanban_ids.is_empty());

    let b1 = KanbanBoardId::new();
    let b2 = KanbanBoardId::new();
    ws.kanban_ids.push(b1.clone());
    ws.kanban_ids.push(b2.clone());

    assert_eq!(ws.kanban_ids.len(), 2);

    // Roundtrip through JSON (serde).
    let json = serde_json::to_string(&ws).unwrap();
    let ws2: Workspace = serde_json::from_str(&json).unwrap();
    assert_eq!(ws2.kanban_ids.len(), 2);
    assert!(ws2.kanban_ids.contains(&b1));
    assert!(ws2.kanban_ids.contains(&b2));
}
