//! Observation classification as ordered first-match rules.
//!
//! The answer is a **precedence**, not a set of independent facts, which is why this is its own plan and not a
//! field with sources. A transport attribute makes a span a plain span whatever else it carries; a conventional
//! operation name outranks a dialect's own span-kind attribute; a name that merely *mentions* a tool loses to
//! any of them. That ordering is the whole of the knowledge, so it belongs in the assets with the conditions.
//!
//! One thing here is not a producer's business: which stored value each label means. The plan answers with the
//! label and the caller maps it to the enum, which is why no framework name appears in this file.

use std::collections::HashMap;

use super::schema::ClassifyRule;
use super::span_conditions::{self, Readable, SpanExpr};

#[derive(Debug, thiserror::Error)]
pub enum ClassifyCompileError {
    #[error("classification rule `{rule}` in `{file}` has a condition that {detail}")]
    DeadCondition {
        file: String,
        rule: String,
        detail: String,
    },
    #[error("classification rule `{rule}` in `{file}` names no result")]
    NoResult { file: String, rule: String },
    #[error(
        "classification rule `{rule}` in `{file}` answers `{result}`, which is not one of: {allowed}"
    )]
    UnknownResult {
        file: String,
        rule: String,
        result: String,
        allowed: String,
    },
    /// A rule an earlier rule always satisfies first, so its result is unreachable.
    #[error(
        "classification rule `{later}` can never be reached: `{earlier}` is tried ahead of it and every span `{later}` matches satisfies `{earlier}` too, so `{later}`'s result is unreachable and reads as protection it does not give"
    )]
    ShadowedRule { earlier: String, later: String },
    #[error(
        "classification rules `{first}` and `{second}` share priority {priority}, so which answers depends on load order"
    )]
    SharedPriority {
        first: String,
        second: String,
        priority: i32,
    },
    #[error("classification rule id `{rule}` is declared twice, in `{first}` and `{second}`")]
    DuplicateId {
        rule: String,
        first: String,
        second: String,
    },
}

struct CompiledRule {
    rule_id: String,
    condition: SpanExpr,
    result: String,
    replaces_legacy_result: Option<String>,
}

/// The ordered rules of each classification, sorted by priority at compile time.
///
/// Two questions with two precedences, deliberately not one: a transport call is an HTTP *category* and a plain
/// *observation*, and one dialect's operation names name an agent in one and nothing in the other.
pub struct ClassifyPlan {
    observation_types: Vec<CompiledRule>,
    span_categories: Vec<CompiledRule>,
}

impl ClassifyPlan {
    pub fn rule_count(&self) -> usize {
        self.observation_types.len() + self.span_categories.len()
    }

    /// What kind of observation this span is, or `None` where no rule holds.
    ///
    /// `None` rather than a default, because "nothing recognised this" is the caller's own vocabulary to name -
    /// and a rule answering everything would otherwise be indistinguishable from no rule answering.
    pub fn observation_type(
        &self,
        span_name: &str,
        attrs: &HashMap<String, String>,
    ) -> Option<super::expr::Verdict<&str>> {
        first_match(&self.observation_types, span_name, attrs)
    }

    /// Whether the matching observation rule explicitly replaces this retired-sweep answer.
    pub fn observation_type_replaces_legacy(
        &self,
        span_name: &str,
        attrs: &HashMap<String, String>,
        legacy_result: &str,
    ) -> bool {
        replaces_legacy(&self.observation_types, span_name, attrs, legacy_result)
    }

    /// Which category this span falls in, or `None` where no rule holds.
    pub fn span_category(
        &self,
        span_name: &str,
        attrs: &HashMap<String, String>,
    ) -> Option<super::expr::Verdict<&str>> {
        first_match(&self.span_categories, span_name, attrs)
    }

    /// Whether the matching category rule explicitly replaces this retired-sweep answer.
    pub fn span_category_replaces_legacy(
        &self,
        span_name: &str,
        attrs: &HashMap<String, String>,
        legacy_result: &str,
    ) -> bool {
        replaces_legacy(&self.span_categories, span_name, attrs, legacy_result)
    }
}

fn matching_rule<'a>(
    rules: &'a [CompiledRule],
    span_name: &str,
    attrs: &HashMap<String, String>,
) -> Option<&'a CompiledRule> {
    let subject = span_conditions::SpanSubject {
        span_name,
        attrs,
        scope_name: None,
        resource: None,
        scope_version: None,
    };
    rules
        .iter()
        .find(|rule| span_conditions::holds(&rule.condition, &subject))
}

fn first_match<'a>(
    rules: &'a [CompiledRule],
    span_name: &str,
    attrs: &HashMap<String, String>,
) -> Option<super::expr::Verdict<&'a str>> {
    matching_rule(rules, span_name, attrs).map(|rule| {
        // The rule that answered, named. A classification used to return the label alone, so "this span is
        // a plain span" and "no rule recognised it" were the same answer to a reader, and the rule's id -
        // which compilation keeps - was discarded at the one moment it is useful.
        super::expr::Verdict::from_one(
            rule.result.as_str(),
            super::expr::ClausePath::root(rule.rule_id.clone()),
        )
    })
}

fn replaces_legacy(
    rules: &[CompiledRule],
    span_name: &str,
    attrs: &HashMap<String, String>,
    legacy_result: &str,
) -> bool {
    matching_rule(rules, span_name, attrs).and_then(|rule| rule.replaces_legacy_result.as_deref())
        == Some(legacy_result)
}

/// Compile every asset's classification rules into one ordered plan.
pub fn compile(assets: &super::assets::ParsedAssets) -> Result<ClassifyPlan, ClassifyCompileError> {
    let mut observation_types: Vec<(i32, CompiledRule)> = Vec::new();
    let mut span_categories: Vec<(i32, CompiledRule)> = Vec::new();
    // One id space across both classifications: an id names a rule, and the same name meaning two rules would
    // make a diagnostic ambiguous.
    let mut by_id: HashMap<String, String> = HashMap::new();

    for (file_id, file) in assets.iter() {
        let file_id = &file_id.to_owned();
        for (rules, into, allowed) in [
            (
                &file.observation_types,
                &mut observation_types,
                OBSERVATION_TYPES,
            ),
            (&file.span_categories, &mut span_categories, SPAN_CATEGORIES),
        ] {
            for rule in rules {
                if let Some(first) = by_id.get(&rule.id) {
                    return Err(ClassifyCompileError::DuplicateId {
                        rule: rule.id.clone(),
                        first: first.clone(),
                        second: file_id.clone(),
                    });
                }
                by_id.insert(rule.id.clone(), file_id.clone());
                into.push((rule.priority, compile_rule(file_id, rule, allowed)?));
            }
        }
    }

    // Sorted by priority, and a **shared** priority is refused *within* a classification: the answer is a
    // precedence, so two rules that could both hold at the same priority would be resolved by whichever asset
    // loaded first, which is not a statement anybody made. Across the two classifications a priority means
    // nothing, so they are separate arenas.
    for rules in [&mut observation_types, &mut span_categories] {
        if let Some((first, second)) =
            super::precedence::shared_priority(rules, |(priority, _)| *priority, |_, _| true)
        {
            return Err(ClassifyCompileError::SharedPriority {
                first: first.1.rule_id.clone(),
                second: second.1.rule_id.clone(),
                priority: first.0,
            });
        }
        rules.sort_by_key(|(priority, _)| *priority);
    }

    // A rule an earlier one always satisfies first can never answer, and its result is unreachable - the same
    // defect as a subsumed literal, one level up. Checked *within* a classification, in priority order, since that is
    // where the precedence lives. Sound rather than complete (see `detect_rules::shadows`): a false refusal breaks
    // a build for a reason nobody can act on.
    for rules in [&observation_types, &span_categories] {
        for (index, (_, earlier)) in rules.iter().enumerate() {
            for (_, later) in &rules[index + 1..] {
                if span_conditions::implies(&later.condition, &earlier.condition) {
                    return Err(ClassifyCompileError::ShadowedRule {
                        earlier: earlier.rule_id.clone(),
                        later: later.rule_id.clone(),
                    });
                }
            }
        }
    }

    Ok(ClassifyPlan {
        observation_types: observation_types.into_iter().map(|(_, r)| r).collect(),
        span_categories: span_categories.into_iter().map(|(_, r)| r).collect(),
    })
}

/// The answers each classification may give. **Ours**, not any producer's - which is why they are here rather
/// than in an asset, and why a rule naming something else is a build defect rather than a silent `span`.
pub(super) const OBSERVATION_TYPES: &[&str] = &[
    "generation",
    "embedding",
    "agent",
    "tool",
    "chain",
    "retriever",
    "guardrail",
    "evaluator",
    "span",
];

pub(super) const SPAN_CATEGORIES: &[&str] = &[
    "llm",
    "tool",
    "agent",
    "chain",
    "retriever",
    "embedding",
    "db",
    "storage",
    "http",
    "messaging",
    "other",
];

fn compile_rule(
    file_id: &str,
    rule: &ClassifyRule,
    allowed: &[&str],
) -> Result<CompiledRule, ClassifyCompileError> {
    if rule.result.is_empty() {
        return Err(ClassifyCompileError::NoResult {
            file: file_id.to_string(),
            rule: rule.id.clone(),
        });
    }
    // A misspelling was silently a plain span, or a plain `other` - the answer the caller gives when *no rule
    // holds*, so a typo was indistinguishable from a rule that did not apply.
    if !allowed.contains(&rule.result.as_str()) {
        return Err(ClassifyCompileError::UnknownResult {
            file: file_id.to_string(),
            rule: rule.id.clone(),
            result: rule.result.clone(),
            allowed: allowed.join(", "),
        });
    }
    if let Some(legacy_result) = &rule.replaces_legacy_result
        && !allowed.contains(&legacy_result.as_str())
    {
        return Err(ClassifyCompileError::UnknownResult {
            file: file_id.to_string(),
            rule: rule.id.clone(),
            result: legacy_result.clone(),
            allowed: allowed.join(", "),
        });
    }
    // A condition that can never hold makes the rule dead, and one reading the resource or the scope never holds
    // here: classification is given a span's own name and attributes only.
    let condition = super::detect_rules::checked_condition(&rule.condition, Readable::SPAN)
        .map_err(|refusal| ClassifyCompileError::DeadCondition {
            file: file_id.to_string(),
            rule: rule.id.clone(),
            detail: refusal.to_string(),
        })?;
    Ok(CompiledRule {
        rule_id: rule.id.clone(),
        condition,
        result: rule.result.clone(),
        replaces_legacy_result: rule.replaces_legacy_result.clone(),
    })
}
