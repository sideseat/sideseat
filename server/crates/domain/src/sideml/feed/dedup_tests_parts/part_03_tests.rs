/// A result keeps the failure a dropped copy reported. OpenTelemetry's botocore instrumentation carries a
/// failed tool's result as a tool message, without a status, and inside the next request's user turn, with
/// one; the copy that won on quality read as a success.
#[test]
fn a_result_keeps_the_failure_a_dropped_copy_reported() {
    let current = make_tool_result_block("t1", "s1", "call_1", "No seats", utc(100));
    let mut resent = make_tool_result_block("t1", "s1", "call_1", "No seats", utc(100));
    resent.is_history = true;
    if let ContentBlock::ToolResult { is_error, .. } = &mut resent.content {
        *is_error = true;
    }
    let result = process_dedup(vec![current, resent], HashMap::new());
    assert_eq!(result.len(), 1);
    assert!(
        matches!(result[0].content, ContentBlock::ToolResult { is_error: true, .. }),
        "{:?}",
        result[0].content
    );
}
