#[test]
fn test_tool_result_same_id_different_content_deduped() {
    // Same tool_use_id → same identity, regardless of content format.
    // Covers Vercel AI SDK toModelOutput: raw execute() output vs transformed format.
    let t0 = utc(0);
    let t1 = utc(1);

    // Raw tool result from tool span
    let mut raw = make_tool_result_block("trace1", "tool_span", "call_123", "raw result data", t0);
    raw.observation_type = Some("tool".to_string());

    // Transformed tool result from generation span (different content, same tool_use_id)
    let mut transformed = BlockEntry {
        content: ContentBlock::ToolResult {
            tool_use_id: Some("call_123".to_string()),
            name: None,
            content: serde_json::json!({"type": "content", "value": [{"type": "text", "text": "transformed result"}]}),
            is_error: false,
            provider_executed: false,
        },
        span_id: "gen_span".to_string(),
        timestamp: t1,
        observation_type: Some("generation".to_string()),
        ..make_tool_result_block("trace1", "gen_span", "call_123", "", t1)
    };
    transformed.model = Some("claude-haiku".to_string());

    let span_timestamps = HashMap::from([
        (
            "tool_span".to_string(),
            SpanTimestamps {
                span_start: t0,
                span_end: Some(t0),
            },
        ),
        (
            "gen_span".to_string(),
            SpanTimestamps {
                span_start: t1,
                span_end: Some(t1),
            },
        ),
    ]);

    let result = process_dedup(vec![raw, transformed], span_timestamps);

    // Same tool_use_id → deduped to 1 (tool_use_id is identity, not content)
    assert_eq!(result.len(), 1);
    // Tool span version wins via FROM_TOOL_SPAN quality bonus
    assert_eq!(result[0].span_id, "tool_span");
}

#[test]
fn test_tool_result_different_ids_not_deduped_by_tool_id() {
    // Different tool_use_ids should NOT be merged even if both are tool results
    let t0 = utc(0);

    let result1 = make_tool_result_block("trace1", "span1", "call_111", "result A", t0);
    let result2 = make_tool_result_block("trace1", "span2", "call_222", "result B", t0);

    let span_timestamps = HashMap::from([
        (
            "span1".to_string(),
            SpanTimestamps {
                span_start: t0,
                span_end: Some(t0),
            },
        ),
        (
            "span2".to_string(),
            SpanTimestamps {
                span_start: t0,
                span_end: Some(t0),
            },
        ),
    ]);

    let result = process_dedup(vec![result1, result2], span_timestamps);

    // Both should be kept (different tool_use_ids = different identities)
    assert_eq!(result.len(), 2);
}

/// The band space is bounded below `own_output`'s bit: a trace and carrier past 2^14 delta spans folds the
/// overflow onto its last band - a deliberate bound, so adversarial span counts degrade within their own
/// group instead of colliding with another pass's namespace.
#[test]
fn delta_bands_past_their_space_fold_onto_the_last_band() {
    const LAST_BAND: usize = (1 << 14) - 1;
    let threads: Vec<String> = (0..=LAST_BAND + 1).map(|n| format!("th-{n}")).collect();
    let spans: Vec<String> = (0..=LAST_BAND + 1).map(|n| format!("s{n}")).collect();
    let blocks: Vec<BlockEntry> = (0..=LAST_BAND + 1)
        .map(|n| {
            threaded_delta_block(
                &threads[n],
                &spans[n],
                0,
                "Tokens left: 1",
                utc(100 + n as i64),
            )
        })
        .collect();
    let survivors = process_dedup(blocks, HashMap::new());
    assert_eq!(
        survivors.len(),
        LAST_BAND + 1,
        "the span past the band space folds onto the last band"
    );
}

/// A resumption restates the thread's whole history, not only the previous delta: after `[A]` and `[B]`, a
/// resumed turn says `[A, B, C]`, and A and B fold onto the originals while only C is new. Compared against
/// the previous delta alone, the restated A and B each showed twice.
#[test]
fn a_resumption_restates_the_thread_s_whole_history() {
    let survivors = process_dedup(
        vec![
            delta_block("s1", 0, "Env: /tmp", utc(100)),
            delta_block("s2", 0, "Tokens left: 2", utc(200)),
            delta_block("s3", 0, "Env: /tmp", utc(300)),
            delta_block("s3", 1, "Tokens left: 2", utc(300)),
            delta_block("s3", 2, "Tokens left: 1", utc(300)),
        ],
        HashMap::new(),
    );
    assert_eq!(survivors.len(), 3, "{survivors:?}");
}

/// A restated member lands exactly on its original's ordinal, rank included: after `[A]` and `[A]` - two
/// statements, the equal lists being fresh deliveries - a resumption `[A, A, B]` restates both. Its second A
/// sits at a different position than the original did, so inheriting only the band left it one rank off and
/// a fourth statement appeared.
#[test]
fn a_restated_member_lands_on_its_original_s_rank() {
    let survivors = process_dedup(
        vec![
            delta_block("s1", 0, "Tokens left: 1", utc(100)),
            delta_block("s2", 0, "Tokens left: 1", utc(200)),
            delta_block("s3", 0, "Tokens left: 1", utc(300)),
            delta_block("s3", 1, "Tokens left: 1", utc(300)),
            delta_block("s3", 2, "Env: /tmp", utc(300)),
        ],
        HashMap::new(),
    );
    assert_eq!(survivors.len(), 3, "{survivors:?}");
}
