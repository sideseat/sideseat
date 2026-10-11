//! A span's log records, joined to it at read time.

use super::*;

/// The span's own raw messages followed by those its log records carry.
///
/// Log messages are read-time joined rather than stored on the span, and they are the same raw-message shape
/// a span event produces, so appending them is all attaching takes: normalisation, role derivation and
/// deduplication then treat a log event exactly as the span event of that name. A producer that writes a
/// message both ways wrote one message on two carriers, and normalisation reads it once
/// (`normalize::signals`), which is why the span's messages come first: where they end is where the records'
/// begin.
///
/// Tool-span enrichment - copying the span's tool name and call id onto its tool messages - happens when a
/// span's *events* are extracted and is not repeated here, because the row carries no tool attributes. A
/// log event on a tool execution span therefore keeps only what the record itself says.
pub(super) fn with_log_messages(
    row: &MessageSpanRow,
    span_messages: Option<Arc<Vec<RawMessage>>>,
) -> Option<Arc<Vec<RawMessage>>> {
    if row.log_messages_json.is_empty() || row.log_messages_json == "[]" {
        return span_messages;
    }
    match serde_json::from_str::<Vec<RawMessage>>(&row.log_messages_json) {
        Ok(log_messages) if !log_messages.is_empty() => {
            let mut all = span_messages
                .map(|messages| messages.as_ref().clone())
                .unwrap_or_default();
            all.extend(log_messages);
            Some(Arc::new(all))
        }
        Ok(_) => span_messages,
        Err(error) => {
            tracing::debug!(
                span_id = %row.span_id,
                %error,
                "Failed to parse log messages JSON"
            );
            span_messages
        }
    }
}
