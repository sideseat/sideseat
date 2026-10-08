//! Which conversation thread a request span belongs to, for a producer that exports each request's delta.
//!
//! Derived when the span is ingested, because the key reads the span's attributes and the read path holds only its
//! messages. What the key *is* stays the asset's statement: this module only evaluates the declared condition and
//! renders the declared sources.

use std::collections::{BTreeSet, HashMap};

use serde_json::Value as JsonValue;

use super::detect_rules::checked_condition;
use super::diagnostics::ClauseDefect;
use super::schema::RuleFile;
use super::span_conditions::{self, Readable, SpanExpr, SpanSubject};

/// The declared thread rules, compiled once with the rest of the ruleset.
#[derive(Debug, Default)]
pub struct RequestThreadPlan {
    rules: Vec<CompiledThread>,
}

#[derive(Debug)]
struct CompiledThread {
    id: String,
    condition: SpanExpr,
    /// The attribute each key source reads, in the declared order.
    key: Vec<String>,
}

impl RequestThreadPlan {
    pub fn compile(files: &[RuleFile]) -> Result<Self, ClauseDefect> {
        let mut ids = BTreeSet::new();
        let mut rules: Vec<CompiledThread> = Vec::new();
        for file in files {
            for rule in &file.request_threads {
                let defect =
                    |reason: String| ClauseDefect::new(&[&rule.id], Some(&file.id), reason);
                if rule.id.is_empty() || !ids.insert(rule.id.as_str()) {
                    return Err(defect(format!(
                        "request thread `{}` has an empty or repeated id, and the id is part of every key it \
                         derives",
                        rule.id
                    )));
                }
                let condition =
                    checked_condition(&rule.condition, Readable::SPAN).map_err(|refusal| {
                        defect(format!("request thread `{}`: {refusal}", rule.id))
                    })?;
                if rule.key.is_empty() {
                    return Err(defect(format!(
                        "request thread `{}` names no key source, so every request it holds for would be one \
                         thread",
                        rule.id
                    )));
                }
                let mut key = Vec::with_capacity(rule.key.len());
                for source in &rule.key {
                    match source.strip_prefix("attr:") {
                        Some(attribute)
                            if !attribute.is_empty() && !key.iter().any(|k| k == attribute) =>
                        {
                            key.push(attribute.to_string());
                        }
                        _ => {
                            return Err(defect(format!(
                                "request thread `{}` key source `{source}` is not a distinct `attr:<key>` - a \
                                 thread is identified by the span's own attributes, each once",
                                rule.id
                            )));
                        }
                    }
                }
                // One thread per span: a span two rules hold for would be keyed by whichever came first, which is
                // the asset load order rather than anyone's statement.
                if let Some(other) = rules
                    .iter()
                    .find(|other| !span_conditions::disjoint(&other.condition, &condition))
                {
                    return Err(defect(format!(
                        "request threads `{}` and `{}` may both hold for one span, which would then belong to \
                         two threads",
                        other.id, rule.id
                    )));
                }
                rules.push(CompiledThread {
                    id: rule.id.clone(),
                    condition,
                    key,
                });
            }
        }
        Ok(Self { rules })
    }

    /// The thread this span is a request of, or `None` where no rule holds for it.
    ///
    /// Rendered as a JSON array - the rule's id, then each key source's value in declared order, `null` where the
    /// attribute is absent - so two spans share a thread exactly when every source agrees, absent included, and
    /// two rules' threads never meet.
    pub fn thread_key(&self, span_name: &str, attrs: &HashMap<String, String>) -> Option<String> {
        let subject = SpanSubject {
            span_name,
            attrs,
            scope_name: None,
            scope_version: None,
            resource: None,
        };
        let rule = self
            .rules
            .iter()
            .find(|rule| span_conditions::holds(&rule.condition, &subject))?;
        let mut parts = Vec::with_capacity(rule.key.len() + 1);
        parts.push(JsonValue::String(rule.id.clone()));
        parts.extend(rule.key.iter().map(|attribute| match attrs.get(attribute) {
            Some(value) => JsonValue::String(value.clone()),
            None => JsonValue::Null,
        }));
        Some(JsonValue::Array(parts).to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe(threads: serde_json::Value) -> RuleFile {
        serde_json::from_value(serde_json::json!({
            "id": "probe",
            "request_threads": threads,
        }))
        .expect("the probe asset parses")
    }

    fn plan(threads: serde_json::Value) -> Result<RequestThreadPlan, ClauseDefect> {
        RequestThreadPlan::compile(&[probe(threads)])
    }

    fn attrs(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    /// Two requests share a thread when every key source agrees, absent included, and the rule's id is part of
    /// the key; a span no rule holds for has none.
    #[test]
    fn a_thread_key_is_every_declared_source_absent_included() {
        let plan = plan(serde_json::json!([{
            "id": "probe.thread",
            "where": {"source": "span_name", "equals": "probe.request"},
            "key": ["attr:session", "attr:agent"]
        }]))
        .expect("the probe compiles");
        let main = plan.thread_key("probe.request", &attrs(&[("session", "s1")]));
        let sub = plan.thread_key(
            "probe.request",
            &attrs(&[("session", "s1"), ("agent", "a1")]),
        );
        assert_eq!(main.as_deref(), Some(r#"["probe.thread","s1",null]"#));
        assert_eq!(sub.as_deref(), Some(r#"["probe.thread","s1","a1"]"#));
        assert_ne!(main, sub, "an agent with its own id is a thread of its own");
        assert_eq!(
            plan.thread_key("probe.tool", &attrs(&[("session", "s1")])),
            None
        );
    }

    /// A key that could not identify a thread, and two rules that could both hold for one span, are refused.
    #[test]
    fn a_thread_rule_that_cannot_key_one_thread_is_refused() {
        let rule = |id: &str, name: &str, key: serde_json::Value| serde_json::json!({"id": id, "where": {"source": "span_name", "equals": name}, "key": key});
        for (why, threads) in [
            (
                "no key source",
                serde_json::json!([rule("a", "x", serde_json::json!([]))]),
            ),
            (
                "a source that is not an attribute",
                serde_json::json!([rule("a", "x", serde_json::json!(["span_name"]))]),
            ),
            (
                "an empty attribute",
                serde_json::json!([rule("a", "x", serde_json::json!(["attr:"]))]),
            ),
            (
                "one attribute twice",
                serde_json::json!([rule("a", "x", serde_json::json!(["attr:s", "attr:s"]))]),
            ),
            (
                "two rules holding for one span",
                serde_json::json!([rule("a", "x", serde_json::json!(["attr:s"])), {
                    "id": "b", "where": {"source": "span_name", "starts_with": "x"}, "key": ["attr:s"]}]),
            ),
            (
                "a repeated id",
                serde_json::json!([
                    rule("a", "x", serde_json::json!(["attr:s"])),
                    rule("a", "y", serde_json::json!(["attr:s"]))
                ]),
            ),
        ] {
            assert!(plan(threads).is_err(), "{why}: accepted");
        }
        assert!(
            plan(serde_json::json!([
                rule("a", "x", serde_json::json!(["attr:s"])),
                rule("b", "y", serde_json::json!(["attr:s"]))
            ]))
            .is_ok(),
            "two rules for different span names are two producers' threads"
        );
    }
}
