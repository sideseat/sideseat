//! Producer detection from declared signals.
//!
//! This replaces an ordered table of Rust structs, four of whose rules escaped into hand-written
//! matcher functions. All four are gone: one was a duplicate of the existing prefix dimension, two were
//! "a resource attribute contains this", and the fourth a case-insensitive phrase search. None needed
//! code, which is the point - a rule that needs a callback is a rule the vocabulary cannot express, and
//! the answer is to extend the vocabulary generically rather than to open a hole for one producer.
//!
//! What detection produces is a **label**: provenance, display and filtering. Nothing that decides
//! behaviour reads it. That is deliberate and it is checked (`the_engine_names_no_framework`), because a
//! label consulted by the pipeline is framework identity back in the driving seat under another name.

use std::collections::BTreeMap;
use std::collections::HashMap;

use super::span_conditions::{self, Readable, SpanExpr};

/// A compiled detection rule.
#[derive(Debug, Clone)]
pub struct CompiledDetect {
    pub rule_file: String,
    pub rule_id: String,
    pub doc: Option<String>,
    pub label: String,
    pub priority: i32,
    /// The overlaps this clause documents. Validated against `priority` at compile time and read only by the
    /// overlap report: the priority alone decides which rule answers.
    pub supersedes: Vec<String>,
    /// The evidence, lowered once.
    pub condition: SpanExpr,
}

/// A rule that reads a key this span has, and disagreed about its value.
///
/// The evidence behind "nothing recognised this producer": it names the declaration, the label it would have
/// answered with, the key both sides are talking about, what the rule required and what the span carried - which
/// together are the operator's next step, since the remedy is to declare the value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NearMiss {
    pub rule_id: String,
    pub label: String,
    pub carrier: String,
    pub expected: String,
    pub found: String,
}

/// The compiled detection plan: rules in priority order, plus the declaration fallback.
#[derive(Debug, Default)]
pub struct DetectPlan {
    rules: Vec<CompiledDetect>,
    /// SDK-declared slug → label.
    sdk_slugs: BTreeMap<String, String>,
}

/// What a detection rule may examine.
///
/// No framework label, for the same reason the carrier context carries none: detection is what
/// *produces* the label, so consuming one here would be circular.
#[derive(Debug, Clone, Copy)]
pub struct DetectContext<'a> {
    pub span_name: &'a str,
    pub scope_name: Option<&'a str>,
    pub span_attrs: &'a HashMap<String, String>,
    pub resource_attrs: &'a HashMap<String, String>,
}

/// Why a detection ruleset would not compile.
#[derive(Debug)]
pub enum DetectCompileError {
    EmptyLiteral {
        rule: String,
        dimension: &'static str,
    },
    /// A condition that cannot be lowered, or carries a literal that matches everything or nothing.
    Condition {
        rule: String,
        detail: String,
    },
    /// A disjunct another disjunct of the same group already covers, so it can never be why the rule matched.
    SubsumedLiteral {
        rule: String,
        dead: String,
        covering: String,
    },
    /// A rule an earlier rule always satisfies first, so it can never be reached.
    ShadowedRule {
        earlier: String,
        later: String,
    },
    /// An SDK slug resolving to a label no detection rule produces.
    SlugLabelNoRuleProduces {
        slug: String,
        label: String,
    },
    DuplicateRuleId {
        rule: String,
    },
    DuplicatePriority {
        first: String,
        second: String,
    },
    DuplicateSlug {
        slug: String,
    },
    /// A `supersedes` edge that cannot mean what it says.
    UselessSupersedes {
        rule: String,
        target: String,
        detail: String,
    },
}

impl std::fmt::Display for DetectCompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SlugLabelNoRuleProduces { slug, label } => write!(
                f,
                "SDK slug `{slug}` resolves to `{label}`, which no detection rule produces - the two sections \
                 fill the label independently, so this is a second framework name for one producer, and which \
                 one a span gets depends on whether its signals were detected or its SDK declared itself"
            ),
            Self::ShadowedRule { earlier, later } => write!(
                f,
                "rule `{later}` can never be reached: `{earlier}` is tried ahead of it and every span `{later}` \
                 matches satisfies `{earlier}` too, so `{later}`'s answer is unreachable and reads as protection \
                 it does not give"
            ),
            Self::SubsumedLiteral {
                rule,
                dead,
                covering,
            } => write!(
                f,
                "detection rule `{rule}` declares `{dead}` beside `{covering}`, which already covers it - the \
                 broader condition always holds first, so this one can never be why the rule matched, and it \
                 reads as precision the rule does not have"
            ),
            Self::Condition { rule, detail } => {
                write!(f, "detection rule `{rule}` has a condition that {detail}")
            }
            Self::UselessSupersedes {
                rule,
                target,
                detail,
            } => write!(
                f,
                "detection rule `{rule}` supersedes `{target}`, which {detail}. `supersedes` documents an \
                 overlap the priorities resolve and waives the overlap report for that pair; an edge that \
                 contradicts them, or names nothing, reads as an ordering it does not state"
            ),
            Self::EmptyLiteral { rule, dimension } => write!(
                f,
                "detection rule `{rule}` has an empty value in `{dimension}`, which matches everything"
            ),
            Self::DuplicateRuleId { rule } => {
                write!(f, "detection rule id `{rule}` is declared more than once")
            }
            Self::DuplicatePriority { first, second } => write!(
                f,
                "detection rules `{first}` and `{second}` share a priority: their relative order would \
                 depend on load order, and detection order is policy"
            ),
            Self::DuplicateSlug { slug } => write!(
                f,
                "SDK slug `{slug}` is claimed by more than one framework, so a declaration naming it \
                 has no single answer"
            ),
        }
    }
}

/// A section's `where`, lowered and checked the way every section checks one: a source the section is not given
/// and a test a source cannot answer are refused, so is a literal that matches everything or nothing, and so is
/// a disjunct its own group already covers. The error is the reason, for the caller to locate.
pub(super) fn checked_condition(
    condition: &super::schema::SpanWhere,
    readable: Readable,
) -> Result<SpanExpr, ConditionRefusal> {
    let lowered = span_conditions::lower(condition, readable)
        .map_err(|defect| ConditionRefusal::Unlowerable(defect.0))?;
    if let Some(defect) = lowered
        .defects(&super::span_conditions::SpanAtom::defect)
        .first()
    {
        return Err(ConditionRefusal::Unlowerable(format!(
            "carries `{}` with {}",
            defect.atom, defect.reason
        )));
    }
    if let Some((dead, covering)) = span_conditions::dead_disjunct(&lowered) {
        return Err(ConditionRefusal::DeadDisjunct {
            dead: span_conditions::render(&dead),
            covering: span_conditions::render(&covering),
        });
    }
    Ok(lowered)
}

/// Why a section's condition was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ConditionRefusal {
    Unlowerable(String),
    DeadDisjunct { dead: String, covering: String },
}

impl std::fmt::Display for ConditionRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unlowerable(detail) => f.write_str(detail),
            Self::DeadDisjunct { dead, covering } => write!(
                f,
                "declares `{dead}` beside `{covering}`, which already covers it, so it can never be why the \
                 condition held"
            ),
        }
    }
}

/// Compile every asset's detection rules into one ordered plan.
pub fn compile(assets: &super::assets::ParsedAssets) -> Result<DetectPlan, DetectCompileError> {
    let mut rules: Vec<CompiledDetect> = Vec::new();
    let mut seen_ids: HashMap<String, ()> = HashMap::new();
    let mut plan = DetectPlan::default();

    for file in assets.files() {
        for slug in &file.sdk_slugs {
            if plan
                .sdk_slugs
                .insert(slug.slug.clone(), slug.label.clone())
                .is_some()
            {
                return Err(DetectCompileError::DuplicateSlug {
                    slug: slug.slug.clone(),
                });
            }
        }
        // One body of evidence - a rule's own `where`, or one of its `alternatives` - validated and compiled the
        // same way, so an alternative cannot get a weaker check than the primary.
        let compile_one = |id: &str,
                           doc: Option<String>,
                           priority: i32,
                           label: &str,
                           supersedes: Vec<String>,
                           condition: &super::schema::SpanWhere|
         -> Result<CompiledDetect, DetectCompileError> {
            if id.is_empty() || label.is_empty() {
                return Err(DetectCompileError::EmptyLiteral {
                    rule: id.to_string(),
                    dimension: "id/label",
                });
            }
            let condition =
                checked_condition(condition, Readable::ALL).map_err(|refusal| match refusal {
                    ConditionRefusal::Unlowerable(detail) => DetectCompileError::Condition {
                        rule: id.to_string(),
                        detail,
                    },
                    ConditionRefusal::DeadDisjunct { dead, covering } => {
                        DetectCompileError::SubsumedLiteral {
                            rule: id.to_string(),
                            dead,
                            covering,
                        }
                    }
                })?;
            Ok(CompiledDetect {
                rule_file: file.id.clone(),
                rule_id: id.to_string(),
                doc,
                label: label.to_string(),
                priority,
                supersedes,
                condition,
            })
        };

        for rule in &file.detect {
            // Every id, the rule's and its alternatives', in one namespace: they are all rules once compiled, and
            // a diagnostic naming one has to identify it.
            for id in std::iter::once(&rule.id).chain(rule.alternatives.iter().map(|alt| &alt.id)) {
                if seen_ids.insert(id.clone(), ()).is_some() {
                    return Err(DetectCompileError::DuplicateRuleId { rule: id.clone() });
                }
            }
            rules.push(compile_one(
                &rule.id,
                rule.doc.clone(),
                rule.priority,
                &rule.label,
                rule.supersedes.clone(),
                &rule.condition,
            )?);
            for alternative in &rule.alternatives {
                // The label is the *rule's*: an alternative is further evidence for one producer, so declaring its
                // own label would make it a separate rule wearing a rule's id. Its documented overlaps are its own,
                // because it exists to sit at a different priority.
                rules.push(compile_one(
                    &alternative.id,
                    alternative.doc.clone(),
                    alternative.priority,
                    &rule.label,
                    alternative.supersedes.clone(),
                    &alternative.condition,
                )?);
            }
        }
    }

    // A shared priority is refused: two rules that both match one span would be separated by load order, and the
    // whole reason the order is explicit is that it is policy somebody has to own.
    if let Some((first, second)) =
        super::precedence::shared_priority(&rules, |rule| rule.priority, |_, _| true)
    {
        return Err(DetectCompileError::DuplicatePriority {
            first: first.rule_id.clone(),
            second: second.rule_id.clone(),
        });
    }
    rules.sort_by_key(|rule| rule.priority);

    // Every `supersedes` edge must agree with the priorities it documents. It once *ordered*, ahead of rank, and
    // that made the order a preference relation no total order could state (two shipped edges pointed at rules
    // ranked ahead of their sources); now the priority is the order and the edge is a checked statement about it.
    let priority_of: HashMap<&str, i32> = rules
        .iter()
        .map(|rule| (rule.rule_id.as_str(), rule.priority))
        .collect();
    for rule in &rules {
        if let Some((target, defect)) =
            super::precedence::edge_defect(&rule.rule_id, rule.priority, &rule.supersedes, |id| {
                priority_of.get(id).copied()
            })
        {
            return Err(DetectCompileError::UselessSupersedes {
                rule: rule.rule_id.clone(),
                target: target.to_string(),
                detail: defect.describe(),
            });
        }
    }

    // A rule an earlier one always satisfies first can never answer. The same defect as a subsumed literal, one
    // level up - and a *detection* rule shadowed this way silently never attributes its producer at all. Sound
    // rather than complete (see `span_conditions::implies`).
    for (index, earlier) in rules.iter().enumerate() {
        for later in &rules[index + 1..] {
            if span_conditions::implies(&later.condition, &earlier.condition) {
                return Err(DetectCompileError::ShadowedRule {
                    earlier: earlier.rule_id.clone(),
                    later: later.rule_id.clone(),
                });
            }
        }
    }

    // A slug's label has to be one some rule produces. The two sections fill the label independently and only
    // duplicate *slugs* were refused - so a typo declared a second framework name for one producer, and which one
    // a span got depended on whether its signals were detected or its SDK declared itself. A reader filtering on
    // the label then sees one producer as two.
    let produced: std::collections::BTreeSet<&str> =
        rules.iter().map(|rule| rule.label.as_str()).collect();
    if let Some((slug, label)) = plan
        .sdk_slugs
        .iter()
        .find(|(_, label)| !produced.contains(label.as_str()))
    {
        return Err(DetectCompileError::SlugLabelNoRuleProduces {
            slug: slug.clone(),
            label: label.clone(),
        });
    }

    plan.rules = rules;
    Ok(plan)
}

mod evaluation;
