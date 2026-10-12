//! A producer's redaction placeholders, compiled from `redaction_markers`.
//!
//! Read-time only. A block whose whole content is a declared marker is a copy of a value its producer scrubbed,
//! and yields to an unscrubbed twin of the same statement. Raw keeps both copies; nothing is withdrawn from it.

use std::collections::{BTreeSet, HashMap};

use crate::sideml::ContentBlock;

use super::diagnostics::ClauseDefect;
use super::schema::RuleFile;
use super::span_conditions::{self, Readable, SpanExpr, SpanSubject};

/// The one placeholder a marker may name.
const SLOT: &str = "{reason}";

/// The facts a block's marker test reads: its span's name and instrumentation scope.
pub struct BlockScope<'a> {
    pub span_name: Option<&'a str>,
    pub scope_name: Option<&'a str>,
    pub scope_version: Option<&'a str>,
}

/// The declared markers, compiled once with the rest of the embedded ruleset.
#[derive(Debug, Default)]
pub struct RedactionMarkerPlan {
    markers: Vec<CompiledMarker>,
}

#[derive(Debug)]
struct CompiledMarker {
    condition: SpanExpr,
    prefix: String,
    suffix: String,
}

impl RedactionMarkerPlan {
    pub fn compile(files: &[RuleFile]) -> Result<Self, ClauseDefect> {
        let mut ids = BTreeSet::new();
        let mut markers = Vec::new();
        for file in files {
            for rule in &file.redaction_markers {
                let defect =
                    |message: String| ClauseDefect::new(&[&rule.id], Some(&file.id), message);
                if rule.id.is_empty() {
                    return Err(defect(
                        "a redaction marker clause has an empty id".to_string(),
                    ));
                }
                if !ids.insert(rule.id.as_str()) {
                    return Err(defect(format!(
                        "redaction marker clause id `{}` is declared more than once",
                        rule.id
                    )));
                }
                let (prefix, suffix) = rule
                    .marker
                    .split_once(SLOT)
                    .filter(|(prefix, suffix)| {
                        !format!("{prefix}{suffix}").contains(['{', '}'])
                    })
                    .ok_or_else(|| {
                        defect(format!(
                            "redaction marker `{}` must name exactly one `{SLOT}` and no other placeholder",
                            rule.id
                        ))
                    })?;
                if prefix.trim().is_empty() || suffix.trim().is_empty() {
                    return Err(defect(format!(
                        "redaction marker `{}` needs text other than whitespace either side of `{SLOT}`: matched \
                         alone, a slot at an edge would take ordinary text for a marker",
                        rule.id
                    )));
                }
                let condition =
                    super::detect_rules::checked_condition(&rule.condition, Readable::BLOCK)
                        .map_err(|refusal| {
                            defect(format!("redaction marker `{}`: {refusal}", rule.id))
                        })?;
                if !super::message_projection::requires_a_scope(&condition) {
                    return Err(defect(format!(
                        "redaction marker `{}` does not require one instrumentation scope, so another producer's \
                         identical text would read as this producer's placeholder",
                        rule.id
                    )));
                }
                markers.push(CompiledMarker {
                    condition,
                    prefix: prefix.to_string(),
                    suffix: suffix.to_string(),
                });
            }
        }
        Ok(Self { markers })
    }

    /// Whether `block`'s whole content is one marker its span's producer declares.
    pub fn is_marker(&self, scope: &BlockScope<'_>, block: &ContentBlock) -> bool {
        if self.markers.is_empty() {
            return false;
        }
        let Some(text) = whole_text(block) else {
            return false;
        };
        let no_attributes = HashMap::new();
        let subject = SpanSubject {
            span_name: scope.span_name,
            attrs: &no_attributes,
            scope_name: scope.scope_name,
            scope_version: scope.scope_version,
            resource: None,
            marks: 0,
        };
        self.markers.iter().any(|marker| {
            text.len() > marker.prefix.len() + marker.suffix.len()
                && text
                    .strip_prefix(marker.prefix.as_str())
                    .and_then(|rest| rest.strip_suffix(marker.suffix.as_str()))
                    .is_some_and(|reason| {
                        // One marker, not two side by side: a reason holding the marker's own edges is text
                        // quoting several markers.
                        !reason.is_empty()
                            && !reason.contains(['\n', '\r'])
                            && !reason.contains(marker.prefix.as_str())
                            && !reason.contains(marker.suffix.as_str())
                    })
                && span_conditions::holds(&marker.condition, &subject)
        })
    }
}

/// A block's content when it is one text and nothing else: a text block's text, or a tool result holding one
/// string or one text part.
fn whole_text(block: &ContentBlock) -> Option<&str> {
    match block {
        // A text with citations states more than the marker.
        ContentBlock::Text { text, citations } if citations.is_empty() => Some(text),
        ContentBlock::ToolResult { content, .. } => match content {
            serde_json::Value::String(text) => Some(text),
            serde_json::Value::Array(parts) => match parts.as_slice() {
                // One text part and nothing beside its text: a part carrying citations or more is more than it.
                [serde_json::Value::Object(part)]
                    if part.len() == 2
                        && part.get("type").and_then(serde_json::Value::as_str) == Some("text") =>
                {
                    part.get("text").and_then(serde_json::Value::as_str)
                }
                _ => None,
            },
            _ => None,
        },
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compiled(markers: serde_json::Value) -> Result<RedactionMarkerPlan, ClauseDefect> {
        RedactionMarkerPlan::compile(&[probe(markers)])
    }

    fn probe(markers: serde_json::Value) -> RuleFile {
        serde_json::from_value(serde_json::json!({
            "id": "probe",
            "redaction_markers": markers
        }))
        .expect("the probe asset parses")
    }

    fn marker(text: &str) -> serde_json::Value {
        serde_json::json!([{
            "id": "probe.scrubbed",
            "where": {"source": "scope.name", "equals": "probe.scope"},
            "marker": text
        }])
    }

    fn in_scope(scope: &str) -> BlockScope<'_> {
        BlockScope {
            span_name: None,
            scope_name: Some(scope),
            scope_version: None,
        }
    }

    fn text(value: &str) -> ContentBlock {
        ContentBlock::Text {
            text: value.to_string(),
            citations: Vec::new(),
        }
    }

    /// A marker names exactly one `{reason}` and nothing else in braces, has real text either side of it, and
    /// is gated on an instrumentation scope; anything else is refused when the assets load.
    #[test]
    fn a_marker_is_refused_unless_its_form_and_scope_are_exact() {
        assert!(compiled(marker("[Scrubbed due to '{reason}']")).is_ok());
        for refused in [
            "[Scrubbed]",
            "[Scrubbed {reason} and {reason}]",
            "[Scrubbed {reason} {other}]",
            "[Scrubbed {reason} }",
            "{reason}]",
            "[Scrubbed {reason}",
            "  {reason}  ",
        ] {
            assert!(compiled(marker(refused)).is_err(), "{refused}");
        }
        let unscoped = serde_json::json!([{
            "id": "probe.scrubbed",
            "where": {"source": "span_name", "starts_with": "probe "},
            "marker": "[Scrubbed due to '{reason}']"
        }]);
        assert!(
            compiled(unscoped).is_err(),
            "a marker every producer's text could match is refused"
        );
        let mut twice = marker("[Scrubbed due to '{reason}']");
        let first = twice[0].clone();
        twice.as_array_mut().expect("a list").push(first);
        assert!(compiled(twice).is_err(), "an id declared twice is refused");
    }

    /// A block is a marker only when its whole content is one, with a reason and no line break, under its
    /// producer's scope: text quoting the marker, and another producer's identical string, are values.
    #[test]
    fn only_a_whole_content_marker_of_its_own_producer_is_one() {
        let plan = compiled(marker("[Scrubbed due to '{reason}']")).expect("compiles");
        let scope = in_scope("probe.scope");
        assert!(plan.is_marker(&scope, &text("[Scrubbed due to 'credential']")));
        let result = |content: serde_json::Value| ContentBlock::ToolResult {
            tool_use_id: Some("c".to_string()),
            name: None,
            content,
            is_error: false,
            provider_executed: false,
        };
        assert!(plan.is_marker(
            &scope,
            &result(serde_json::json!("[Scrubbed due to 'credential']"))
        ));
        assert!(plan.is_marker(
            &scope,
            &result(
                serde_json::json!([{"type": "text", "text": "[Scrubbed due to 'credential']"}])
            )
        ));
        for value in [
            "The log said [Scrubbed due to 'credential'] here.",
            "[Scrubbed due to 'credential'] and more",
            "[Scrubbed due to '']",
            "[Scrubbed due to 'a\nb']",
            "[Scrubbed due to 'credential'] and [Scrubbed due to 'credential']",
        ] {
            assert!(!plan.is_marker(&scope, &text(value)), "{value:?}");
        }
        assert!(!plan.is_marker(
            &scope,
            &result(serde_json::json!([
                {"type": "text", "text": "[Scrubbed due to 'credential']"},
                {"type": "text", "text": "more"}
            ]))
        ));
        // A marker carrying more than its text - citations - states more than the marker.
        assert!(!plan.is_marker(
            &scope,
            &ContentBlock::Text {
                text: "[Scrubbed due to 'credential']".to_string(),
                citations: vec![crate::sideml::Citation {
                    kind: crate::sideml::CitationKind::Url,
                    source: Some("https://example.com".to_string()),
                    title: None,
                    text_start: None,
                    text_end: None,
                    cited_text: None,
                    raw: None,
                }],
            }
        ));
        assert!(!plan.is_marker(
            &scope,
            &result(
                serde_json::json!([{"type": "text", "text": "[Scrubbed due to 'credential']",
                "citations": [{"url": "https://example.com"}]}])
            )
        ));

        assert!(
            !plan.is_marker(
                &in_scope("other.scope"),
                &text("[Scrubbed due to 'credential']")
            ),
            "another producer's identical string is its value"
        );
        assert!(!plan.is_marker(
            &BlockScope {
                span_name: None,
                scope_name: None,
                scope_version: None
            },
            &text("[Scrubbed due to 'credential']")
        ));
    }
}
