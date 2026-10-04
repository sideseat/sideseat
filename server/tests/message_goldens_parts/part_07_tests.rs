fn occurrence_row(index: usize, position: &str, proves_occurrence: bool) -> InvariantRow {
    InvariantRow {
        trace_id: "trace-a".to_string(),
        span_id: "span-1".to_string(),
        span_path: vec!["span-1".to_string()],
        index,
        role: "user".to_string(),
        entry_type: "text".to_string(),
        content: "same turn".to_string(),
        content_digest: "same-digest".to_string(),
        occurrence_ordinal: index as u32,
        tool_use_id: None,
        carrier: "attr:ordered-conversation".to_string(),
        carrier_orders_positions: true,
        carrier_proves_occurrence: proves_occurrence,
        order_time: chrono::DateTime::UNIX_EPOCH,
        position: position.to_string(),
    }
}

fn reused_id_row(index: usize, kind: &str) -> InvariantRow {
    InvariantRow {
        trace_id: "trace-a".to_string(),
        span_id: "span-1".to_string(),
        span_path: vec!["span-1".to_string()],
        index,
        role: if kind == "tool_use" {
            "assistant"
        } else {
            "tool"
        }
        .to_string(),
        entry_type: kind.to_string(),
        content: "{\"id\":\"reused\"}".to_string(),
        content_digest: format!("{kind}-{index}"),
        occurrence_ordinal: (index / 2) as u32,
        tool_use_id: Some("reused".to_string()),
        carrier: "attr:test".to_string(),
        carrier_orders_positions: true,
        carrier_proves_occurrence: true,
        order_time: chrono::DateTime::UNIX_EPOCH,
        position: index.to_string(),
    }
}

#[test]
fn duplicate_invariant_accepts_only_proven_distinct_occurrences() {
    let fires = |rows: &[InvariantRow]| {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            assert_no_duplicates("test", "synthetic", rows);
        }))
        .is_err()
    };

    let distinct = vec![
        occurrence_row(0, "2.0", true),
        occurrence_row(1, "3.0", true),
    ];
    assert!(
        !fires(&distinct),
        "an ordered carrier's distinct positions are distinct occurrences"
    );

    let repeated_delivery = vec![
        occurrence_row(0, "2.0", true),
        occurrence_row(1, "2.0", true),
    ];
    assert!(
        fires(&repeated_delivery),
        "the same proven position delivered twice is still a duplicate"
    );

    let unproven = vec![
        occurrence_row(0, "2.0", true),
        occurrence_row(1, "3.0", false),
    ];
    assert!(
        fires(&unproven),
        "every retained repeat must carry occurrence evidence"
    );

    let mut separate_executions = vec![
        occurrence_row(0, "0.0", false),
        occurrence_row(1, "0.0", false),
    ];
    separate_executions[1].span_id = "span-2".to_string();
    separate_executions[1].span_path = vec!["span-2".to_string()];
    assert!(
        !fires(&separate_executions),
        "distinct execution spans with distinct ranks are distinct occurrences"
    );

    separate_executions[1].occurrence_ordinal = 0;
    assert!(
        fires(&separate_executions),
        "different spans without different occurrence ranks may only be re-listing one request"
    );
}

#[test]
fn reused_tool_ids_are_paired_by_occurrence() {
    let ordered = vec![
        reused_id_row(0, "tool_use"),
        reused_id_row(1, "tool_result"),
        reused_id_row(2, "tool_use"),
        reused_id_row(3, "tool_result"),
    ];
    assert_tool_pairing("test", "synthetic", &ordered);
    assert_tool_causality("test", "synthetic", &ordered);

    let misordered = vec![
        reused_id_row(0, "tool_use"),
        reused_id_row(1, "tool_result"),
        reused_id_row(2, "tool_result"),
        reused_id_row(3, "tool_use"),
    ];
    assert_tool_pairing("test", "synthetic", &misordered);
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            assert_tool_causality("test", "synthetic", &misordered);
        }))
        .is_err(),
        "a second result cannot consume the first occurrence's already-used call"
    );
}
