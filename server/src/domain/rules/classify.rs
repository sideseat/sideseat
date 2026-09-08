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
    #[error("classification rule `{rule}` in `{file}` names no observation type")]
    NoResult { file: String, rule: String },
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
    observation_type: String,
}

/// The ordered rules, sorted by rank at compile time.
pub struct ClassifyPlan {
    rules: Vec<CompiledRule>,
}

impl ClassifyPlan {
    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }

    /// What this span is, or `None` where no rule holds.
    ///
    /// `None` rather than a default, because "nothing recognised this" is the caller's own vocabulary to name -
    /// and a rule answering everything would otherwise be indistinguishable from no rule answering.
    pub fn observation_type(
        &self,
        span_name: &str,
        attrs: &HashMap<String, String>,
    ) -> Option<&str> {
        self.rules
            .iter()
            .find(|rule| {
                rule.all_of.iter().all(|signals| {
                    super::detect_rules::compiled_signals_hold(signals, span_name, attrs)
                })
            })
            .map(|rule| rule.observation_type.as_str())
    }
}

/// Compile every asset's classification rules into one ordered plan.
pub fn compile(
    sources: &std::collections::BTreeMap<String, Vec<u8>>,
) -> Result<ClassifyPlan, ClassifyCompileError> {
    let mut declared: Vec<(i32, String, CompiledRule)> = Vec::new();
    let mut by_id: HashMap<String, String> = HashMap::new();

    for (file_id, bytes) in sources {
        let file: RuleFile =
            serde_json::from_slice(bytes).map_err(|error| ClassifyCompileError::Parse {
                path: file_id.clone(),
                message: error.to_string(),
            })?;
        for rule in &file.observation_types {
            if let Some(first) = by_id.get(&rule.id) {
                return Err(ClassifyCompileError::DuplicateId {
                    rule: rule.id.clone(),
                    first: first.clone(),
                    second: file_id.clone(),
                });
            }
            by_id.insert(rule.id.clone(), file_id.clone());
            declared.push((rule.rank, file_id.clone(), compile_rule(file_id, rule)?));
        }
    }

    // Sorted by rank, and a **shared** rank is refused: the answer is a precedence, so two rules that could
    // both hold at the same rank would be resolved by whichever asset loaded first - which is not a statement
    // anybody made.
    declared.sort_by_key(|(rank, _, _)| *rank);
    for pair in declared.windows(2) {
        if pair[0].0 == pair[1].0 {
            return Err(ClassifyCompileError::SharedRank {
                first: pair[0].2.rule_id.clone(),
                second: pair[1].2.rule_id.clone(),
                rank: pair[0].0,
            });
        }
    }

    Ok(ClassifyPlan {
        rules: declared.into_iter().map(|(_, _, rule)| rule).collect(),
    })
}

fn compile_rule(file_id: &str, rule: &ClassifyRule) -> Result<CompiledRule, ClassifyCompileError> {
    if rule.all_of.is_empty() {
        return Err(ClassifyCompileError::NoCondition {
            file: file_id.to_string(),
            rule: rule.id.clone(),
        });
    }
    if rule.observation_type.is_empty() {
        return Err(ClassifyCompileError::NoResult {
            file: file_id.to_string(),
            rule: rule.id.clone(),
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
        observation_type: rule.observation_type.clone(),
    })
}
