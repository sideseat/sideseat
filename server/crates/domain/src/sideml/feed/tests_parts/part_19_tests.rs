// ============================================================================
// Redaction markers (`redaction`): what reaches the copy that finally stands for a statement
// ============================================================================

/// A copy of another tool winning for a while does not decide what reaches the copy that stays: the report of a
/// lookup marker reaches the best lookup result whether or not a write result held the place in between.
#[test]
fn a_report_reaches_the_settled_survivor_past_another_tool() {
    let result_row = |span: &str, content: &str, tool: &str, rank: u8| {
        let marker = content == MARKER;
        let block = json!({"type": "tool_result", "tool_use_id": "call-1", "name": tool, "content": content, "provider_executed": marker});
        // The best copy is the one an event carried.
        let source = if rank > 1 {
            json!({"event": {"name": "gen_ai.tool.message", "time": "2025-01-01T00:00:00Z"}})
        } else {
            json!({"attribute": {"key": "output", "time": "2025-01-01T00:00:00Z"}})
        };
        let messages = json!([{
            "source": source,
            "content": {"role": "tool", "content": [block]}
        }]);
        let mut row = make_span_row("trace1", span, None, &messages.to_string(), "[]", "[]");
        row.scope_name = Some(SCRUBBING_SCOPE.to_string());
        if marker {
            row.status_code = Some(status::ERROR.to_string());
        }
        if rank > 0 {
            row.observation_type = Some("tool".to_string());
        }
        row
    };
    let marker = result_row("span1", MARKER, "lookup", 0);
    let write = result_row("span2", "Wrote it.", "write", 1);
    let lookup = result_row("span3", "Found it.", "lookup", 2);
    for rows in [
        vec![marker.clone(), write.clone(), lookup.clone()],
        vec![lookup.clone(), write.clone(), marker.clone()],
        vec![write.clone(), marker.clone(), lookup.clone()],
    ] {
        let found: Vec<BlockEntry> = process_spans(rows, &FeedOptions::new())
            .messages
            .into_iter()
            .filter(|b| {
                serde_json::to_string(&b.content)
                    .unwrap_or_default()
                    .contains("Found it.")
            })
            .collect();
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].is_error, "{found:?}");
        assert!(
            matches!(
                found[0].content,
                ContentBlock::ToolResult {
                    provider_executed: true,
                    ..
                }
            ),
            "{found:?}"
        );
    }
}

/// Copies of one call naming two tools between them are decided as before markers, by an order no third copy can
/// undo: whichever order a lookup value, a write value and a lookup marker of equal quality arrive in, one copy
/// survives.
#[test]
fn a_tie_across_tools_is_decided_alike_in_every_order() {
    // The order the copies arrive in follows their times and spans, so both are permuted.
    let result_row = |span: &str, content: &str, tool: &str, kind: Option<&str>, second: usize| {
        let block = json!({"type": "tool_result", "tool_use_id": "call-1", "name": tool, "content": content});
        let messages = json!([{
            "source": {"attribute": {"key": "output", "time": format!("2025-01-01T00:00:0{second}Z")}},
            "content": {"role": "tool", "content": [block]}
        }]);
        let mut row = make_span_row("trace1", span, None, &messages.to_string(), "[]", "[]");
        row.scope_name = Some(SCRUBBING_SCOPE.to_string());
        row.observation_type = kind.map(str::to_string);
        row
    };
    let orders = [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ];
    let survivors: Vec<Vec<String>> = orders
        .iter()
        .map(|order| {
            let at = |copy: usize| order.iter().position(|&i| i == copy).unwrap_or(0) + 1;
            let span = |copy: usize| format!("span{}", at(copy));
            let rows = vec![
                result_row(&span(0), "Found it.", "lookup", None, at(0)),
                result_row(&span(1), "Wrote it.", "write", Some("generation"), at(1)),
                result_row(&span(2), MARKER, "lookup", Some("agent"), at(2)),
            ];
            process_spans(rows, &FeedOptions::new())
                .messages
                .iter()
                .filter(|b| b.entry_type == "tool_result")
                .map(|b| serde_json::to_string(&b.content).unwrap_or_default())
                .collect()
        })
        .collect();
    assert!(survivors.windows(2).all(|w| w[0] == w[1]), "{survivors:?}");
    assert_eq!(survivors[0].len(), 1, "{survivors:?}");
}
