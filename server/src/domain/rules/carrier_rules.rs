//! Carrier semantics as data: the compiled plan, and the lookup the pipeline calls.
//!
//! This replaces a span-blind table in Rust: a carrier's semantics can now depend on the *span* carrying it,
//! which the table could not express.
//!
//! **What that did not fix, stated because three places used to say it did.** `gen_ai.output.messages` is
//! classified as one emission wherever it appears, and on an *aggregator* span - a framework's root agent span
//! re-listing the whole turn - it is accumulated state, so reading it as an emission puts the turn's final
//! answer ahead of the tool calls that produced it. That misordering is **still here**. The embedded rules
//! deliberately keep the generic emission reading, and the reason is measured rather than pending: the
//! carrier-local facts cannot distinguish "this span is re-listing a turn" from "this span is the sole witness
//! to it", and a clause that guessed made the second case worse.
//!
//! So what this module adds is the *capability* - a clause may constrain the observation
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
    /// A combination of facts the model cannot mean.
    IncoherentFacts {
        clause: String,
        detail: &'static str,
    },
    /// An observation type that is not one, or a list of them that holds for everything.
    UnknownObservationType {
        clause: String,
        declared: String,
    },
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
            Self::IncoherentFacts { clause, detail } => {
                write!(f, "carrier clause `{clause}` {detail}")
            }
            Self::UnknownObservationType { clause, declared } => write!(
                f,
                "carrier clause `{clause}` is qualified by observation type `{declared}`, which is not one \
                 this server classifies - so the clause could never match"
            ),
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

/// Resolve a preset name to the eight facts, then apply the clause's overrides, then refuse a vector the
/// model cannot mean.
///
/// **The presets are not a semantic vocabulary**, and saying so here is more use than the names suggest.
/// `snapshot` and `accumulated_state` differ in exactly one bit - whether the carrier holds the span's output -
/// so `{preset: accumulated_state}` and `{preset: snapshot, carrier_holds_span_output: true}` are the same
/// declaration written two ways, and the corpus contains both spellings. Nor does the preset name survive
/// compilation: only the bits do. So they are historical constructors for an eight-bit value rather than
/// categories the engine acts on, and the honest form is orthogonal axes with one spelling each.
fn resolve_facts(
    clause_id: &str,
    facts: &Facts,
    ordering_family: &Option<String>,
) -> Result<CarrierSemantics, CompileError> {
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
    if let Some(detail) = incoherent(&semantics, ordering_family) {
        return Err(CompileError::IncoherentFacts {
            clause: clause_id.to_string(),
            detail,
        });
    }
    Ok(semantics)
}

/// A combination of facts the model cannot mean.
///
/// The eight overrides are applied independently, so any vector at all compiled - including ones where the
/// facts contradict each other and a reader could not say which the engine would act on. Each rule here holds
/// across all 55 shipped clauses, which is what makes it a statement about the model rather than a preference.
///
/// **One implication is deliberately absent: a detached request frame must hold the span's input.** All three
/// shipped frames declare `carrier_holds_span_input: false` while their own docs say they are what the model
/// was given, so the rule is *true of the model and false of the assets*. Correcting the three declarations was
/// tried and measured: that flag also gates **history detection**, not only ordering, so making the declaration
/// true changed what gets *filtered* - four fixtures moved, a span view lost two messages, and an assistant's
/// intro text sorted after its own tool call. The declarations and the ordering consumer have to move together,
/// which is separate work; enforcing the implication now would refuse the shipped ruleset for a defect that is
/// real and not yet safely fixable.
fn incoherent(
    semantics: &CarrierSemantics,
    ordering_family: &Option<String>,
) -> Option<&'static str> {
    if semantics.carrier_is_atomic_emission && !semantics.position_proves_distinct_occurrence {
        return Some(
            "is one atomic emission and says its positions do not prove distinct occurrences - the whole \
             point of an emission is that each position in it is a separate thing that happened",
        );
    }
    if semantics.carrier_is_atomic_emission && semantics.carrier_may_contain_history_or_state {
        return Some(
            "is one atomic emission and may contain history - an emission is what this span produced now, so \
             it cannot also be a re-listing of earlier turns",
        );
    }
    if semantics.carrier_is_detached_request_frame && semantics.carrier_holds_span_output {
        return Some(
            "is a detached request frame and holds the span's output - a frame precedes what the request saw, \
             so it is on the input side by definition",
        );
    }
    if ordering_family.is_some() {
        if !semantics.position_provides_sequence_order {
            return Some(
                "names an ordering family and says its positions carry no sequence order, so the family \
                 orders nothing",
            );
        }
        if semantics.carrier_holds_span_output {
            return Some(
                "names an ordering family and holds the span's output - the families order a request's \
                 *inputs*, so the resolver never reads it",
            );
        }
        if !semantics.carrier_holds_span_input {
            return Some(
                "names an ordering family and does not hold the span's input, which is the side those \
                 families order",
            );
        }
    }
    if semantics.carrier_holds_expandable_message_array
        && !semantics.position_provides_sequence_order
    {
        return Some(
            "expands into one observation per message and says its positions carry no sequence order - the \
             expansion is what gives each message its position",
        );
    }
    None
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
                || match_spec
                    .observation_type
                    .iter()
                    .flatten()
                    .any(String::is_empty)
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
            // The observation types must be ones that exist, and there must be some. A misspelling compiled
            // and could never match; an explicitly empty list was silently the same as omitting the qualifier,
            // so a clause that reads as narrow held for every span. The vocabulary is the classifier's own, so
            // this reads it rather than keeping a second copy.
            if let Some(types) = &match_spec.observation_type {
                if types.is_empty() {
                    return Err(CompileError::UnknownObservationType {
                        clause: id.clone(),
                        declared: "an empty list, which holds for every span".to_string(),
                    });
                }
                let mut seen: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
                for declared in types {
                    if !super::classify::OBSERVATION_TYPES.contains(&declared.as_str()) {
                        return Err(CompileError::UnknownObservationType {
                            clause: id.clone(),
                            declared: declared.clone(),
                        });
                    }
                    if !seen.insert(declared.as_str()) {
                        return Err(CompileError::UnknownObservationType {
                            clause: id.clone(),
                            declared: format!("{declared} (named twice)"),
                        });
                    }
                }
            }
            // The **scope** dimensions are refused for the same reason, and it is not hypothetical: the
            // ingestion-side read of `carrier_holds_span_output` supplies no scope, while query-time resolution
            // supplies the persisted one. So a clause qualified by scope selects the generic clause at ingestion
            // and its own at read time - and those two clauses can disagree about whether the carrier holds the
            // span's output, which decides whether a generation span's answer is augmented. Refused until scope
            // reaches every consumer, rather than left as a dimension that answers differently depending on who
            // asks.
            for (dimension, declared) in [
                (
                    "scope_name_contains",
                    match_spec.scope_name_contains.is_some(),
                ),
                (
                    "scope_version_prefix",
                    match_spec.scope_version_prefix.is_some(),
                ),
            ] {
                if declared {
                    return Err(CompileError::UnavailableDimension {
                        clause: id.clone(),
                        dimension,
                    });
                }
            }
            clauses.push(CompiledClause {
                rule_file: file.id.clone(),
                clause_id: id.clone(),
                doc: doc.clone(),
                match_spec: match_spec.clone(),
                semantics: resolve_facts(id, facts, ordering_family)?,
                ordering_family: ordering_family.clone(),
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
            // One bucket for both, because both are scanned rather than looked up: a family is a prefix with
            // the separator respected, so nothing about the *indexing* differs.
            Some(PrimaryKey::AttributePrefix(_) | PrimaryKey::AttributeFamily(_)) => {
                plan.by_attribute_prefix.push(clause);
            }
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
/// ambiguous, and the specs must be made strictly ordered rather than the pair being waived.
///
/// There **was** a waiver - a `supersedes` list on a carrier clause - and it is gone. It licensed the
/// ambiguity without resolving it: compilation accepted the pair and `better()` never read the edge, so
/// whichever incomparable clause was declared first still won and reordering the declarations changed the
/// answer. No asset used it. A refusal an author cannot silence is the honest form here: two clauses that
/// match the same span and neither of which is more specific have no order to discover.
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
        if let Some(types) = &spec.observation_type {
            match ctx.observation_type {
                Some(observed) => {
                    if !types.iter().any(|t| t == observed) {
                        return false;
                    }
                }
                None => return false,
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
                // Through the source itself, so the raw-prefix and family readings cannot diverge here from the
                // ones specificity is computed with.
                c.match_spec
                    .primary_key()
                    .is_some_and(|key| key.selects_attribute(attribute))
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
