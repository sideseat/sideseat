//! The span-predicate dialects `where` replaced, kept as the equivalence oracle for that migration.
//!
//! `DetectMatch` (detection, classification, span-field and message gates), the span-fact signal and the
//! message rule's instrumentation-scope match, with the evaluators that answered them. The oracle in the goldens
//! (`every_where_answers_as_the_predicate_it_replaced_over_the_corpus`) reads a frozen copy of the retired
//! declarations and asks both forms of every clause about every captured span.

use std::collections::HashMap;

use serde::Deserialize;

/// A pair of strings - an attribute key and the value or substring it must hold.
#[derive(PartialEq, Eq, Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct KeyValue {
    pub key: String,
    pub value: String,
}

/// A case-insensitive text search over named sources.
#[derive(PartialEq, Eq, Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct TextContains {
    pub sources: Vec<String>,
    pub needles: Vec<String>,
    #[serde(default)]
    pub first_present_source: bool,
}

/// The retired signal set: every populated dimension is independently sufficient.
#[derive(PartialEq, Eq, Debug, Default, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct DetectMatch {
    #[serde(default)]
    pub span_name: Vec<String>,
    #[serde(default)]
    pub span_name_exact: Vec<String>,
    #[serde(default)]
    pub scope_name: Vec<String>,
    #[serde(default)]
    pub attr_prefix: Vec<String>,
    #[serde(default)]
    pub attr_equals: Vec<KeyValue>,
    #[serde(default)]
    pub attr_equals_ignore_case: Vec<KeyValue>,
    #[serde(default)]
    pub attr_exists: Vec<String>,
    #[serde(default)]
    pub service_name: Vec<String>,
    #[serde(default)]
    pub span_attr_contains: Vec<KeyValue>,
    #[serde(default)]
    pub resource_attr_contains: Vec<KeyValue>,
    #[serde(default)]
    pub text_contains: Option<TextContains>,
}

/// The retired span-fact signal: an optional equality, optionally case-folded, and keys that must all be present.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct SpanSignal {
    #[serde(default)]
    pub attr_equals: Option<KeyValue>,
    #[serde(default)]
    pub ignore_case: bool,
    #[serde(default)]
    pub attrs_present: Vec<String>,
}

/// The retired message-rule scope gate.
#[derive(Debug, Deserialize, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct InstrumentationScopeMatch {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub one_of: Vec<String>,
}

/// What a retired predicate was asked about.
pub struct RetiredSubject<'a> {
    pub span_name: &'a str,
    pub scope_name: Option<&'a str>,
    pub attrs: &'a HashMap<String, String>,
    pub resource: &'a HashMap<String, String>,
}

/// Whether a retired signal set held: any populated dimension.
pub fn signals_hold(spec: &DetectMatch, subject: &RetiredSubject<'_>) -> bool {
    let attrs = subject.attrs;
    if spec
        .attr_equals
        .iter()
        .any(|KeyValue { key, value }| attrs.get(key).is_some_and(|v| v == value))
        || spec
            .attr_equals_ignore_case
            .iter()
            .any(|KeyValue { key, value }| {
                attrs
                    .get(key)
                    .is_some_and(|v| v.eq_ignore_ascii_case(value))
            })
        || spec.attr_exists.iter().any(|key| attrs.contains_key(key))
        || spec
            .span_name_exact
            .iter()
            .any(|name| subject.span_name == name)
        || subject
            .scope_name
            .is_some_and(|scope| spec.scope_name.iter().any(|name| scope == name))
        || spec
            .span_name
            .iter()
            .any(|prefix| subject.span_name.starts_with(prefix))
        || subject.resource.get("service.name").is_some_and(|service| {
            spec.service_name
                .iter()
                .any(|declared| service.contains(declared.as_str()))
        })
        || spec
            .resource_attr_contains
            .iter()
            .any(|KeyValue { key, value }| {
                subject
                    .resource
                    .get(key)
                    .is_some_and(|v| v.contains(value.as_str()))
            })
        || spec
            .span_attr_contains
            .iter()
            .any(|KeyValue { key, value }| {
                attrs.get(key).is_some_and(|v| v.contains(value.as_str()))
            })
        || attrs
            .keys()
            .any(|k| spec.attr_prefix.iter().any(|p| k.starts_with(p.as_str())))
    {
        return true;
    }
    let Some(text) = &spec.text_contains else {
        return false;
    };
    let needles: Vec<String> = text.needles.iter().map(|n| n.to_lowercase()).collect();
    let hit = |value: &str| {
        let lowered = value.to_lowercase();
        needles.iter().any(|n| lowered.contains(n.as_str()))
    };
    let span_name_is_a_source = text.sources.iter().any(|s| s == "span_name");
    let keys: Vec<&str> = text
        .sources
        .iter()
        .filter_map(|s| s.strip_prefix("attr:"))
        .collect();
    if text.first_present_source {
        if span_name_is_a_source {
            return hit(subject.span_name);
        }
        return keys
            .iter()
            .find_map(|k| attrs.get(*k))
            .is_some_and(|v| hit(v));
    }
    (span_name_is_a_source && hit(subject.span_name))
        || keys.iter().filter_map(|k| attrs.get(*k)).any(|v| hit(v))
}

/// Whether a retired span-fact signal held.
pub fn signal_holds(signal: &SpanSignal, attrs: &HashMap<String, String>) -> bool {
    let equals = signal.attr_equals.as_ref().is_none_or(|want| {
        attrs.get(&want.key).is_some_and(|found| {
            if signal.ignore_case {
                found.eq_ignore_ascii_case(&want.value)
            } else {
                found == &want.value
            }
        })
    });
    equals
        && signal
            .attrs_present
            .iter()
            .all(|key| attrs.contains_key(key))
}

/// Whether a retired scope gate admitted this scope.
pub fn scope_holds(gate: &InstrumentationScopeMatch, scope: Option<&str>) -> bool {
    scope.is_some_and(|scope| {
        gate.name
            .iter()
            .chain(&gate.one_of)
            .any(|name| name == scope)
    })
}

/// Every clause's current `where` and what its section may read, keyed as the frozen retired declarations are:
/// a detection rule or alternative and a classification rule by id, a span-field source as `<rule>.<source>`,
/// a message rule or branch leaf by id, a compose member's fallback as `<rule>.member<index>`, a span-fact
/// signal as `<rule>.<signal>`.
pub fn current_conditions(
    assets: &super::assets::ParsedAssets,
) -> std::collections::BTreeMap<String, (super::span_conditions::Readable, super::schema::SpanWhere)>
{
    use super::span_conditions::Readable;
    let mut out = std::collections::BTreeMap::new();
    fn message(
        rule: &super::schema::MessageRule,
        out: &mut std::collections::BTreeMap<String, (Readable, super::schema::SpanWhere)>,
    ) {
        if let Some(condition) = &rule.condition {
            out.insert(
                rule.id.clone(),
                (Readable::SPAN_AND_SCOPE, condition.clone()),
            );
        }
        for (index, member) in rule.compose.iter().flat_map(|c| &c.members).enumerate() {
            if let Some(fallback) = &member.fallback {
                out.insert(
                    format!("{}.member{index}", rule.id),
                    (Readable::SPAN, fallback.condition.clone()),
                );
            }
        }
        if let Some(set) = &rule.branch_set {
            for leaf in set
                .primary
                .iter()
                .chain(&set.fallback_if_primary_empty)
                .chain(&set.always)
            {
                message(leaf, out);
            }
        }
    }
    for file in assets.files() {
        for rule in &file.detect {
            out.insert(rule.id.clone(), (Readable::ALL, rule.condition.clone()));
            for alternative in &rule.alternatives {
                out.insert(
                    alternative.id.clone(),
                    (Readable::ALL, alternative.condition.clone()),
                );
            }
        }
        for rule in file.observation_types.iter().chain(&file.span_categories) {
            out.insert(rule.id.clone(), (Readable::SPAN, rule.condition.clone()));
        }
        for rule in &file.span_fields {
            for source in &rule.sources {
                if let Some(condition) = &source.condition {
                    out.insert(
                        format!("{}.{}", rule.id, source.id),
                        (Readable::SPAN, condition.clone()),
                    );
                }
            }
        }
        for rule in &file.messages {
            message(rule, &mut out);
        }
        for rule in &file.span_facts {
            for signal in &rule.signals {
                out.insert(
                    format!("{}.{}", rule.id, signal.id),
                    (Readable::ATTRIBUTES, signal.condition.clone()),
                );
            }
        }
    }
    out
}

/// Whether one frozen retired declaration held of a span, the way its section asked it.
///
/// The record is one entry of the frozen file: `kind` and the retired fields of its clause. A message rule's
/// gate held where its `when` held, its `unless` did not, and its scope was admitted; detection was the only
/// section given the scope and the resource as signals.
pub fn retired_holds(record: &serde_json::Value, subject: &RetiredSubject<'_>) -> bool {
    let spec = |member: &str| -> Option<DetectMatch> {
        record
            .get(member)
            .map(|value| serde_json::from_value(value.clone()).expect("a frozen predicate decodes"))
    };
    let all_of = |member: &str| -> Vec<DetectMatch> {
        record
            .get(member)
            .map(|value| serde_json::from_value(value.clone()).expect("a frozen predicate decodes"))
            .unwrap_or_default()
    };
    let none = HashMap::new();
    let gate_subject = RetiredSubject {
        span_name: subject.span_name,
        scope_name: None,
        attrs: subject.attrs,
        resource: &none,
    };
    match record["kind"].as_str().expect("a frozen record has a kind") {
        "detect" => {
            signals_hold(&spec("match").expect("a detection rule matches"), subject)
                && all_of("all_of")
                    .iter()
                    .all(|set| signals_hold(set, subject))
        }
        "classify" => all_of("all_of")
            .iter()
            .all(|set| signals_hold(set, &gate_subject)),
        "span_field" | "compose" => {
            signals_hold(&spec("when").expect("a gate is declared"), &gate_subject)
        }
        "message" => {
            spec("when").is_none_or(|gate| signals_hold(&gate, &gate_subject))
                && spec("unless").is_none_or(|gate| !signals_hold(&gate, &gate_subject))
                && record.get("instrumentation_scope").is_none_or(|scope| {
                    scope_holds(
                        &serde_json::from_value(scope.clone()).expect("a frozen scope decodes"),
                        subject.scope_name,
                    )
                })
        }
        "span_fact" => signal_holds(
            &serde_json::from_value(record["signal"].clone()).expect("a frozen signal decodes"),
            subject.attrs,
        ),
        other => panic!("a frozen record of unknown kind `{other}`"),
    }
}
