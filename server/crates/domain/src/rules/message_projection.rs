//! Read-time message projection filters compiled from producer rule assets.
//!
//! Telemetry extraction is intentionally untouched. A matching rule can hide producer bookkeeping
//! from the reconstructed conversation while the raw span and extracted messages remain stored.

use std::collections::{BTreeSet, HashMap};

use crate::observations::{MessageSource, RawMessage};

use super::diagnostics::ClauseDefect;
use super::schema::{MessageProjectionAction, RuleFile};
use super::span_conditions::{self, Readable, SpanAtom, SpanExpr, SpanSubject};

/// Facts available while projecting one stored span's messages.
pub struct MessageProjectionContext<'a> {
    pub scope_name: Option<&'a str>,
    pub scope_version: Option<&'a str>,
    pub span_name: Option<&'a str>,
    pub successful: bool,
    pub messages: &'a [RawMessage],
}

/// Typed projection rules, compiled once with the rest of the embedded ruleset.
#[derive(Debug, Default)]
pub struct MessageProjectionPlan {
    rules: Vec<CompiledProjection>,
}

#[derive(Debug)]
struct CompiledProjection {
    condition: SpanExpr,
    only_attribute_sources: Vec<String>,
    successful_only: bool,
    action: MessageProjectionAction,
}

impl MessageProjectionPlan {
    pub fn compile(files: &[RuleFile]) -> Result<Self, super::diagnostics::ClauseDefect> {
        let mut ids = BTreeSet::new();
        let mut rules = Vec::new();

        for file in files {
            for rule in &file.message_projections {
                if !ids.insert(rule.id.as_str()) {
                    return Err(ClauseDefect::new(
                        &[&rule.id],
                        Some(&file.id),
                        format!(
                            "message projection clause id `{}` is declared more than once",
                            rule.id
                        ),
                    ));
                }
                if rule.id.is_empty()
                    || rule.only_attribute_sources.is_empty()
                    || rule.only_attribute_sources.iter().any(String::is_empty)
                {
                    return Err(ClauseDefect::new(
                        &[&rule.id],
                        Some(&file.id),
                        format!(
                            "message projection clause `{}` contains an empty identifier, an empty attribute source \
                             or no attribute source at all - which no message comes from, so it would match nothing",
                            rule.id
                        ),
                    ));
                }
                let mut sources = BTreeSet::new();
                if !rule
                    .only_attribute_sources
                    .iter()
                    .all(|source| sources.insert(source.as_str()))
                {
                    return Err(ClauseDefect::new(
                        &[&rule.id],
                        Some(&file.id),
                        format!(
                            "message projection clause `{}` names one attribute source twice",
                            rule.id
                        ),
                    ));
                }
                // Through the validator every section's `where` goes through, so a projection cannot carry the
                // empty literal or the covered disjunct the others refuse: lowered alone, `span_name starts_with
                // ""` beside a scope matched every row of that scope.
                let condition =
                    super::detect_rules::checked_condition(&rule.condition, Readable::PROJECTION)
                        .map_err(|refusal| {
                        ClauseDefect::new(
                            &[&rule.id],
                            Some(&file.id),
                            format!("message projection clause `{}`: {refusal}", rule.id),
                        )
                    })?;
                if !requires_a_scope(&condition) {
                    return Err(ClauseDefect::new(
                        &[&rule.id],
                        Some(&file.id),
                        format!(
                            "message projection clause `{}` does not require one instrumentation scope, so it could \
                         suppress ordinary rows of every producer",
                            rule.id
                        ),
                    ));
                }
                rules.push(CompiledProjection {
                    condition,
                    only_attribute_sources: rule.only_attribute_sources.clone(),
                    successful_only: rule.successful_only,
                    action: rule.action,
                });
            }
        }

        Ok(Self { rules })
    }

    /// Whether the matching producer declaration withdraws this row from the SideML conversation.
    pub fn suppresses_messages(&self, context: &MessageProjectionContext<'_>) -> bool {
        // A row's span name may be missing; a projection asks only for a name it has, so a missing one is empty
        // text, which no `starts_with` the compiler accepts holds for.
        let no_attributes = HashMap::new();
        let subject = SpanSubject {
            span_name: context.span_name.unwrap_or(""),
            attrs: &no_attributes,
            scope_name: context.scope_name,
            scope_version: context.scope_version,
            resource: None,
        };
        self.rules.iter().any(|rule| {
            let success_matches = !rule.successful_only || context.successful;
            let source_matches = !context.messages.is_empty()
                && context.messages.iter().all(|message| {
                    matches!(
                        &message.source,
                        MessageSource::Attribute { key, .. }
                            if rule.only_attribute_sources.iter().any(|source| source == key)
                    )
                });
            span_conditions::holds(&rule.condition, &subject)
                && success_matches
                && source_matches
                && rule.action == MessageProjectionAction::SuppressMessages
        })
    }
}

/// Whether the condition cannot hold without naming an instrumentation scope: the scope test itself, a
/// conjunction - at any depth - with one among its members, or a disjunction every branch of which does. A
/// negation does not qualify, nor does a disjunction with a branch that names none: each can hold for a row of
/// a scope it never names.
fn requires_a_scope(condition: &SpanExpr) -> bool {
    use super::expr::Expr;
    match condition {
        Expr::Atom(atom) => matches!(atom, SpanAtom::ScopeNameEquals { .. }),
        Expr::All(group) => group.children().iter().any(requires_a_scope),
        Expr::Any(group) => group.children().iter().all(requires_a_scope),
        Expr::Not(_) => false,
    }
}

/// The retired dimension `scope_version_major_at_least`: the leading digits of the scope's version, as a
/// number. Kept as the oracle `scope_version` replaces it against.
#[cfg(test)]
fn version_major(version: &str) -> Option<u64> {
    let digits = version.bytes().take_while(u8::is_ascii_digit).count();
    (digits > 0)
        .then(|| version[..digits].parse().ok())
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A projection names the attributes a row's messages may come from: every message from one of them. A
    /// row with a message from any other attribute is not this shape, and an empty list, an empty name or a
    /// repeated one is refused.
    #[test]
    fn a_projection_suppresses_a_row_only_when_every_message_comes_from_a_named_attribute() {
        let compiled = |sources: serde_json::Value| {
            let file: RuleFile = serde_json::from_value(serde_json::json!({
                "id": "probe",
                "message_projections": [{
                    "id": "probe.projection",
                    "where": {"source": "scope.name", "equals": "probe.scope"},
                    "only_attribute_sources": sources,
                    "successful_only": false,
                    "action": "suppress_messages"
                }]
            }))
            .expect("the probe asset parses");
            MessageProjectionPlan::compile(&[file])
        };
        let plan = compiled(serde_json::json!(["probe.messages", "probe.instructions"]))
            .expect("two attribute sources compile");
        let message = |key: &str| RawMessage {
            source: MessageSource::Attribute {
                key: key.to_string(),
                time: chrono::Utc::now(),
            },
            content: serde_json::json!({"role": "user", "content": "q"}),
            rendering: false,
        };
        let suppressed = |messages: &[RawMessage]| {
            plan.suppresses_messages(&MessageProjectionContext {
                scope_name: Some("probe.scope"),
                scope_version: None,
                span_name: None,
                successful: true,
                messages,
            })
        };
        assert!(suppressed(&[message("probe.messages")]));
        assert!(suppressed(&[
            message("probe.messages"),
            message("probe.instructions")
        ]));
        assert!(
            !suppressed(&[message("probe.messages"), message("probe.other")]),
            "a message from an attribute not named keeps the row"
        );
        assert!(!suppressed(&[]), "an empty message list never matches");
        for sources in [
            serde_json::json!([]),
            serde_json::json!([""]),
            serde_json::json!(["probe.messages", "probe.messages"]),
        ] {
            assert!(compiled(sources.clone()).is_err(), "{sources}");
        }
    }

    /// A scope requirement counts at any depth of conjunction, and only there: a nested `all` holding the scope
    /// test cannot hold without it, while an `any` or a `not` can hold for a row of a scope it never names.
    #[test]
    fn a_projection_requires_its_scope_through_nested_conjunctions_only() {
        let compiled = |condition: serde_json::Value| {
            let file: RuleFile = serde_json::from_value(serde_json::json!({
                "id": "probe",
                "message_projections": [{
                    "id": "probe.projection",
                    "where": condition,
                    "only_attribute_sources": ["probe.messages"],
                    "successful_only": true,
                    "action": "suppress_messages"
                }]
            }))
            .expect("the probe asset parses");
            MessageProjectionPlan::compile(&[file])
        };
        let scope = serde_json::json!({"source": "scope.name", "equals": "probe.scope"});
        let name = serde_json::json!({"source": "span_name", "starts_with": "probe "});
        let other = serde_json::json!({"source": "span_name", "starts_with": "other "});
        for (why, condition) in [
            ("the scope alone", scope.clone()),
            (
                "a conjunction holding it",
                serde_json::json!({"all": [scope.clone(), name.clone()]}),
            ),
            (
                "a disjunction every branch of which names the scope",
                serde_json::json!({"any": [
                    {"all": [scope.clone(), name.clone()]},
                    {"all": [scope.clone(), other.clone()]}
                ]}),
            ),
            (
                "a conjunction nested in a conjunction",
                serde_json::json!({"all": [other, {"all": [scope.clone(), name.clone()]}]}),
            ),
        ] {
            assert!(
                compiled(condition).is_ok(),
                "{why}: refused, and it names its scope"
            );
        }
        for (why, condition) in [
            ("a span name alone", name.clone()),
            (
                "a disjunction with the scope in one branch",
                serde_json::json!({"any": [scope.clone(), name.clone()]}),
            ),
            (
                "a conjunction holding only the scope's negation",
                serde_json::json!({"all": [name.clone(), {"not": scope}]}),
            ),
            (
                "a scope prefix, which many scopes share",
                serde_json::json!({"all": [name, {"source": "scope.name", "starts_with": "probe."}]}),
            ),
        ] {
            assert!(
                compiled(condition).is_err(),
                "{why}: accepted without a scope"
            );
        }
    }

    /// A projection's `where` meets the refusals every section's does: lowered alone, it accepted an empty prefix
    /// beside its scope - matching every row of that scope - an empty scope name, and a covered disjunct.
    #[test]
    fn a_projection_condition_meets_the_common_refusals() {
        let compiled = |condition: serde_json::Value| {
            let file: RuleFile = serde_json::from_value(serde_json::json!({
                "id": "probe",
                "message_projections": [{
                    "id": "probe.projection",
                    "where": condition,
                    "only_attribute_sources": ["probe.messages"],
                    "successful_only": true,
                    "action": "suppress_messages"
                }]
            }))
            .expect("the probe asset parses");
            MessageProjectionPlan::compile(&[file])
        };
        let scope = serde_json::json!({"source": "scope.name", "equals": "probe.scope"});
        for (why, condition) in [
            (
                "an empty span-name prefix beside the scope",
                serde_json::json!({"all": [scope.clone(), {"source": "span_name", "starts_with": ""}]}),
            ),
            (
                "an empty scope name",
                serde_json::json!({"source": "scope.name", "equals": ""}),
            ),
            (
                "a disjunct its group covers",
                serde_json::json!({"all": [scope.clone(), {"any": [
                    {"source": "span_name", "starts_with": "probe "},
                    {"source": "span_name", "starts_with": "probe x"}
                ]}]}),
            ),
        ] {
            assert!(compiled(condition).is_err(), "{why}: accepted");
        }
        assert!(
            compiled(serde_json::json!({"all": [scope, {"source": "span_name", "starts_with": "probe "}]}))
                .is_ok(),
            "a scope and a real prefix compile"
        );
    }

    /// `scope.version` against the major-number dimension it replaced, over the versions a scope reports: equal
    /// on every release, and different, deliberately, where the old reading was not a version comparison - text
    /// after the digits, a leading `v`, an epoch, and the pre-releases of the bound.
    #[test]
    fn a_version_range_answers_as_the_major_number_did_on_every_release() {
        let condition = span_conditions::lower(
            &serde_json::from_value(serde_json::json!({
                "source": "scope.version",
                "version": {"scheme": "pep440", "at_least": "6.dev0", "because": "probe"}
            }))
            .expect("the condition parses"),
            Readable::PROJECTION,
        )
        .expect("the condition lowers");
        let no_attributes = HashMap::new();
        let new = |version: Option<&str>| {
            span_conditions::holds(
                &condition,
                &SpanSubject {
                    span_name: "",
                    attrs: &no_attributes,
                    scope_name: None,
                    scope_version: version,
                    resource: None,
                },
            )
        };
        let old = |version: Option<&str>| {
            version
                .and_then(version_major)
                .is_some_and(|major| major >= 6)
        };
        // `6.dev0` is the first release of the 6 series, so its pre-releases are in it as they were.
        for release in [
            "5.9.1",
            "6",
            "6.0",
            "6.0.0",
            "6.0.0b7",
            "6.0rc1",
            "6.12.3",
            "7.1",
            "10.0.post1",
            "4.28.0",
        ] {
            assert_eq!(new(Some(release)), old(Some(release)), "{release}");
        }
        assert!(!new(None) && !old(None), "no version answers neither");
        for (version, was, is) in [
            ("6garbage", true, false),
            ("v6.0", false, true),
            ("1!5.0", false, true),
        ] {
            assert_eq!(
                (old(Some(version)), new(Some(version))),
                (was, is),
                "{version}"
            );
        }
    }
}
