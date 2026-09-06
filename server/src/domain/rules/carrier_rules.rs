//! Carrier semantics as data: the compiled plan, and the lookup the pipeline calls.
//!
//! This replaces a span-blind table in Rust. The defect it removes: `gen_ai.output.messages` was
//! classified as one emission wherever it appeared, but on an *aggregator* span - a framework's root
//! agent span re-listing the whole turn - it is accumulated state, and reading it as an emission put a
//! turn's final answer ahead of the tool calls that produced it.
//!
//! The fix is not a special case for that carrier. It is that a clause may constrain the observation
//! type, so "the same carrier name means different things on different spans" becomes something a rule
//! file can state.

use std::collections::BTreeMap;
use std::collections::HashMap;

use crate::domain::sideml::carrier::CarrierSemantics;

use super::schema::{CarrierRule, Facts, MatchSpec, PrimaryKey, RuleFile};

/// What the pipeline knows about an observation when it asks what its carrier means.
///
/// Deliberately does **not** carry a framework label. Nothing that decides behaviour may consume one:
/// detection produces a label for provenance and display, and a carrier's meaning is a property of its
/// structure and the span that wrote it, not of a name somebody matched earlier.
#[derive(Debug, Default, Clone, Copy)]
pub struct CarrierContext<'a> {
    pub event: Option<&'a str>,
    pub attribute: Option<&'a str>,
    pub observation_type: Option<&'a str>,
    pub span_name: Option<&'a str>,
    pub scope_name: Option<&'a str>,
}

impl<'a> CarrierContext<'a> {
    /// The carrier alone, with no span context - the reading available where context has not been
    /// plumbed through yet. Keeps those call sites honest: they get the generic clause, never a
    /// qualified one, because they cannot supply what a qualified clause asks about.
    pub fn carrier_only(event: Option<&'a str>, attribute: Option<&'a str>) -> Self {
        Self {
            event,
            attribute,
            ..Self::default()
        }
    }
}

/// A compiled clause: the match spec, its resolved facts, and where it came from.
#[derive(Debug, Clone)]
pub struct CompiledClause {
    pub rule_file: String,
    pub clause_id: String,
    pub doc: Option<String>,
    pub match_spec: MatchSpec,
    pub semantics: CarrierSemantics,
    pub ordering_family: Option<String>,
    pub specificity: u32,
}

/// The resolved answer, with the clause that produced it.
///
/// The clause travels with the answer because a fact nobody can trace to a declaration is not
/// explainable, and the explain trace is one of the engine's acceptance conditions.
#[derive(Debug, Clone)]
pub struct CarrierVerdict {
    pub semantics: CarrierSemantics,
    pub ordering_family: Option<String>,
    /// `None` when no clause matched and the conservative default was used.
    pub matched: Option<&'static CompiledClause>,
}

/// Carrier clauses, indexed so a lookup is a hash probe plus a short scan rather than a walk over
/// every declaration.
///
/// Prefix clauses cannot be hashed on the observation's key, so they are held in one list ordered by
/// descending prefix length: the first match is the longest, which is the most specific prefix.
#[derive(Debug, Default)]
pub struct CarrierPlan {
    by_event: HashMap<String, Vec<CompiledClause>>,
    by_attribute: HashMap<String, Vec<CompiledClause>>,
    by_attribute_prefix: Vec<CompiledClause>,
}

/// Why a ruleset would not compile.
#[derive(Debug)]
pub enum CompileError {
    Parse {
        path: String,
        message: String,
    },
    UnknownPreset {
        clause: String,
        preset: String,
    },
    NoPrimaryKey {
        clause: String,
    },
    /// Two clauses could match the same observation with equal specificity, so which one applied would
    /// depend on nothing a reader can see. Refused rather than resolved by load order.
    Ambiguous {
        first: String,
        second: String,
        key: String,
    },
    DuplicateClauseId {
        clause: String,
    },
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Parse { path, message } => write!(f, "{path}: {message}"),
            Self::UnknownPreset { clause, preset } => write!(
                f,
                "clause `{clause}` names preset `{preset}`, which the engine does not define"
            ),
            Self::NoPrimaryKey { clause } => write!(
                f,
                "clause `{clause}` constrains no event, attribute or attribute prefix, so it would \
                 match every observation of a span"
            ),
            Self::Ambiguous { first, second, key } => write!(
                f,
                "clauses `{first}` and `{second}` both match `{key}` at equal specificity: which one \
                 applies would depend on load order. Constrain one further, or make their qualifiers \
                 disjoint"
            ),
            Self::DuplicateClauseId { clause } => {
                write!(f, "clause id `{clause}` is declared more than once")
            }
        }
    }
}

/// Resolve a preset name to the six facts, then apply the clause's overrides.
fn resolve_facts(clause_id: &str, facts: &Facts) -> Result<CarrierSemantics, CompileError> {
    let mut semantics = match facts.preset.as_str() {
        "emission" => CarrierSemantics::EMISSION,
        "snapshot" => CarrierSemantics::SNAPSHOT,
        "accumulated_state" => CarrierSemantics::ACCUMULATED_STATE,
        other => {
            return Err(CompileError::UnknownPreset {
                clause: clause_id.to_string(),
                preset: other.to_string(),
            });
        }
    };
    if let Some(v) = facts.position_proves_distinct_occurrence {
        semantics.position_proves_distinct_occurrence = v;
    }
    if let Some(v) = facts.position_provides_sequence_order {
        semantics.position_provides_sequence_order = v;
    }
    if let Some(v) = facts.carrier_is_atomic_emission {
        semantics.carrier_is_atomic_emission = v;
    }
    if let Some(v) = facts.carrier_may_contain_history_or_state {
        semantics.carrier_may_contain_history_or_state = v;
    }
    if let Some(v) = facts.carrier_holds_span_output {
        semantics.carrier_holds_span_output = v;
    }
    if let Some(v) = facts.carrier_is_detached_request_frame {
        semantics.carrier_is_detached_request_frame = v;
    }
    Ok(semantics)
}

/// Compile every asset into one plan.
pub fn compile(sources: &BTreeMap<String, Vec<u8>>) -> Result<CarrierPlan, CompileError> {
    let mut clauses: Vec<CompiledClause> = Vec::new();
    let mut seen_ids: HashMap<String, ()> = HashMap::new();

    for (path, bytes) in sources {
        let file: RuleFile = serde_json::from_slice(bytes).map_err(|e| CompileError::Parse {
            path: path.clone(),
            message: e.to_string(),
        })?;
        for rule in &file.carriers {
            let CarrierRule {
                id,
                doc,
                match_spec,
                facts,
                ordering_family,
            } = rule;
            if seen_ids.insert(id.clone(), ()).is_some() {
                return Err(CompileError::DuplicateClauseId { clause: id.clone() });
            }
            if match_spec.primary_key().is_none() {
                return Err(CompileError::NoPrimaryKey { clause: id.clone() });
            }
            clauses.push(CompiledClause {
                rule_file: file.id.clone(),
                clause_id: id.clone(),
                doc: doc.clone(),
                match_spec: match_spec.clone(),
                semantics: resolve_facts(id, facts)?,
                ordering_family: ordering_family.clone(),
                specificity: match_spec.specificity(),
            });
        }
    }

    reject_ambiguity(&clauses)?;

    let mut plan = CarrierPlan::default();
    for clause in clauses {
        match clause.match_spec.primary_key() {
            Some(PrimaryKey::Event(event)) => plan
                .by_event
                .entry(event.to_string())
                .or_default()
                .push(clause),
            Some(PrimaryKey::Attribute(attribute)) => plan
                .by_attribute
                .entry(attribute.to_string())
                .or_default()
                .push(clause),
            Some(PrimaryKey::AttributePrefix(_)) => plan.by_attribute_prefix.push(clause),
            None => unreachable!("checked above"),
        }
    }

    // Most specific first, so a lookup takes the first match and stops.
    for bucket in plan
        .by_event
        .values_mut()
        .chain(plan.by_attribute.values_mut())
    {
        // Stable, so two clauses of equal specificity keep their asset order - and they can only be
        // equal here when their qualifiers are disjoint, since `reject_ambiguity` refused the rest.
        bucket.sort_by_key(|c| std::cmp::Reverse(c.specificity));
    }
    // Longest prefix first: a longer prefix is the more specific statement about a key, and the
    // specificity score cannot express that because both clauses constrain the same one dimension.
    plan.by_attribute_prefix.sort_by(|a, b| {
        let len = |c: &CompiledClause| {
            c.match_spec
                .attribute_prefix
                .as_deref()
                .map(str::len)
                .unwrap_or(0)
        };
        len(b)
            .cmp(&len(a))
            .then_with(|| b.specificity.cmp(&a.specificity))
    });

    Ok(plan)
}

/// Refuse a ruleset in which two clauses could match one observation with nothing to separate them.
///
/// Bounded and honest about it: this compares clauses that share a *primary key* - the same event, the
/// same attribute, or one prefix that is a prefix of the other - at equal specificity, and asks whether
/// their qualifiers can both hold. It does not attempt to decide overlap between arbitrary predicates,
/// which is why the match dimensions are a fixed finite set of scalars rather than a predicate language.
fn reject_ambiguity(clauses: &[CompiledClause]) -> Result<(), CompileError> {
    for (i, a) in clauses.iter().enumerate() {
        for b in &clauses[i + 1..] {
            if a.specificity != b.specificity {
                continue;
            }
            let (Some(ka), Some(kb)) = (a.match_spec.primary_key(), b.match_spec.primary_key())
            else {
                continue;
            };
            let same_key = match (ka, kb) {
                (PrimaryKey::Event(x), PrimaryKey::Event(y)) => x == y,
                (PrimaryKey::Attribute(x), PrimaryKey::Attribute(y)) => x == y,
                // Only an *equal* prefix is a collision. One prefix containing another is itself an
                // ordering - the longer is the more specific statement about a key, and the sort below
                // applies it - so refusing nested prefixes would reject a perfectly determinate
                // ruleset. The specificity score cannot see this, because both clauses constrain the
                // same single dimension.
                (PrimaryKey::AttributePrefix(x), PrimaryKey::AttributePrefix(y)) => x == y,
                _ => false,
            };
            if !same_key {
                continue;
            }
            if a.match_spec.qualifiers_can_overlap(&b.match_spec) {
                return Err(CompileError::Ambiguous {
                    first: a.clause_id.clone(),
                    second: b.clause_id.clone(),
                    key: format!("{ka:?}"),
                });
            }
        }
    }
    Ok(())
}

impl CarrierPlan {
    /// Does this clause's qualifiers hold for the observation?
    ///
    /// An absent context fact does **not** satisfy a clause that constrains it. A qualified clause makes
    /// a claim about the span, and a caller that cannot say what the span was has not established it -
    /// so such a caller gets the generic reading, which is the conservative one.
    fn qualifiers_hold(clause: &CompiledClause, ctx: &CarrierContext<'_>) -> bool {
        let spec = &clause.match_spec;
        if !spec.observation_type.is_empty() {
            match ctx.observation_type {
                Some(observed) => {
                    if !spec.observation_type.iter().any(|t| t == observed) {
                        return false;
                    }
                }
                None => return false,
            }
        }
        if let Some(prefix) = &spec.span_name_prefix {
            match ctx.span_name {
                Some(name) if name.starts_with(prefix.as_str()) => {}
                _ => return false,
            }
        }
        if let Some(needle) = &spec.scope_name_contains {
            match ctx.scope_name {
                Some(scope) if scope.contains(needle.as_str()) => {}
                _ => return false,
            }
        }
        true
    }

    /// The clause that governs this observation, most specific first.
    pub fn resolve(&self, ctx: &CarrierContext<'_>) -> Option<&CompiledClause> {
        if let Some(event) = ctx.event
            && let Some(bucket) = self.by_event.get(event)
            && let Some(hit) = bucket.iter().find(|c| Self::qualifiers_hold(c, ctx))
        {
            return Some(hit);
        }
        if let Some(attribute) = ctx.attribute {
            if let Some(bucket) = self.by_attribute.get(attribute)
                && let Some(hit) = bucket.iter().find(|c| Self::qualifiers_hold(c, ctx))
            {
                return Some(hit);
            }
            if let Some(hit) = self.by_attribute_prefix.iter().find(|c| {
                c.match_spec
                    .attribute_prefix
                    .as_deref()
                    .is_some_and(|p| attribute.starts_with(p))
                    && Self::qualifiers_hold(c, ctx)
            }) {
                return Some(hit);
            }
        }
        None
    }

    /// How many clauses the plan holds, for the coverage test.
    pub fn clause_count(&self) -> usize {
        self.by_event.values().map(Vec::len).sum::<usize>()
            + self.by_attribute.values().map(Vec::len).sum::<usize>()
            + self.by_attribute_prefix.len()
    }

    /// Every clause, for structural tests.
    pub fn clauses(&self) -> impl Iterator<Item = &CompiledClause> {
        self.by_event
            .values()
            .flatten()
            .chain(self.by_attribute.values().flatten())
            .chain(self.by_attribute_prefix.iter())
    }
}
