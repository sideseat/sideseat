//! What kind of observation a span is, as ordered first-match rules.
//!
//! The answer is a **precedence**, not a set of independent facts, which is why this is its own plan and not a
//! field with sources. A transport attribute makes a span a plain span whatever else it carries; a conventional
//! operation name outranks a dialect's own span-kind attribute; a name that merely *mentions* a tool loses to
//! any of them. That ordering is the whole of the knowledge, so it belongs in the assets with the conditions.
//!
//! One thing here is not a producer's business: which stored value each label means. The plan answers with the
//! label and the caller maps it to the enum, which is why no framework name appears in this file.

use std::collections::HashMap;

use super::detect_rules::{CompiledDetect, compile_signals, gate_defect};
use super::schema::{ClassifyRule, RuleFile};

#[derive(Debug, thiserror::Error)]
pub enum ClassifyCompileError {
    #[error("classification rules in `{path}` are malformed: {message}")]
    Parse { path: String, message: String },
    #[error(
        "classification rule `{rule}` in `{file}` states no condition, so it answers every span"
    )]
    NoCondition { file: String, rule: String },
    #[error("classification rule `{rule}` in `{file}` has a condition that never holds: {detail}")]
    DeadCondition {
        file: String,
        rule: String,
        detail: &'static str,
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
    #[error(
        "classification rules `{first}` and `{second}` share rank {rank}, so which answers depends on load order"
    )]
    SharedRank {
        first: String,
        second: String,
        rank: i32,
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
    /// Every one of these must hold; each is internally a disjunction.
    all_of: Vec<CompiledDetect>,
    result: String,
}

/// The ordered rules of each classification, sorted by rank at compile time.
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

    /// Which category this span falls in, or `None` where no rule holds.
    pub fn span_category(
        &self,
        span_name: &str,
        attrs: &HashMap<String, String>,
    ) -> Option<super::expr::Verdict<&str>> {
        first_match(&self.span_categories, span_name, attrs)
    }
}

fn first_match<'a>(
    rules: &'a [CompiledRule],
    span_name: &str,
    attrs: &HashMap<String, String>,
) -> Option<super::expr::Verdict<&'a str>> {
    rules
        .iter()
        .find(|rule| {
            rule.all_of.iter().all(|signals| {
                super::detect_rules::compiled_signals_hold(signals, span_name, attrs)
            })
        })
        .map(|rule| {
            // The rule that answered, named. A classification used to return the label alone, so "this span is
            // a plain span" and "no rule recognised it" were the same answer to a reader, and the rule's id -
            // which compilation keeps - was discarded at the one moment it is useful.
            super::expr::Verdict::from_one(
                rule.result.as_str(),
                super::expr::ClausePath::root(rule.rule_id.clone()),
            )
        })
}

/// Compile every asset's classification rules into one ordered plan.
pub fn compile(
    sources: &std::collections::BTreeMap<String, Vec<u8>>,
) -> Result<ClassifyPlan, ClassifyCompileError> {
    let mut observation_types: Vec<(i32, CompiledRule)> = Vec::new();
    let mut span_categories: Vec<(i32, CompiledRule)> = Vec::new();
    // One id space across both classifications: an id names a rule, and the same name meaning two rules would
    // make a diagnostic ambiguous.
    let mut by_id: HashMap<String, String> = HashMap::new();

    for (file_id, bytes) in sources {
        let file: RuleFile =
            serde_json::from_slice(bytes).map_err(|error| ClassifyCompileError::Parse {
                path: file_id.clone(),
                message: error.to_string(),
            })?;
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
                into.push((rule.rank, compile_rule(file_id, rule, allowed)?));
            }
        }
    }

    // Sorted by rank, and a **shared** rank is refused *within* a classification: the answer is a precedence,
    // so two rules that could both hold at the same rank would be resolved by whichever asset loaded first,
    // which is not a statement anybody made. Across the two classifications a rank means nothing, so they are
    // checked apart.
    for rules in [&mut observation_types, &mut span_categories] {
        rules.sort_by_key(|(rank, _)| *rank);
        for pair in rules.windows(2) {
            if pair[0].0 == pair[1].0 {
                return Err(ClassifyCompileError::SharedRank {
                    first: pair[0].1.rule_id.clone(),
                    second: pair[1].1.rule_id.clone(),
                    rank: pair[0].0,
                });
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

const SPAN_CATEGORIES: &[&str] = &[
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
    if rule.all_of.is_empty() {
        return Err(ClassifyCompileError::NoCondition {
            file: file_id.to_string(),
            rule: rule.id.clone(),
        });
    }
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
    for spec in &rule.all_of {
        // A conjunct that can never hold makes the whole rule dead, so it is refused for the same reason a
        // field source's gate is. `service_name` and `resource_attr_contains` are *not* refused here, unlike on
        // a message gate: classification is given the span's own attributes only, and a resource dimension
        // would be accepted and never hold - so it is caught by the same check.
        if let Some(detail) = gate_defect(spec) {
            return Err(ClassifyCompileError::DeadCondition {
                file: file_id.to_string(),
                rule: rule.id.clone(),
                detail,
            });
        }
        if let Some(dimension) = super::detect_rules::unavailable_gate_dimension(spec) {
            return Err(ClassifyCompileError::DeadCondition {
                file: file_id.to_string(),
                rule: rule.id.clone(),
                detail: match dimension {
                    "service_name" => {
                        "`service_name` names a resource attribute, and classification is \
                                       given a span's own attributes only"
                    }
                    _ => {
                        "`resource_attr_contains` names a resource attribute, and classification is given \
                          a span's own attributes only"
                    }
                },
            });
        }
    }
    Ok(CompiledRule {
        rule_id: rule.id.clone(),
        all_of: rule.all_of.iter().map(compile_signals).collect(),
        result: rule.result.clone(),
    })
}
