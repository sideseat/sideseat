use super::*;
use crate::context::crdt::CrdtExtension;

#[test]
fn column_crud() {
    let crdt = CrdtExtension::new("c");
    let ext = KanbanExtension;
    let conv_id = ConversationId::new();
    let board = KanbanBoard::new(conv_id, "Sprint 1");

    let col1 = KanbanColumn::new(board.id.clone(), "To Do", 0);
    let col2 = KanbanColumn::new(board.id.clone(), "Done", 1);

    ext.upsert_column(&crdt, &col1).unwrap();
    ext.upsert_column(&crdt, &col2).unwrap();

    let cols = ext.list_columns(&crdt, &board.id);
    assert_eq!(cols.len(), 2);
    assert_eq!(cols[0].name, "To Do"); // sorted by position
    assert_eq!(cols[1].name, "Done");
}

#[test]
fn column_tombstone() {
    let crdt = CrdtExtension::new("c");
    let ext = KanbanExtension;
    let conv_id = ConversationId::new();
    let board = KanbanBoard::new(conv_id, "Board");

    let col = KanbanColumn::new(board.id.clone(), "Backlog", 0);
    ext.upsert_column(&crdt, &col).unwrap();

    ext.remove_column(&crdt, &board.id, &col.id);
    assert!(ext.list_columns(&crdt, &board.id).is_empty());
}

#[test]
fn cards_in_deleted_column_filtered() {
    // Cards whose column has been deleted must not appear in list_cards (C3 fix).
    let crdt = CrdtExtension::new("c");
    let ext = KanbanExtension;
    let conv_id = ConversationId::new();
    let board = KanbanBoard::new(conv_id, "Board");
    let col = KanbanColumn::new(board.id.clone(), "Deleted Col", 0);
    let card = KanbanCard::new(col.id.clone(), board.id.clone(), "Ghost", 0);

    ext.upsert_column(&crdt, &col).unwrap();
    ext.upsert_card(&crdt, &card).unwrap();

    // Verify card appears before column deletion.
    assert_eq!(ext.list_cards(&crdt, &board.id, None).len(), 1);

    // Delete the column — card still in cpos pointing to this column.
    ext.remove_column(&crdt, &board.id, &col.id);

    // Card must not appear even though it was never explicitly deleted.
    assert!(
        ext.list_cards(&crdt, &board.id, None).is_empty(),
        "card in deleted column must be filtered from list_cards"
    );
}

#[test]
fn card_crud_and_filter() {
    let crdt = CrdtExtension::new("c");
    let ext = KanbanExtension;
    let conv_id = ConversationId::new();
    let board = KanbanBoard::new(conv_id, "Board");
    let col1 = KanbanColumn::new(board.id.clone(), "Todo", 0);
    let col2 = KanbanColumn::new(board.id.clone(), "Done", 1);

    let card = KanbanCard::new(col1.id.clone(), board.id.clone(), "Task A", 0);
    ext.upsert_card(&crdt, &card).unwrap();

    let all = ext.list_cards(&crdt, &board.id, None);
    assert_eq!(all.len(), 1);

    let in_col1 = ext.list_cards(&crdt, &board.id, Some(&col1.id));
    assert_eq!(in_col1.len(), 1);

    let in_col2 = ext.list_cards(&crdt, &board.id, Some(&col2.id));
    assert!(in_col2.is_empty());
}

#[test]
fn card_tombstone() {
    let crdt = CrdtExtension::new("c");
    let ext = KanbanExtension;
    let conv_id = ConversationId::new();
    let board = KanbanBoard::new(conv_id, "Board");
    let col = KanbanColumn::new(board.id.clone(), "Col", 0);
    let card = KanbanCard::new(col.id.clone(), board.id.clone(), "Task", 0);

    ext.upsert_card(&crdt, &card).unwrap();
    ext.remove_card(&crdt, &board.id, &card.id);

    assert!(ext.list_cards(&crdt, &board.id, None).is_empty());
}

#[test]
fn remove_card_only_updates_cdel() {
    let crdt = CrdtExtension::new("c");
    let ext = KanbanExtension;
    let conv_id = ConversationId::new();
    let board = KanbanBoard::new(conv_id, "Board");
    let col = KanbanColumn::new(board.id.clone(), "Col", 0);
    let card = KanbanCard::new(col.id.clone(), board.id.clone(), "Task", 0);

    ext.upsert_card(&crdt, &card).unwrap();
    ext.remove_card(&crdt, &board.id, &card.id);

    // cpos must still exist (remove_card only touches cdel).
    let cpos_entries = crdt.map_entries(&KanbanExtension::cpos_map(&board.id));
    assert!(
        cpos_entries.contains_key(card.id.as_str()),
        "cpos must survive remove_card"
    );

    // cmeta must still have deleted:false (remove_card does NOT touch cmeta).
    let meta_json = crdt
        .map_get(&KanbanExtension::cmeta_map(&board.id), card.id.as_str())
        .unwrap();
    let meta: CardMeta = serde_json::from_str(&meta_json).unwrap();
    assert!(
        !meta.deleted,
        "cmeta.deleted must remain false — tombstone is in cdel"
    );

    // cdel must contain the card id.
    let cdel_entries = crdt.map_entries(&KanbanExtension::cdel_map(&board.id));
    assert_eq!(
        cdel_entries.get(card.id.as_str()).map(|s| s.as_str()),
        Some("1")
    );

    // list_cards must return empty.
    assert!(ext.list_cards(&crdt, &board.id, None).is_empty());
}

#[test]
fn remove_card_tombstone_survives_concurrent_patch() {
    // Client A deletes the card (writes cdel="1").
    // Client B concurrently edits the title (writes cmeta + cfields).
    // After merge: card must still be deleted (cdel survives).
    let crdt_a = CrdtExtension::new("a");
    let crdt_b = CrdtExtension::new("b");
    let ext = KanbanExtension;
    let conv_id = ConversationId::new();
    let board = KanbanBoard::new(conv_id, "Board");
    let col = KanbanColumn::new(board.id.clone(), "Col", 0);
    let card = KanbanCard::new(col.id.clone(), board.id.clone(), "Task", 0);

    // Both start with the card.
    ext.upsert_card(&crdt_a, &card).unwrap();
    crdt_b.merge_raw(&crdt_a.full_state()).unwrap();

    // A deletes; B edits title concurrently.
    ext.remove_card(&crdt_a, &board.id, &card.id);
    ext.patch_card_meta(
        &crdt_b,
        &board.id,
        &card.id,
        Some("New Title".into()),
        None,
        None,
        None,
        None,
        vec![],
    )
    .unwrap();

    // Merge both ways.
    let sv_a = crdt_a.state_vector();
    let sv_b = crdt_b.state_vector();
    crdt_a.merge_raw(&crdt_b.encode_diff(&sv_a)).unwrap();
    crdt_b.merge_raw(&crdt_a.encode_diff(&sv_b)).unwrap();

    // On both sides: card must be absent (deletion wins).
    assert!(
        ext.list_cards(&crdt_a, &board.id, None).is_empty(),
        "A: deleted card must not appear"
    );
    assert!(
        ext.list_cards(&crdt_b, &board.id, None).is_empty(),
        "B: deleted card must not appear"
    );
}

#[test]
fn cards_sorted_by_position() {
    let crdt = CrdtExtension::new("c");
    let ext = KanbanExtension;
    let conv_id = ConversationId::new();
    let board = KanbanBoard::new(conv_id, "Board");
    let col = KanbanColumn::new(board.id.clone(), "Col", 0);

    let card_b = KanbanCard::new(col.id.clone(), board.id.clone(), "B", 1);
    let card_a = KanbanCard::new(col.id.clone(), board.id.clone(), "A", 0);
    ext.upsert_card(&crdt, &card_b).unwrap();
    ext.upsert_card(&crdt, &card_a).unwrap();

    let cards = ext.list_cards(&crdt, &board.id, None);
    assert_eq!(cards[0].title, "A");
    assert_eq!(cards[1].title, "B");
}

#[test]
fn concurrent_field_edits_no_conflict() {
    // Client A writes ONLY the title cfield.
    // Client B writes ONLY the description cfield.
    // Per-field cfields entries let both survive CRDT merge independently.
    // (Contrast with cmeta blob where either write overwrites the other.)
    let crdt_a = CrdtExtension::new("a");
    let crdt_b = CrdtExtension::new("b");
    let ext = KanbanExtension;
    let conv_id = ConversationId::new();
    let board = KanbanBoard::new(conv_id, "Board");
    let col = KanbanColumn::new(board.id.clone(), "Col", 0);
    let card = KanbanCard::new(col.id.clone(), board.id.clone(), "Original", 0);

    ext.upsert_card(&crdt_a, &card).unwrap();
    crdt_b.merge_raw(&crdt_a.full_state()).unwrap();

    // A writes ONLY the title field (targeted cfields write, no desc).
    let cfields = KanbanExtension::cfields_map(&board.id);
    crdt_a.map_set(
        &cfields,
        &format!("{}:title", card.id.as_str()),
        r#""New Title""#,
    );

    // B writes ONLY the description field (targeted cfields write, no title).
    crdt_b.map_set(
        &cfields,
        &format!("{}:desc", card.id.as_str()),
        r#""Desc from B""#,
    );

    // CRDT merge.
    let sv_a = crdt_a.state_vector();
    let sv_b = crdt_b.state_vector();
    crdt_a.merge_raw(&crdt_b.encode_diff(&sv_a)).unwrap();
    crdt_b.merge_raw(&crdt_a.encode_diff(&sv_b)).unwrap();

    // A must see its own title + B's description.
    let cards_a = ext.list_cards(&crdt_a, &board.id, None);
    assert_eq!(cards_a.len(), 1);
    assert_eq!(cards_a[0].title, "New Title", "A must see own title");
    assert_eq!(
        cards_a[0].description.as_deref(),
        Some("Desc from B"),
        "A must see B's description"
    );

    // B must see A's title + its own description.
    let cards_b = ext.list_cards(&crdt_b, &board.id, None);
    assert_eq!(cards_b[0].title, "New Title", "B must see A's title");
    assert_eq!(
        cards_b[0].description.as_deref(),
        Some("Desc from B"),
        "B must see own description"
    );
}

#[test]
fn concurrent_move_and_edit_no_conflict() {
    // Client A moves the card (writes cpos only).
    // Client B edits the title (writes cmeta + cfields).
    // After merging, the new cpos (col2) and new title both survive.
    let crdt_a = CrdtExtension::new("a");
    let crdt_b = CrdtExtension::new("b");
    let ext = KanbanExtension;
    let conv_id = ConversationId::new();
    let board = KanbanBoard::new(conv_id, "Board");
    let col1 = KanbanColumn::new(board.id.clone(), "Todo", 0);
    let col2 = KanbanColumn::new(board.id.clone(), "Done", 1);
    let card = KanbanCard::new(col1.id.clone(), board.id.clone(), "Original", 0);

    // B starts from A's state so divergent writes are unambiguously newer than baseline.
    ext.upsert_card(&crdt_a, &card).unwrap();
    crdt_b.merge_raw(&crdt_a.full_state()).unwrap();

    // A moves the card to col2.
    ext.move_card(&crdt_a, &board.id, &card.id, col2.id.clone(), 0)
        .unwrap();

    // B edits the title.
    ext.patch_card_meta(
        &crdt_b,
        &board.id,
        &card.id,
        Some("Edited".into()),
        None,
        None,
        None,
        None,
        vec![],
    )
    .unwrap();

    // Full bidirectional CRDT merge — propagates cpos, cmeta, and cfields.
    let sv_a = crdt_a.state_vector();
    let sv_b = crdt_b.state_vector();
    crdt_a.merge_raw(&crdt_b.encode_diff(&sv_a)).unwrap();
    crdt_b.merge_raw(&crdt_a.encode_diff(&sv_b)).unwrap();

    // A: sees its own move + B's title edit.
    let cards_a = ext.list_cards(&crdt_a, &board.id, Some(&col2.id));
    assert_eq!(cards_a.len(), 1, "A must see card in col2");
    assert_eq!(cards_a[0].title, "Edited", "A must see B's title edit");

    // B: sees A's move + its own title edit.
    let cards_b = ext.list_cards(&crdt_b, &board.id, Some(&col2.id));
    assert_eq!(cards_b.len(), 1, "B must see card in col2 after merge");
    assert_eq!(cards_b[0].title, "Edited", "B must see its own title edit");
}

#[test]
fn concurrent_label_adds_no_conflict() {
    // Client A adds label "feature"; Client B adds label "bug" concurrently.
    // With per-label clabels entries both labels survive merge (C1 fix).
    let crdt_a = CrdtExtension::new("a");
    let crdt_b = CrdtExtension::new("b");
    let ext = KanbanExtension;
    let conv_id = ConversationId::new();
    let board = KanbanBoard::new(conv_id, "Board");
    let col = KanbanColumn::new(board.id.clone(), "Todo", 0);
    let card = KanbanCard::new(col.id.clone(), board.id.clone(), "Task", 0);

    ext.upsert_card(&crdt_a, &card).unwrap();
    crdt_b.merge_raw(&crdt_a.full_state()).unwrap();

    // A adds "feature" label.
    ext.add_card_label(&crdt_a, &board.id, &card.id, "feature");
    // B adds "bug" label.
    ext.add_card_label(&crdt_b, &board.id, &card.id, "bug");

    // CRDT merge.
    let sv_a = crdt_a.state_vector();
    let sv_b = crdt_b.state_vector();
    crdt_a.merge_raw(&crdt_b.encode_diff(&sv_a)).unwrap();
    crdt_b.merge_raw(&crdt_a.encode_diff(&sv_b)).unwrap();

    // Both labels must be present on both sides.
    let cards_a = ext.list_cards(&crdt_a, &board.id, None);
    assert_eq!(cards_a.len(), 1);
    let mut labels_a = cards_a[0].labels.clone();
    labels_a.sort();
    assert_eq!(
        labels_a,
        vec!["bug", "feature"],
        "both labels must survive merge"
    );

    let cards_b = ext.list_cards(&crdt_b, &board.id, None);
    let mut labels_b = cards_b[0].labels.clone();
    labels_b.sort();
    assert_eq!(
        labels_b,
        vec!["bug", "feature"],
        "both clients must converge on labels"
    );
}

#[test]
fn board_meta() {
    let crdt = CrdtExtension::new("c");
    let ext = KanbanExtension;
    let conv_id = ConversationId::new();
    let board = KanbanBoard::new(conv_id, "Board");

    ext.set_meta(&crdt, &board.id, "sprint", "1");
    assert_eq!(ext.get_meta(&crdt, &board.id, "sprint"), Some("1".into()));
    assert_eq!(ext.get_meta(&crdt, &board.id, "missing"), None);
}

#[test]
fn board_roundtrip_serde() {
    let conv_id = ConversationId::new();
    let board = KanbanBoard::new(conv_id.clone(), "Sprint 1");
    assert_eq!(board.name, "Sprint 1");
    assert_eq!(board.conversation_id, conv_id);

    let json = serde_json::to_string(&board).unwrap();
    let back: KanbanBoard = serde_json::from_str(&json).unwrap();
    assert_eq!(back.id, board.id);
}

#[tokio::test]
async fn push_pull_round_trip() {
    use crate::context::backend::InMemoryContextBackend;
    use crate::context::crdt::CrdtExtension;

    let backend = std::sync::Arc::new(InMemoryContextBackend::new());
    let conv = ConversationId::new();
    let branch = crate::context::types::BranchId::new();

    let writer_crdt = CrdtExtension::new("writer");
    let ext = KanbanExtension;
    let board = KanbanBoard::new(conv.clone(), "Board");
    let col = KanbanColumn::new(board.id.clone(), "Todo", 0);
    let card = KanbanCard::new(col.id.clone(), board.id.clone(), "Task", 0);

    ext.upsert_column(&writer_crdt, &col).unwrap();
    ext.upsert_card(&writer_crdt, &card).unwrap();
    writer_crdt
        .push(&conv, &branch, backend.as_ref())
        .await
        .unwrap();

    let reader_crdt = CrdtExtension::new("reader");
    reader_crdt
        .pull(&conv, &branch, backend.as_ref())
        .await
        .unwrap();

    let cols = ext.list_columns(&reader_crdt, &board.id);
    assert_eq!(cols.len(), 1);
    assert_eq!(cols[0].name, "Todo");

    let cards = ext.list_cards(&reader_crdt, &board.id, None);
    assert_eq!(cards.len(), 1);
    assert_eq!(cards[0].title, "Task");
}
