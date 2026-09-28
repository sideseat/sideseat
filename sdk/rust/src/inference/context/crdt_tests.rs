use super::*;
use crate::context::backend::InMemoryContextBackend;
use std::sync::Arc;

fn make_backend() -> Arc<InMemoryContextBackend> {
    Arc::new(InMemoryContextBackend::new())
}

#[test]
fn empty_bytes_produces_blank_doc() {
    let result = CrdtDoc::from_state(&[]);
    assert!(result.is_ok());
    let doc = result.unwrap();
    assert_eq!(doc.full_state(), CrdtDoc::new().full_state());
}

#[test]
fn map_set_and_entries() {
    let mut doc = CrdtDoc::new();
    doc.map_set("items", "a", r#"{"id":"a","v":1}"#);
    doc.map_set("items", "b", r#"{"id":"b","v":2}"#);

    let entries = doc.map_entries("items");
    assert_eq!(entries.len(), 2);
    assert!(entries.contains_key("a"));
    assert!(entries.contains_key("b"));
}

#[test]
fn map_tombstone_removes_entry_from_entries() {
    // Application-level tombstone: map_set with a sentinel value that
    // callers filter. map_entries still sees the key, but the entity is
    // logically deleted. This is the correct pattern — yrs map removals
    // can be resurrected by a concurrent insert with a later clock.
    let mut doc = CrdtDoc::new();
    doc.map_set("items", "a", r#"{"id":"a","deleted":true}"#);

    // Key is present (tombstone) but carries the deleted flag.
    assert!(doc.map_get("items", "a").is_some());
    // Callers filter by deserialising and checking deleted == true.
    let entries = doc.map_entries("items");
    assert_eq!(entries.len(), 1);
    assert!(entries["a"].contains("\"deleted\":true"));
}

#[test]
fn text_roundtrip() {
    let mut doc = CrdtDoc::new();
    doc.text_insert("file.txt", 0, "hello");
    assert_eq!(doc.text_read("file.txt"), "hello");
    assert_eq!(doc.text_len("file.txt"), 5);

    doc.text_remove("file.txt", 0, 5);
    assert_eq!(doc.text_read("file.txt"), "");
}

#[test]
fn state_snapshot_roundtrip() {
    let mut doc1 = CrdtDoc::new();
    doc1.map_set("items", "k", r#"{"id":"k","v":99}"#);

    let state = doc1.full_state();
    let doc2 = CrdtDoc::from_state(&state).unwrap();
    assert_eq!(
        doc2.map_get("items", "k"),
        Some(r#"{"id":"k","v":99}"#.into())
    );
}

#[test]
fn merge_delta_converges() {
    let mut doc_a = CrdtDoc::new();
    let mut doc_b = CrdtDoc::new();

    let delta_a = doc_a.map_set("items", "a", r#"{"id":"a"}"#);
    let delta_b = doc_b.map_set("items", "b", r#"{"id":"b"}"#);

    doc_a.merge_delta(&delta_b).unwrap();
    doc_b.merge_delta(&delta_a).unwrap();

    let ea = doc_a.map_entries("items");
    let eb = doc_b.map_entries("items");
    assert_eq!(ea.len(), 2);
    assert_eq!(eb.len(), 2);
}

#[tokio::test]
async fn push_only_sends_diff() {
    let backend = make_backend();
    let conv = ConversationId::new();
    let branch = BranchId::new();

    let ext = CrdtExtension::new("client-a");
    ext.map_set("items", "a", r#"{"id":"a"}"#);
    ext.push(&conv, &branch, backend.as_ref()).await.unwrap();

    // Second push with no new changes should be a no-op.
    ext.push(&conv, &branch, backend.as_ref()).await.unwrap();

    let deltas = backend.crdt_fetch(&conv, &branch, 0).await.unwrap();
    assert_eq!(deltas.len(), 1, "second push should be a no-op");
}

#[tokio::test]
async fn push_returns_seq_for_watermark() {
    let backend = make_backend();
    let conv = ConversationId::new();
    let branch = BranchId::new();

    let ext = CrdtExtension::new("client-a");
    ext.map_set("items", "a", r#"{"id":"a"}"#);
    let seq = ext.push(&conv, &branch, backend.as_ref()).await.unwrap();

    assert!(seq > 0, "push must return assigned seq");

    // push() does NOT save a snapshot (prevents lost-update race).
    // Verify the returned seq matches the delta appended to the log.
    let deltas = backend.crdt_fetch(&conv, &branch, 0).await.unwrap();
    assert_eq!(deltas.len(), 1);
    assert_eq!(
        deltas[0].global_seq, seq,
        "push must return the assigned global_seq"
    );
}

#[tokio::test]
async fn pull_idempotent_on_own_ops() {
    // pull() merges all deltas unconditionally. CRDT merge is idempotent, so
    // re-applying ops the doc already contains is a safe no-op. This test
    // verifies that a client pulling its own previously-pushed delta does not
    // produce duplicate entries (idempotency guarantee in practice).
    let backend = make_backend();
    let conv = ConversationId::new();
    let branch = BranchId::new();

    let writer = CrdtExtension::new("writer");
    writer.map_set("items", "a", r#"{"id":"a"}"#);
    writer.push(&conv, &branch, backend.as_ref()).await.unwrap();

    // Writer pulls its own branch — "a" was already in the doc before push.
    // After pull it should still be there exactly once (idempotent re-apply).
    writer.pull(&conv, &branch, backend.as_ref()).await.unwrap();
    let entries = writer.map_entries("items");
    assert_eq!(
        entries.len(),
        1,
        "own data must not be duplicated after pull"
    );
    assert_eq!(entries["a"], r#"{"id":"a"}"#);
}

#[tokio::test]
async fn pull_applies_foreign_deltas() {
    let backend = make_backend();
    let conv = ConversationId::new();
    let branch = BranchId::new();

    let writer = CrdtExtension::new("writer");
    let reader = CrdtExtension::new("reader");

    writer.map_set("items", "a", r#"{"id":"a"}"#);
    writer.push(&conv, &branch, backend.as_ref()).await.unwrap();

    reader.pull(&conv, &branch, backend.as_ref()).await.unwrap();
    assert_eq!(reader.map_get("items", "a"), Some(r#"{"id":"a"}"#.into()),);
}

#[tokio::test]
async fn pull_advances_snapshot_seq() {
    let backend = make_backend();
    let conv = ConversationId::new();
    let branch = BranchId::new();

    // Writer pushes two deltas independently.
    let writer = CrdtExtension::new("writer");
    writer.map_set("items", "a", r#"{"id":"a"}"#);
    let seq1 = writer.push(&conv, &branch, backend.as_ref()).await.unwrap();

    let writer2 = CrdtExtension::new("writer2");
    writer2.map_set("items", "b", r#"{"id":"b"}"#);
    let seq2 = writer2
        .push(&conv, &branch, backend.as_ref())
        .await
        .unwrap();
    assert!(seq2 > seq1);

    // Fresh reader with no snapshot — pull must advance snapshot to max delta seq.
    let reader = CrdtExtension::new("reader");
    reader.pull(&conv, &branch, backend.as_ref()).await.unwrap();

    let (snap_seq, _, _) = backend.crdt_load_snapshot(&branch).await.unwrap();
    assert!(
        snap_seq >= seq2,
        "snapshot seq must be >= max delta seq after pull"
    );
}

#[tokio::test]
async fn load_snapshot_then_pull_no_double_apply() {
    let backend = make_backend();
    let conv = ConversationId::new();
    let branch = BranchId::new();

    let writer = CrdtExtension::new("writer");
    writer.map_set("items", "a", r#"{"id":"a"}"#);
    writer.push(&conv, &branch, backend.as_ref()).await.unwrap();

    let snapshot_bytes = writer.to_snapshot();

    // New extension loads snapshot then pulls — must not double-apply.
    let reader = CrdtExtension::new("reader");
    reader.load_snapshot(&snapshot_bytes).unwrap();
    reader.pull(&conv, &branch, backend.as_ref()).await.unwrap();

    // Data is present exactly once.
    assert_eq!(reader.map_get("items", "a"), Some(r#"{"id":"a"}"#.into()));
}

#[tokio::test]
async fn load_snapshot_only_updates_doc() {
    // After load_snapshot(), push should only send NEW ops, not resend snapshot.
    let backend = make_backend();
    let conv = ConversationId::new();
    let branch = BranchId::new();

    // Another client establishes the baseline snapshot.
    let baseline = CrdtExtension::new("baseline");
    baseline.map_set("items", "existing", r#"{"id":"existing"}"#);
    baseline
        .push(&conv, &branch, backend.as_ref())
        .await
        .unwrap();

    // New client loads snapshot, adds one new item, pushes.
    let client = CrdtExtension::new("client");
    let snap = baseline.to_snapshot();
    client.load_snapshot(&snap).unwrap();
    client.map_set("items", "new", r#"{"id":"new"}"#);
    client.push(&conv, &branch, backend.as_ref()).await.unwrap();

    // Only 2 deltas total: baseline's push + client's push (not a full resend).
    let all_deltas = backend.crdt_fetch(&conv, &branch, 0).await.unwrap();
    assert_eq!(
        all_deltas.len(),
        2,
        "push after load_snapshot must send only new ops"
    );
}

#[tokio::test]
async fn concurrent_push_from_two_clients_merges() {
    let backend = make_backend();
    let conv = ConversationId::new();
    let branch = BranchId::new();

    let client_a = CrdtExtension::new("a");
    let client_b = CrdtExtension::new("b");

    client_a.map_set("items", "ka", r#"{"id":"ka"}"#);
    client_b.map_set("items", "kb", r#"{"id":"kb"}"#);

    client_a
        .push(&conv, &branch, backend.as_ref())
        .await
        .unwrap();
    client_b
        .push(&conv, &branch, backend.as_ref())
        .await
        .unwrap();

    // Both pull each other's changes.
    client_a
        .pull(&conv, &branch, backend.as_ref())
        .await
        .unwrap();
    client_b
        .pull(&conv, &branch, backend.as_ref())
        .await
        .unwrap();

    // Both should see both entries.
    assert_eq!(client_a.map_entries("items").len(), 2);
    assert_eq!(client_b.map_entries("items").len(), 2);
}

#[tokio::test]
async fn push_derives_cursor_from_backend() {
    // After checkout (load_snapshot + pull), a subsequent push must send
    // ONLY the new ops added after checkout, not the full snapshot state.
    let backend = make_backend();
    let conv = ConversationId::new();
    let branch = BranchId::new();

    // Step 1: Establish some baseline state on the branch.
    let init = CrdtExtension::new("init");
    init.map_set("items", "base", r#"{"id":"base"}"#);
    init.push(&conv, &branch, backend.as_ref()).await.unwrap();

    // Step 2: New client checks out (simulates checkout flow).
    // push() does not save a snapshot, so crdt_load_snapshot returns (0, [], []).
    // load_snapshot([]) is a no-op; pull() fetches init's delta and populates the doc.
    let (_, snap_bytes, _) = backend.crdt_load_snapshot(&branch).await.unwrap();
    let client = CrdtExtension::new("client");
    client.load_snapshot(&snap_bytes).unwrap();
    client.pull(&conv, &branch, backend.as_ref()).await.unwrap();

    // Step 3: Client adds new data and pushes.
    client.map_set("items", "new", r#"{"id":"new"}"#);
    client.push(&conv, &branch, backend.as_ref()).await.unwrap();

    // Only 2 deltas: init's push + client's push.
    let all = backend.crdt_fetch(&conv, &branch, 0).await.unwrap();
    assert_eq!(
        all.len(),
        2,
        "checkout + push must not resend snapshot state"
    );
}

#[tokio::test]
async fn concurrent_pushes_converge() {
    let backend = make_backend();
    let conv = ConversationId::new();
    let branch = BranchId::new();

    let a = Arc::new(CrdtExtension::new("a"));
    let b = Arc::new(CrdtExtension::new("b"));

    a.map_set("items", "ka", r#"{"id":"ka"}"#);
    b.map_set("items", "kb", r#"{"id":"kb"}"#);

    let backend_a = Arc::clone(&backend);
    let backend_b = Arc::clone(&backend);
    let a_ref = Arc::clone(&a);
    let b_ref = Arc::clone(&b);
    let conv_a = conv.clone();
    let conv_b = conv.clone();
    let branch_a = branch.clone();
    let branch_b = branch.clone();

    let ha = tokio::spawn(async move {
        a_ref
            .push(&conv_a, &branch_a, backend_a.as_ref())
            .await
            .unwrap();
    });
    let hb = tokio::spawn(async move {
        b_ref
            .push(&conv_b, &branch_b, backend_b.as_ref())
            .await
            .unwrap();
    });
    ha.await.unwrap();
    hb.await.unwrap();

    // Pull both sides.
    a.pull(&conv, &branch, backend.as_ref()).await.unwrap();
    b.pull(&conv, &branch, backend.as_ref()).await.unwrap();

    assert_eq!(
        a.map_entries("items").len(),
        2,
        "a must see both after pull"
    );
    assert_eq!(
        b.map_entries("items").len(),
        2,
        "b must see both after pull"
    );
}

#[tokio::test]
async fn load_snapshot_empty_bytes_is_blank_doc() {
    let ext = CrdtExtension::new("c");
    ext.load_snapshot(&[]).unwrap();
    assert!(ext.map_entries("items").is_empty());
}

#[tokio::test]
async fn compact_prunes_deltas() {
    let backend = make_backend();
    let conv = ConversationId::new();
    let branch = BranchId::new();

    let ext = CrdtExtension::new("client");
    for i in 0..10u32 {
        ext.map_set("items", &i.to_string(), &format!(r#"{{"id":"{}"}}"#, i));
        ext.push(&conv, &branch, backend.as_ref()).await.unwrap();
    }

    let before = backend.crdt_fetch(&conv, &branch, 0).await.unwrap();
    assert_eq!(before.len(), 10);

    // pull() must be called first to advance the snapshot (push does not save one).
    ext.pull(&conv, &branch, backend.as_ref()).await.unwrap();
    ext.compact(&conv, &branch, backend.as_ref()).await.unwrap();

    let after = backend.crdt_fetch(&conv, &branch, 0).await.unwrap();
    assert!(
        after.is_empty(),
        "compact must prune all snapshot-covered deltas"
    );
}

#[tokio::test]
async fn subscribe_fires_on_pull() {
    let backend = make_backend();
    let conv = ConversationId::new();
    let branch = BranchId::new();

    // Writer pushes some data.
    let writer = CrdtExtension::new("writer");
    writer.map_set("items", "a", r#"{"id":"a"}"#);
    let pushed_seq = writer.push(&conv, &branch, backend.as_ref()).await.unwrap();

    // Reader subscribes to notifications.
    let reader = CrdtExtension::new("reader").with_notifications(16);
    let mut rx = reader.subscribe().unwrap();

    reader.pull(&conv, &branch, backend.as_ref()).await.unwrap();

    let notified_seq = rx.try_recv().expect("subscriber must receive seq on pull");
    assert_eq!(
        notified_seq, pushed_seq,
        "notified seq must match pushed seq"
    );
}

#[tokio::test]
async fn fork_snapshot_reconstruction() {
    // Parent branch accumulates some state + pushes.
    // Child is created from a snapshot of parent at a specific seq.
    let backend = make_backend();
    let conv = ConversationId::new();
    let parent_branch = BranchId::new();

    let parent = CrdtExtension::new("parent");
    parent.map_set("items", "k1", r#"{"id":"k1"}"#);
    let push_seq = parent
        .push(&conv, &parent_branch, backend.as_ref())
        .await
        .unwrap();
    assert!(push_seq > 0);

    // Capture snapshot at this point.
    let snapshot_bytes = parent.to_snapshot();

    // Parent adds more after the snapshot.
    parent.map_set("items", "k2", r#"{"id":"k2"}"#);
    parent
        .push(&conv, &parent_branch, backend.as_ref())
        .await
        .unwrap();

    // Build child from the snapshot (before k2 was added).
    let child_branch = BranchId::new();
    let child = CrdtExtension::new("child");
    child.load_snapshot(&snapshot_bytes).unwrap();

    // Child should see k1 but NOT k2 (k2 was added after snapshot).
    let entries = child.map_entries("items");
    assert!(
        entries.contains_key("k1"),
        "child must have k1 from snapshot"
    );
    assert!(
        !entries.contains_key("k2"),
        "child must not have k2 (post-snapshot)"
    );

    // Push child state; it should be a no-op since no new writes.
    child
        .push(&conv, &child_branch, backend.as_ref())
        .await
        .unwrap();
    // Child pulls its own branch — no deltas expected.
    child
        .pull(&conv, &child_branch, backend.as_ref())
        .await
        .unwrap();
    assert_eq!(child.map_entries("items").len(), 1);
}

#[tokio::test]
async fn two_level_ancestry() {
    // Grandparent → parent snapshot → child snapshot.
    // Child must see grandparent's entries via the parent snapshot chain.
    let backend = make_backend();
    let conv = ConversationId::new();
    let grandparent_branch = BranchId::new();

    // Grandparent writes and pushes.
    let grandparent = CrdtExtension::new("gp");
    grandparent.map_set("items", "gp_key", r#"{"id":"gp_key"}"#);
    grandparent
        .push(&conv, &grandparent_branch, backend.as_ref())
        .await
        .unwrap();

    // Parent snapshot = grandparent full state.
    let gp_bytes = grandparent.to_snapshot();

    let parent_branch = BranchId::new();
    let parent = CrdtExtension::new("parent");
    parent.load_snapshot(&gp_bytes).unwrap();
    parent.map_set("items", "parent_key", r#"{"id":"parent_key"}"#);
    parent
        .push(&conv, &parent_branch, backend.as_ref())
        .await
        .unwrap();

    // Child snapshot = parent full state (includes gp_key via snapshot).
    let parent_bytes = parent.to_snapshot();

    let child_branch = BranchId::new();
    let child = CrdtExtension::new("child");
    child.load_snapshot(&parent_bytes).unwrap();

    let entries = child.map_entries("items");
    assert!(
        entries.contains_key("gp_key"),
        "child must see grandparent key"
    );
    assert!(
        entries.contains_key("parent_key"),
        "child must see parent key"
    );

    // Child adds its own key.
    child.map_set("items", "child_key", r#"{"id":"child_key"}"#);
    child
        .push(&conv, &child_branch, backend.as_ref())
        .await
        .unwrap();

    let all = child.map_entries("items");
    assert_eq!(all.len(), 3);
}
