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
    /// The marks the span answered at ingest, as stored on the row (`rules::span_marks`). How a read-time rule
    /// asks about a span's attributes, which the read itself does not carry.
    pub marks: u16,
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
    only_event_sources: Vec<String>,
    successful_only: bool,
    action: MessageProjectionAction,
}

impl MessageProjectionPlan {
    pub fn compile(
        files: &[RuleFile],
        marks: &super::span_marks::SpanMarkPlan,
    ) -> Result<Self, super::diagnostics::ClauseDefect> {
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
                let lists = [
                    ("attribute", &rule.only_attribute_sources),
                    ("event", &rule.only_event_sources),
                ];
                if rule.id.is_empty()
                    || lists.iter().all(|(_, names)| names.is_empty())
                    || lists
                        .iter()
                        .any(|(_, names)| names.iter().any(String::is_empty))
                {
                    return Err(ClauseDefect::new(
                        &[&rule.id],
                        Some(&file.id),
                        format!(
                            "message projection clause `{}` contains an empty identifier, an empty source name \
                             or no attribute and no event source at all - which no message comes from, so it \
                             would match nothing",
                            rule.id
                        ),
                    ));
                }
                for (kind, names) in lists {
                    let mut seen = BTreeSet::new();
                    if !names.iter().all(|name| seen.insert(name.as_str())) {
                        return Err(ClauseDefect::new(
                            &[&rule.id],
                            Some(&file.id),
                            format!(
                                "message projection clause `{}` names one {kind} source twice",
                                rule.id
                            ),
                        ));
                    }
                }
                // Through the validator every section's `where` goes through, so a projection cannot carry the
                // empty literal or the covered disjunct the others refuse: lowered alone, `span_name starts_with
                // ""` beside a scope matched every row of that scope.
                let condition = super::detect_rules::checked_condition_with_marks(
                    &rule.condition,
                    Readable::PROJECTION,
                    marks,
                )
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
                    only_event_sources: rule.only_event_sources.clone(),
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
            marks: context.marks,
        };
        self.rules.iter().any(|rule| {
            let success_matches = !rule.successful_only || context.successful;
            // By kind and name: an attribute and an event of one name are different carriers.
            let source_matches = !context.messages.is_empty()
                && context
                    .messages
                    .iter()
                    .all(|message| match &message.source {
                        MessageSource::Attribute { key, .. } => rule
                            .only_attribute_sources
                            .iter()
                            .any(|source| source == key),
                        MessageSource::Event { name, .. } => {
                            rule.only_event_sources.iter().any(|source| source == name)
                        }
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
            MessageProjectionPlan::compile(
                &[file],
                &crate::rules::span_marks::SpanMarkPlan::default(),
            )
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
            direction: None,
        };
        let suppressed = |messages: &[RawMessage]| {
            plan.suppresses_messages(&MessageProjectionContext {
                scope_name: Some("probe.scope"),
                scope_version: None,
                span_name: None,
                successful: true,
                messages,
                marks: 0,
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
            MessageProjectionPlan::compile(
                &[file],
                &crate::rules::span_marks::SpanMarkPlan::default(),
            )
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
            MessageProjectionPlan::compile(
                &[file],
                &crate::rules::span_marks::SpanMarkPlan::default(),
            )
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
                    marks: 0,
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

    /// **A projection may ask about a span mark, which is how a read-time rule reads what the row cannot carry.**
    ///
    /// The read holds a row's messages and not its attributes, so a projection that must distinguish two spans by
    /// what they carried asks about the answer stored at ingest. Here two rows differ only in their mark, and only
    /// the marked one is withdrawn - which is the whole point: with no mark the projection would have to suppress
    /// both or neither.
    #[test]
    fn a_projection_asks_about_a_mark_the_span_answered_at_ingest() {
        let file: RuleFile = serde_json::from_value(serde_json::json!({
            "id": "probe",
            "span_marks": [{
                "id": "probe.streamed",
                "because": "the read holds a row's messages and not its attributes",
                "where": {"source": "attr:probe_options", "parses": "json",
                    "member": {"path": "$.stream", "equals": true}}
            }],
            "message_projections": [{
                "id": "probe.projection",
                "where": {"all": [
                    {"source": "scope.name", "equals": "probe.scope"},
                    {"source": "mark:probe.streamed", "exists": true}
                ]},
                "only_attribute_sources": ["probe.messages"],
                "successful_only": false,
                "action": "suppress_messages"
            }]
        }))
        .expect("the probe asset parses");
        let marks = crate::rules::span_marks::SpanMarkPlan::compile(std::slice::from_ref(&file))
            .expect("the marks compile");
        let plan =
            MessageProjectionPlan::compile(&[file], &marks).expect("the projection compiles");
        let message = RawMessage {
            source: MessageSource::Attribute {
                key: "probe.messages".to_string(),
                time: chrono::Utc::now(),
            },
            content: serde_json::json!({"role": "user", "content": "q"}),
            rendering: false,
            direction: None,
        };
        let messages = [message];
        let suppressed = |marks: u16| {
            plan.suppresses_messages(&MessageProjectionContext {
                scope_name: Some("probe.scope"),
                scope_version: None,
                span_name: None,
                successful: true,
                messages: &messages,
                marks,
            })
        };
        // The word the ingest side would have stored for a streamed request, and for every other span.
        let streamed = marks.marks_of(
            "span",
            &[(
                "probe_options".to_string(),
                r#"{"stream": true}"#.to_string(),
            )]
            .into_iter()
            .collect(),
            None,
            None,
        );
        let plain = marks.marks_of("span", &HashMap::new(), None, None);
        assert_eq!(
            (streamed, plain),
            (1, 0),
            "one mark, set only for the one span"
        );
        assert!(suppressed(streamed), "the marked row is withdrawn");
        assert!(
            !suppressed(plain),
            "and the unmarked one is not - the two rows are otherwise identical, which is why the mark exists"
        );
        // A projection naming a mark no asset declares is refused rather than never holding.
        let unknown: RuleFile = serde_json::from_value(serde_json::json!({
            "id": "probe",
            "message_projections": [{
                "id": "probe.projection",
                "where": {"all": [
                    {"source": "scope.name", "equals": "probe.scope"},
                    {"source": "mark:probe.absent", "exists": true}
                ]},
                "only_attribute_sources": ["probe.messages"],
                "successful_only": false,
                "action": "suppress_messages"
            }]
        }))
        .expect("the probe asset parses");
        assert!(
            MessageProjectionPlan::compile(
                &[unknown],
                &crate::rules::span_marks::SpanMarkPlan::default()
            )
            .is_err()
        );
    }

    /// **A projection may name the events a row's messages come from, beside or instead of attributes**, each
    /// carrier matched by kind as well as name - and, with a mark over a JSON member, withdraws only the row
    /// whose question was answered *true* at ingest. False and unknown (the options do not parse, or are not
    /// there) leave the row unmarked, so it stays.
    #[test]
    fn a_projection_names_the_events_its_rows_messages_come_from() {
        let compiled = |attributes: Option<serde_json::Value>,
                        events: Option<serde_json::Value>| {
            let mut projection = serde_json::json!({
                "id": "probe.projection",
                "where": {"all": [
                    {"source": "scope.name", "equals": "probe.scope"},
                    {"source": "mark:probe.streamed", "exists": true}
                ]},
                "successful_only": false,
                "action": "suppress_messages"
            });
            if let Some(attributes) = attributes {
                projection["only_attribute_sources"] = attributes;
            }
            if let Some(events) = events {
                projection["only_event_sources"] = events;
            }
            let file: RuleFile = serde_json::from_value(serde_json::json!({
                "id": "probe",
                "span_marks": [{
                    "id": "probe.streamed",
                    "because": "the read holds a row's messages and not its attributes",
                    "where": {"source": "attr:probe_options", "parses": "json",
                        "member": {"path": "$.stream", "equals": true}}
                }],
                "message_projections": [projection]
            }))
            .expect("the probe asset parses");
            let marks =
                crate::rules::span_marks::SpanMarkPlan::compile(std::slice::from_ref(&file))
                    .expect("the marks compile");
            MessageProjectionPlan::compile(&[file], &marks).map(|plan| (plan, marks))
        };
        let (plan, marks) = compiled(
            Some(serde_json::json!(["probe.request"])),
            Some(serde_json::json!(["probe.user", "probe.system"])),
        )
        .expect("attribute and event sources compile");
        let time = chrono::Utc::now();
        let from = |source: MessageSource| RawMessage {
            source,
            content: serde_json::json!({"role": "user", "content": "q"}),
            rendering: false,
            direction: None,
        };
        let event = |name: &str| {
            from(MessageSource::Event {
                name: name.to_string(),
                time,
            })
        };
        let attribute = |key: &str| {
            from(MessageSource::Attribute {
                key: key.to_string(),
                time,
            })
        };
        let mark = |options: Option<&str>| {
            let attrs: HashMap<String, String> = options
                .map(|text| ("probe_options".to_string(), text.to_string()))
                .into_iter()
                .collect();
            marks.marks_of("span", &attrs, None, None)
        };
        let suppressed = |messages: &[RawMessage], marks: u16| {
            plan.suppresses_messages(&MessageProjectionContext {
                scope_name: Some("probe.scope"),
                scope_version: None,
                span_name: None,
                successful: true,
                messages,
                marks,
            })
        };
        let streamed = mark(Some(r#"{"stream": true}"#));
        let request = [
            event("probe.system"),
            event("probe.user"),
            attribute("probe.request"),
        ];
        assert!(suppressed(&request[..2], streamed), "a row of named events");
        assert!(
            suppressed(&request, streamed),
            "a row mixing named events and attributes"
        );
        for (why, marks) in [
            ("false", mark(Some(r#"{"stream": false}"#))),
            ("unknown: the options do not parse", mark(Some("{stream"))),
            ("unknown: there are no options", mark(None)),
        ] {
            assert!(
                !suppressed(&request, marks),
                "{why}: the row is unmarked and stays"
            );
        }
        for (why, messages) in [
            (
                "an event not named",
                vec![event("probe.user"), event("probe.choice")],
            ),
            (
                "an attribute named like a listed event",
                vec![attribute("probe.user")],
            ),
            (
                "an event named like a listed attribute",
                vec![event("probe.request")],
            ),
            ("no message at all", Vec::new()),
        ] {
            assert!(!suppressed(&messages, streamed), "{why} keeps the row");
        }

        assert!(
            compiled(None, Some(serde_json::json!(["probe.user"]))).is_ok(),
            "events alone are a source"
        );
        for (why, attributes, events) in [
            ("no list", None, None),
            (
                "two empty lists",
                Some(serde_json::json!([])),
                Some(serde_json::json!([])),
            ),
            ("an empty event name", None, Some(serde_json::json!([""]))),
            (
                "an event named twice",
                None,
                Some(serde_json::json!(["probe.user", "probe.user"])),
            ),
        ] {
            let refused = compiled(attributes, events)
                .err()
                .map(|defect| defect.to_string());
            assert!(
                refused
                    .as_deref()
                    .is_some_and(|reason| reason.contains("probe.projection")),
                "{why}: {refused:?}"
            );
        }
    }
}
