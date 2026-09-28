use super::*;
use crate::context::crdt::CrdtExtension;

fn make_item(canvas_id: &CanvasId, id: &str, x: f64, y: f64) -> CanvasItem {
    CanvasItem {
        id: id.into(),
        canvas_id: canvas_id.clone(),
        item_type: CanvasItemType::Sticky,
        x,
        y,
        width: 100.0,
        height: 80.0,
        z_index: 0,
        rotation: 0.0,
        parent_item_id: None,
        content: CanvasItemContent::RichText {
            text: "hello".into(),
            format: None,
        },
        style: Value::Null,
        created_at: now_micros(),
        created_by: None,
        locked: false,
        deleted: false,
    }
}

#[test]
fn upsert_and_list() {
    let crdt = CrdtExtension::new("c");
    let ext = CanvasExtension;
    let canvas_id = CanvasId::new();

    let item = make_item(&canvas_id, "item-1", 10.0, 20.0);
    ext.upsert_item(&crdt, &item).unwrap();

    let items = ext.list_items(&crdt, &canvas_id, None);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, "item-1");
}

#[test]
fn tombstone_removal() {
    let crdt = CrdtExtension::new("c");
    let ext = CanvasExtension;
    let canvas_id = CanvasId::new();

    ext.upsert_item(&crdt, &make_item(&canvas_id, "item-1", 0.0, 0.0))
        .unwrap();
    ext.remove_item(&crdt, &canvas_id, "item-1");

    let items = ext.list_items(&crdt, &canvas_id, None);
    assert!(items.is_empty(), "tombstoned item must be filtered");
}

#[test]
fn remove_item_only_updates_prop() {
    let crdt = CrdtExtension::new("c");
    let ext = CanvasExtension;
    let canvas_id = CanvasId::new();

    ext.upsert_item(&crdt, &make_item(&canvas_id, "item-1", 5.0, 10.0))
        .unwrap();
    ext.remove_item(&crdt, &canvas_id, "item-1");

    // Geo map must still have the entry (remove_item only touches pdel).
    let geo_entries = crdt.map_entries(&CanvasExtension::geo_map(&canvas_id));
    assert!(
        geo_entries.contains_key("item-1"),
        "geo must survive remove_item"
    );

    // pdel map must have "true".
    let deleted = crdt
        .map_get(&CanvasExtension::prop_deleted_map(&canvas_id), "item-1")
        .unwrap();
    assert_eq!(deleted, "true", "pdel must be 'true' after remove_item");

    // List must return empty (item is tombstoned).
    assert!(ext.list_items(&crdt, &canvas_id, None).is_empty());
}

#[test]
fn upsert_idempotent() {
    let crdt = CrdtExtension::new("c");
    let ext = CanvasExtension;
    let canvas_id = CanvasId::new();

    ext.upsert_item(&crdt, &make_item(&canvas_id, "item-1", 0.0, 0.0))
        .unwrap();
    ext.upsert_item(&crdt, &make_item(&canvas_id, "item-1", 5.0, 5.0))
        .unwrap(); // overwrite

    let items = ext.list_items(&crdt, &canvas_id, None);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].x, 5.0);
}

#[test]
fn viewport_filter() {
    let crdt = CrdtExtension::new("c");
    let ext = CanvasExtension;
    let canvas_id = CanvasId::new();

    ext.upsert_item(&crdt, &make_item(&canvas_id, "visible", 50.0, 50.0))
        .unwrap();
    ext.upsert_item(&crdt, &make_item(&canvas_id, "outside", 2000.0, 2000.0))
        .unwrap();

    let vp = Viewport {
        x: 0.0,
        y: 0.0,
        width: 500.0,
        height: 500.0,
    };
    let items = ext.list_items(&crdt, &canvas_id, Some(&vp));
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, "visible");
}

#[test]
fn meta_set_get() {
    let crdt = CrdtExtension::new("c");
    let ext = CanvasExtension;
    let canvas_id = CanvasId::new();

    ext.set_meta(&crdt, &canvas_id, "zoom", "1.5");
    assert_eq!(ext.get_meta(&crdt, &canvas_id, "zoom"), Some("1.5".into()));
    assert_eq!(ext.get_meta(&crdt, &canvas_id, "missing"), None);
}

#[test]
fn viewport_contains_item_edges() {
    let vp = Viewport {
        x: 0.0,
        y: 0.0,
        width: 1000.0,
        height: 1000.0,
    };
    let canvas_id = CanvasId::new();

    let inside = make_item(&canvas_id, "i", 100.0, 100.0);
    let outside = CanvasItem {
        x: 2000.0,
        y: 2000.0,
        ..make_item(&canvas_id, "o", 0.0, 0.0)
    };

    assert!(vp.contains_item(&inside));
    assert!(!vp.contains_item(&outside));
}

#[test]
fn list_items_sorted_by_z_index() {
    let crdt = CrdtExtension::new("c");
    let ext = CanvasExtension;
    let canvas_id = CanvasId::new();

    let mut top = make_item(&canvas_id, "top", 0.0, 0.0);
    top.z_index = 10;
    let mut mid = make_item(&canvas_id, "mid", 0.0, 0.0);
    mid.z_index = 5;
    let mut bot = make_item(&canvas_id, "bot", 0.0, 0.0);
    bot.z_index = 1;

    ext.upsert_item(&crdt, &top).unwrap();
    ext.upsert_item(&crdt, &mid).unwrap();
    ext.upsert_item(&crdt, &bot).unwrap();

    let items = ext.list_items(&crdt, &canvas_id, None);
    assert_eq!(items.len(), 3);
    assert_eq!(items[0].id, "bot");
    assert_eq!(items[1].id, "mid");
    assert_eq!(items[2].id, "top");
}

#[test]
fn concurrent_locked_and_style_no_conflict() {
    // Client A locks the item; Client B updates the style concurrently.
    // With per-field maps each write targets only its own map entry, so
    // both changes survive merge — no LWW collision (C1 fix).
    let ext = CanvasExtension;
    let canvas_id = CanvasId::new();

    let crdt_a = CrdtExtension::new("a");
    let item = make_item(&canvas_id, "item-1", 0.0, 0.0);
    ext.upsert_item(&crdt_a, &item).unwrap();

    let crdt_b = CrdtExtension::new("b");
    crdt_b.merge_raw(&crdt_a.full_state()).unwrap();

    // A locks the item — targeted single-field write to plck only.
    crdt_a.map_set(
        &CanvasExtension::prop_locked_map(&canvas_id),
        "item-1",
        "true",
    );

    // B updates the style — per-property write to psty (key = "{item_id}:{prop}").
    // Concurrent writes to different properties don't clobber each other (C1 fix).
    let style = serde_json::json!({"background": "blue"});
    crdt_b.map_set(
        &CanvasExtension::prop_style_map(&canvas_id),
        "item-1:background",
        &serde_json::to_string(&style["background"]).unwrap(),
    );

    // CRDT merge: A gets B's style write, B gets A's locked write.
    let sv_a = crdt_a.state_vector();
    let sv_b = crdt_b.state_vector();
    crdt_a.merge_raw(&crdt_b.encode_diff(&sv_a)).unwrap();
    crdt_b.merge_raw(&crdt_a.encode_diff(&sv_b)).unwrap();

    // After merge: A's lock AND B's style both survive.
    let items_a = ext.list_items(&crdt_a, &canvas_id, None);
    assert_eq!(items_a.len(), 1);
    assert!(items_a[0].locked, "locked must survive");
    assert_eq!(items_a[0].style, style, "style must survive");

    let items_b = ext.list_items(&crdt_b, &canvas_id, None);
    assert_eq!(items_b.len(), 1);
    assert!(items_b[0].locked, "locked must survive after merge");
    assert_eq!(items_b[0].style, style, "style must survive after merge");
}

#[test]
fn concurrent_move_and_edit_no_conflict() {
    // Client A drags the item (writes geo only).
    // Client B edits the text (writes Y.Text + cnt marker).
    // After a proper CRDT merge, both geo and text survive.
    //
    // Note: B must start from A's state so that B's Y.Text delete-all targets
    // A's character IDs; otherwise the two independent "hello" insertions would
    // produce "helloupdated" after merge (each client's chars are distinct).
    let ext = CanvasExtension;
    let canvas_id = CanvasId::new();

    // A creates the initial item.
    let crdt_a = CrdtExtension::new("a");
    let item = make_item(&canvas_id, "item-1", 0.0, 0.0);
    ext.upsert_item(&crdt_a, &item).unwrap();

    // B starts from A's state (simulates pull/checkout from shared baseline).
    let crdt_b = CrdtExtension::new("b");
    crdt_b.merge_raw(&crdt_a.full_state()).unwrap();

    // A moves the item (geo only — no Y.Text change).
    ext.move_item(
        &crdt_a, &canvas_id, "item-1", 100.0, 200.0, 120.0, 90.0, 0.0,
    )
    .unwrap();

    // B edits the text (Y.Text: delete A's "hello", insert "updated").
    let new_content = CanvasItemContent::RichText {
        text: "updated".into(),
        format: None,
    };
    ext.patch_item_content(&crdt_b, &canvas_id, "item-1", &new_content)
        .unwrap();

    // CRDT merge: exchange only the new ops (encode_diff against the other's sv).
    let sv_a = crdt_a.state_vector();
    let sv_b = crdt_b.state_vector();
    crdt_a.merge_raw(&crdt_b.encode_diff(&sv_a)).unwrap(); // A gets B's text changes
    crdt_b.merge_raw(&crdt_a.encode_diff(&sv_b)).unwrap(); // B gets A's geo changes

    // Both see x=100 (A's move) AND text="updated" (B's edit).
    let items_a = ext.list_items(&crdt_a, &canvas_id, None);
    assert_eq!(items_a.len(), 1);
    assert_eq!(items_a[0].x, 100.0, "A must see moved x");
    match &items_a[0].content {
        CanvasItemContent::RichText { text, .. } => assert_eq!(text, "updated"),
        _ => panic!("expected RichText"),
    }

    let items_b = ext.list_items(&crdt_b, &canvas_id, None);
    assert_eq!(items_b.len(), 1);
    assert_eq!(items_b[0].x, 100.0, "B must see moved x after merge");
    match &items_b[0].content {
        CanvasItemContent::RichText { text, .. } => assert_eq!(text, "updated"),
        _ => panic!("expected RichText"),
    }
}

#[test]
fn concurrent_drag_and_zreorder_no_conflict() {
    // Client A drags the item (writes geo only).
    // Client B brings it to front (writes zgeo only).
    // After merge, both new position AND new z_index survive.
    let ext = CanvasExtension;
    let canvas_id = CanvasId::new();

    let crdt_a = CrdtExtension::new("a");
    let item = make_item(&canvas_id, "item-1", 0.0, 0.0);
    ext.upsert_item(&crdt_a, &item).unwrap();

    let crdt_b = CrdtExtension::new("b");
    crdt_b.merge_raw(&crdt_a.full_state()).unwrap();

    // A drags (geo only — no z_index change).
    ext.move_item(
        &crdt_a, &canvas_id, "item-1", 300.0, 400.0, 100.0, 80.0, 0.0,
    )
    .unwrap();

    // B brings to front (zgeo only — no position change).
    ext.set_z_index(&crdt_b, &canvas_id, "item-1", 99).unwrap();

    // CRDT merge.
    let sv_a = crdt_a.state_vector();
    let sv_b = crdt_b.state_vector();
    crdt_a.merge_raw(&crdt_b.encode_diff(&sv_a)).unwrap();
    crdt_b.merge_raw(&crdt_a.encode_diff(&sv_b)).unwrap();

    // Both A's new position AND B's z_index must survive on both sides.
    for (label, crdt) in [("A", &crdt_a), ("B", &crdt_b)] {
        let items = ext.list_items(crdt, &canvas_id, None);
        assert_eq!(items.len(), 1, "{label}: item must survive");
        assert_eq!(items[0].x, 300.0, "{label}: A's x must survive");
        assert_eq!(items[0].y, 400.0, "{label}: A's y must survive");
        assert_eq!(items[0].z_index, 99, "{label}: B's z_index must survive");
    }
}

#[test]
fn concurrent_text_inserts_converge() {
    // Two users type at different positions in the same sticky note simultaneously.
    // With Y.Text both insertions survive (character-level CRDT, not LWW).
    let ext = CanvasExtension;
    let canvas_id = CanvasId::new();

    // A creates the item with initial text "hello".
    let crdt_a = CrdtExtension::new("a");
    let item = make_item(&canvas_id, "note", 0.0, 0.0);
    ext.upsert_item(&crdt_a, &item).unwrap();

    // B starts from A's state.
    let crdt_b = CrdtExtension::new("b");
    crdt_b.merge_raw(&crdt_a.full_state()).unwrap();

    // A inserts " world" at position 5 (end): "hello world"
    ext.insert_rich_text(&crdt_a, &canvas_id, "note", 5, " world");

    // B inserts "!" at position 5 (end before A's addition): "hello!"
    ext.insert_rich_text(&crdt_b, &canvas_id, "note", 5, "!");

    // CRDT merge.
    let sv_a = crdt_a.state_vector();
    let sv_b = crdt_b.state_vector();
    crdt_a.merge_raw(&crdt_b.encode_diff(&sv_a)).unwrap();
    crdt_b.merge_raw(&crdt_a.encode_diff(&sv_b)).unwrap();

    // Both converge to the same text (order of concurrent inserts at position 5
    // is deterministic via Lamport clock; content of both inserts is preserved).
    let text_a = crdt_a.text_read(&CanvasExtension::rich_text_key(&canvas_id, "note"));
    let text_b = crdt_b.text_read(&CanvasExtension::rich_text_key(&canvas_id, "note"));
    assert_eq!(
        text_a, text_b,
        "concurrent text inserts must converge to identical state"
    );
    assert!(text_a.contains("hello"), "original text must be preserved");
    assert!(text_a.contains(" world"), "A's insert must be preserved");
    assert!(text_a.contains('!'), "B's insert must be preserved");
}

#[test]
fn legacy_dual_read() {
    // Items in the old single-blob map are included when no prop entry exists.
    let crdt = CrdtExtension::new("c");
    let ext = CanvasExtension;
    let canvas_id = CanvasId::new();

    let item = make_item(&canvas_id, "legacy-item", 5.0, 5.0);
    let json = serde_json::to_string(&item).unwrap();
    crdt.map_set(
        &format!("canvas:{}:items", canvas_id.as_str()),
        "legacy-item",
        &json,
    );

    let items = ext.list_items(&crdt, &canvas_id, None);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, "legacy-item");
}

#[test]
fn migration_v1_to_v2() {
    let crdt = CrdtExtension::new("c");
    let ext = CanvasExtension;
    let canvas_id = CanvasId::new();

    // Seed legacy map.
    let item = make_item(&canvas_id, "item-1", 3.0, 7.0);
    let json = serde_json::to_string(&item).unwrap();
    crdt.map_set(
        &format!("canvas:{}:items", canvas_id.as_str()),
        "item-1",
        &json,
    );

    ext.migrate_canvas_v1_to_v2(&crdt, &canvas_id);

    // After migration, geo/pimeta/cnt maps must have the item.
    assert!(
        crdt.map_get(&CanvasExtension::geo_map(&canvas_id), "item-1")
            .is_some()
    );
    assert!(
        crdt.map_get(&CanvasExtension::prop_imeta_map(&canvas_id), "item-1")
            .is_some()
    );
    assert!(
        crdt.map_get(&CanvasExtension::cnt_map(&canvas_id), "item-1")
            .is_some()
    );

    // Idempotent — second migration does not change anything.
    ext.migrate_canvas_v1_to_v2(&crdt, &canvas_id);
    let items = ext.list_items(&crdt, &canvas_id, None);
    assert_eq!(items.len(), 1);
}

#[tokio::test]
async fn push_pull_round_trip() {
    use crate::context::backend::InMemoryContextBackend;
    use crate::context::crdt::CrdtExtension;

    let backend = std::sync::Arc::new(InMemoryContextBackend::new());
    let conv = ConversationId::new();
    let branch = crate::context::types::BranchId::new();

    // Writer: upsert an item and push.
    let writer_crdt = CrdtExtension::new("writer");
    let canvas_id = CanvasId::new();
    let ext = CanvasExtension;
    ext.upsert_item(&writer_crdt, &make_item(&canvas_id, "sync-item", 5.0, 5.0))
        .unwrap();
    writer_crdt
        .push(&conv, &branch, backend.as_ref())
        .await
        .unwrap();

    // Reader: pull and verify item is visible.
    let reader_crdt = CrdtExtension::new("reader");
    reader_crdt
        .pull(&conv, &branch, backend.as_ref())
        .await
        .unwrap();

    let items = ext.list_items(&reader_crdt, &canvas_id, None);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, "sync-item");
}
