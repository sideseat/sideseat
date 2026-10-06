/// A stalled replica's cursor write is refused once another replica has moved it, so nothing is skipped.
///
/// Replica A reads and stalls; replica B advances the cursor; A's delayed write must become a no-op rather
/// than overwrite B and skip entries between the two positions.
///
/// Driven at the Redis level, because the race is in the cursor write rather than in the claim: two backends
/// share one cursor key, so a write conditioned on a stale read must not land.
#[tokio::test]
async fn a_stale_cursor_write_is_refused() {
    let Some((backend, topic)) = backend("cursor-cas").await else {
        return;
    };
    let group = "traces";
    backend
        .ensure_group_for_test(&topic, group)
        .await
        .expect("group");

    // Both instances read the same (absent) cursor, then one writes.
    let observed = backend
        .read_scan_cursor_for_test(&topic, group)
        .await
        .expect("read cursor");
    assert!(observed.is_none(), "a fresh group has no cursor");

    backend
        .write_scan_cursor_for_test(&topic, group, observed.clone(), Some("50-0"))
        .await
        .expect("the first write applies");
    // Stored as `<position>|<end>`, so the position is what identifies it.
    let after_first = backend
        .read_scan_cursor_for_test(&topic, group)
        .await
        .expect("read")
        .expect("a cursor was written");
    assert!(
        after_first.starts_with("50-0|"),
        "the first write should have set position 50-0, got {after_first}"
    );

    // The stalled instance now writes, still expecting the value it read before (absent). It must be refused.
    backend
        .write_scan_cursor_for_test(&topic, group, observed, Some("900-0"))
        .await
        .expect("the stale write must not error, only be refused");
    assert_eq!(
        backend
            .read_scan_cursor_for_test(&topic, group)
            .await
            .expect("read"),
        Some(after_first),
        "a write conditioned on a stale read must not move the cursor - it would skip everything between"
    );
}

/// Rotation reaches later entries even while a peer keeps re-claiming the first one.
///
/// A peer repeatedly makes the first entry too fresh for the rescuer. Fixed-endpoint rotation must continue
/// across every examined entry instead of pinning the cursor there.
///
/// The adversary is a **direct `XCLAIM` of the first id**, not another rotating claim: the rotating one would
/// simply advance like any consumer.
#[tokio::test]
async fn rotation_reaches_later_entries_past_a_repeatedly_reclaimed_one() {
    let Some((backend, topic)) = backend("rotation-past").await else {
        return;
    };
    let group = "traces";
    backend
        .ensure_group_for_test(&topic, group)
        .await
        .expect("group");

    let mut ids = Vec::new();
    for i in 0..4u32 {
        ids.push(
            backend
                .stream_publish(&topic, "test-key", format!("entry-{i}").as_bytes())
                .await
                .expect("publish"),
        );
    }
    let mut subscription = backend
        .stream_subscribe(&topic, group, "doomed")
        .await
        .expect("subscribe");
    for _ in 0..ids.len() {
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            futures::StreamExt::next(&mut subscription.receiver),
        )
        .await
        .expect("delivered")
        .expect("open")
        .expect("ok");
    }
    drop(subscription);

    let pinned = ids[0].clone();
    let last = ids[ids.len() - 1].clone();

    // Let every entry age past the threshold the rescuer will use, so all of them start out claimable.
    const IDLE_THRESHOLD_MS: u64 = 400;
    tokio::time::sleep(std::time::Duration::from_millis(IDLE_THRESHOLD_MS + 200)).await;

    // Each pass the peer re-claims the first entry, resetting its idle time below the rescuer's threshold.
    let mut saw_last = false;
    for _ in 0..12 {
        backend
            .claim_specific_for_test(&topic, group, "flapping-peer", &pinned)
            .await
            .expect("peer re-claims the first entry");
        let claimed = backend
            .stream_claim(&topic, group, "rescuer", IDLE_THRESHOLD_MS, 1)
            .await
            .expect("claim");
        assert!(
            !claimed.iter().any(|m| m.id == pinned),
            "the re-claimed entry must be too fresh for the rescuer's threshold, or the test stages no \
             unclaimable entry at all"
        );
        if claimed.iter().any(|m| m.id == last) {
            saw_last = true;
            break;
        }
    }
    assert!(
        saw_last,
        "rotation must reach the last entry while a peer keeps re-claiming {pinned}; a cursor pinned there \
         would starve everything after it"
    );
}

/// A malformed cursor value is replaced, not treated as an immovable obstacle.
///
/// The empty string represents an existing malformed value, not absence. Compare-and-set must replace it so
/// rotation can progress beyond the first page.
#[tokio::test]
async fn a_malformed_cursor_value_is_replaced() {
    let Some((backend, topic)) = backend("cursor-malformed").await else {
        return;
    };
    let group = "traces";
    backend
        .ensure_group_for_test(&topic, group)
        .await
        .expect("group");

    // More entries than one page, so a stuck cursor would visibly starve the tail.
    let mut ids = Vec::new();
    for i in 0..4u32 {
        ids.push(
            backend
                .stream_publish(&topic, "test-key", format!("entry-{i}").as_bytes())
                .await
                .expect("publish"),
        );
    }
    let mut subscription = backend
        .stream_subscribe(&topic, group, "doomed")
        .await
        .expect("subscribe");
    for _ in 0..ids.len() {
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            futures::StreamExt::next(&mut subscription.receiver),
        )
        .await
        .expect("delivered")
        .expect("open")
        .expect("ok");
    }
    drop(subscription);

    for malformed in ["", "not-a-rotation", "50-0", "|", "50-0|"] {
        backend
            .set_raw_scan_cursor_for_test(&topic, group, malformed)
            .await
            .expect("stage the malformed cursor");

        // One pass with a window of one: it must both do work and move the cursor off the bad value.
        let claimed = backend
            .stream_claim(&topic, group, "rescuer", 0, 1)
            .await
            .expect("claim");
        assert_eq!(
            claimed.len(),
            1,
            "a pass starting from a malformed cursor ({malformed:?}) must still claim"
        );
        let after = backend
            .read_scan_cursor_for_test(&topic, group)
            .await
            .expect("read cursor");
        assert_ne!(
            after.as_deref(),
            Some(malformed),
            "the malformed cursor {malformed:?} must be replaced, or rotation is stuck on it forever"
        );
    }
}
