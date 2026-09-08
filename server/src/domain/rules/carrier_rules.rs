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
    /// The instrumentation scope's version, for a clause that narrows on it - a producer can change a
    /// carrier's meaning between releases, and no other dimension can express that.
    pub scope_version: Option<&'a str>,
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
    /// Clause ids this one beats where their languages overlap and neither contains the other.
    pub supersedes: Vec<String>,
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
    /// A dimension the *query-time* resolver is not given, so a clause using it could not hold there.
    UnavailableDimension {
        clause: String,
        dimension: &'static str,
    },
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
    /// An empty name or prefix, which would match every observation of a span.
    EmptyLiteral {
        clause: String,
    },
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnavailableDimension { clause, dimension } => write!(
                f,
                "carrier clause `{clause}` is qualified by `{dimension}`, which the query-time resolver is \
                 given only as the *display* span name - so the clause would hold during ingestion and fail \
                 on the same span when read"
            ),
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
            Self::EmptyLiteral { clause } => write!(
                f,
                "clause `{clause}` has an empty name or prefix, which would match every observation"
            ),
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
    if let Some(v) = facts.carrier_holds_span_input {
        semantics.carrier_holds_span_input = v;
    }
    if let Some(v) = facts.carrier_holds_expandable_message_array {
        semantics.carrier_holds_expandable_message_array = v;
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
                supersedes,
            } = rule;
            if seen_ids.insert(id.clone(), ()).is_some() {
                return Err(CompileError::DuplicateClauseId { clause: id.clone() });
            }
            // Exactly one, not at least one: a clause naming several was silently reduced to whichever
            // the indexer happened to read first, so the others were accepted and ignored.
            if match_spec.primary_key_count() != 1 {
                return Err(CompileError::NoPrimaryKey { clause: id.clone() });
            }
            // An empty carrier name or prefix matches every observation of a span.
            if id.is_empty()
                || match_spec.primary_key().is_some_and(|k| {
                    matches!(
                        k,
                        PrimaryKey::Event("")
                            | PrimaryKey::Attribute("")
                            | PrimaryKey::AttributePrefix("")
                    )
                })
                || match_spec.observation_type.iter().any(String::is_empty)
                || match_spec.span_name_prefix.as_deref() == Some("")
                || match_spec.scope_name_contains.as_deref() == Some("")
                || match_spec.scope_version_prefix.as_deref() == Some("")
            {
                return Err(CompileError::EmptyLiteral { clause: id.clone() });
            }
            // Carrier semantics are resolved at **query** time, from a stored row - and the stored `span_name`
            // is the *display* name, which for a dialect that writes an unresolved template is not the name the
            // producer sent. So a clause qualified by span-name prefix would hold during ingestion and fail on
            // the same span when read, and the generic reading would win. Refused rather than documented,
            // because a declaration that cannot work is the class this engine exists to remove; it becomes
            // expressible once the raw name is persisted beside the display name.
            if match_spec.span_name_prefix.is_some() {
                return Err(CompileError::UnavailableDimension {
                    clause: id.clone(),
                    dimension: "span_name_prefix",
                });
            }
            clauses.push(CompiledClause {
                rule_file: file.id.clone(),
                clause_id: id.clone(),
                doc: doc.clone(),
                match_spec: match_spec.clone(),
                semantics: resolve_facts(id, facts)?,
                ordering_family: ordering_family.clone(),
                supersedes: supersedes.clone(),
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

    // Most specific first, so a lookup can stop at its first hit within a bucket. The order is
    // subsumption: a clause whose language is contained in another's comes first. `sort_by` needs a total
    // order and subsumption is only a partial one, so this sorts by *how many* clauses contain a given
    // one - a linear extension of the partial order, which is enough because the compiler has already
    // refused every overlapping pair that subsumption cannot separate.
    let containment_rank = |clause: &CompiledClause, bucket: &[CompiledClause]| -> usize {
        bucket
            .iter()
            .filter(|other| {
                other.clause_id != clause.clause_id
                    && other.match_spec.contains_language_of(&clause.match_spec)
            })
            .count()
    };
    for bucket in plan
        .by_event
        .values_mut()
        .chain(plan.by_attribute.values_mut())
    {
        let ranks: Vec<usize> = bucket
            .iter()
            .map(|clause| containment_rank(clause, bucket))
            .collect();
        let mut paired: Vec<(usize, CompiledClause)> =
            ranks.into_iter().zip(bucket.drain(..)).collect();
        paired.sort_by_key(|(rank, _)| std::cmp::Reverse(*rank));
        *bucket = paired.into_iter().map(|(_, clause)| clause).collect();
    }
    let ranks: Vec<usize> = plan
        .by_attribute_prefix
        .iter()
        .map(|clause| containment_rank(clause, &plan.by_attribute_prefix))
        .collect();
    let mut paired: Vec<(usize, CompiledClause)> = ranks
        .into_iter()
        .zip(plan.by_attribute_prefix.drain(..))
        .collect();
    paired.sort_by_key(|(rank, _)| std::cmp::Reverse(*rank));
    plan.by_attribute_prefix = paired.into_iter().map(|(_, clause)| clause).collect();

    Ok(plan)
}

/// Refuse a ruleset in which two clauses could match one observation with nothing to separate them.
///
/// The test is **subsumption**, not a score: two clauses may both match, provided one's match language is
/// contained in the other's, because then "the more specific" is a fact rather than an arithmetic
/// accident. Where the languages overlap and neither contains the other, the ruleset is genuinely
/// ambiguous and one clause must say in data that it supersedes the other.
///
/// Bounded, and worth stating precisely: `can_both_match` is conservative, so a pair it cannot prove
/// disjoint is reported. That errs toward asking for an explicit decision rather than assuming
/// independence - which is what the previous version got wrong about two `scope_name_contains` needles,
/// since one scope name can hold both.
fn reject_ambiguity(clauses: &[CompiledClause]) -> Result<(), CompileError> {
    for (i, a) in clauses.iter().enumerate() {
        for b in &clauses[i + 1..] {
            if !a.match_spec.can_both_match(&b.match_spec) {
                continue;
            }
            // Containment must be *strict* in one direction. Two identical specs each contain the
            // other, so a non-strict test read them as ordered when in fact nothing separates them -
            // which is the exact collision this check exists to catch.
            let a_in_b = a.match_spec.contains_language_of(&b.match_spec);
            let b_in_a = b.match_spec.contains_language_of(&a.match_spec);
            if a_in_b != b_in_a {
                continue; // one is strictly more specific; that is an order, not a collision
            }
            if a.supersedes.contains(&b.clause_id) || b.supersedes.contains(&a.clause_id) {
                continue; // declared, by name, in data
            }
            return Err(CompileError::Ambiguous {
                first: a.clause_id.clone(),
                second: b.clause_id.clone(),
                key: format!("{:?}", a.match_spec.primary_key()),
            });
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
        if let Some(prefix) = &spec.scope_version_prefix {
            match ctx.scope_version {
                Some(version) if version.starts_with(prefix.as_str()) => {}
                _ => return false,
            }
        }
        true
    }

    /// The clause that governs this observation: the most specific one that matches it.
    ///
    /// Every candidate is gathered *before* one is chosen. Returning the first exact-attribute hit meant
    /// a qualified prefix clause - one naming the observation type, say - lost to an unqualified exact
    /// clause it should have beaten, because the two were never compared. Specificity is a property of a
    /// pair of clauses, so it cannot be applied by looking in one bucket at a time.
    pub fn resolve(&self, ctx: &CarrierContext<'_>) -> Option<&CompiledClause> {
        // The more specific of the two wins. The compiler has already refused every overlapping pair that
        // subsumption cannot separate, so this comparison always decides.
        fn better<'c>(
            candidate: &'c CompiledClause,
            best: Option<&'c CompiledClause>,
        ) -> Option<&'c CompiledClause> {
            match best {
                Some(current)
                    if !current
                        .match_spec
                        .contains_language_of(&candidate.match_spec) =>
                {
                    Some(current)
                }
                _ => Some(candidate),
            }
        }

        let mut best: Option<&CompiledClause> = None;
        if let Some(event) = ctx.event
            && let Some(bucket) = self.by_event.get(event)
        {
            for clause in bucket.iter().filter(|c| Self::qualifiers_hold(c, ctx)) {
                best = better(clause, best);
            }
        }
        if let Some(attribute) = ctx.attribute {
            if let Some(bucket) = self.by_attribute.get(attribute) {
                for clause in bucket.iter().filter(|c| Self::qualifiers_hold(c, ctx)) {
                    best = better(clause, best);
                }
            }
            for clause in self.by_attribute_prefix.iter().filter(|c| {
                c.match_spec
                    .attribute_prefix
                    .as_deref()
                    .is_some_and(|p| attribute.starts_with(p))
                    && Self::qualifiers_hold(c, ctx)
            }) {
                best = better(clause, best);
            }
        }
        best
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
