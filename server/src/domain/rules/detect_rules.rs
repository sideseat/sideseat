//! Detection as data: which producer a span came from, decided by declared signals.
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

use super::schema::{DetectMatch, KeyValue, RuleFile, TextContains};

/// A compiled detection rule.
#[derive(Debug, Clone)]
pub struct CompiledDetect {
    pub rule_file: String,
    pub rule_id: String,
    pub doc: Option<String>,
    pub label: String,
    pub legacy_rank: i32,
    pub supersedes: Vec<String>,
    pub match_spec: DetectMatch,
    /// `text_contains` sources split once, at compile time, into "the span name" and attribute keys.
    span_name_is_a_text_source: bool,
    text_attribute_keys: Vec<String>,
    /// Needles pre-lowercased, so a match does not lower-case a constant per span.
    text_needles_lowered: Vec<String>,
    /// Search only the first source that has a value. See `TextContains::first_present_source`.
    text_first_present_source: bool,
}

/// The compiled detection plan: rules in rank order, plus the declaration fallback.
#[derive(Debug, Default)]
pub struct DetectPlan {
    rules: Vec<CompiledDetect>,
    /// Rule ids some rule claims to beat, so `resolve` can keep its early exit for everything else.
    superseded: std::collections::BTreeSet<String>,
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
    pub span_attrs: &'a HashMap<String, String>,
    pub resource_attrs: &'a HashMap<String, String>,
}

/// Why a detection ruleset would not compile.
#[derive(Debug)]
pub enum DetectCompileError {
    Parse {
        path: String,
        message: String,
    },
    EmptyLiteral {
        rule: String,
        dimension: &'static str,
    },
    /// A literal another in the same list already covers, so it can never be why a rule matched.
    SubsumedLiteral {
        rule: String,
        dimension: &'static str,
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
    DuplicateRank {
        first: String,
        second: String,
    },
    NoSignal {
        rule: String,
    },
    DuplicateSlug {
        slug: String,
    },
    /// A `supersedes` edge that cannot take effect.
    UselessSupersedes {
        rule: String,
        target: String,
        detail: &'static str,
    },
    BadTextSource {
        rule: String,
        source: String,
    },
}

impl std::fmt::Display for DetectCompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Parse { path, message } => write!(f, "{path}: {message}"),
            Self::SlugLabelNoRuleProduces { slug, label } => write!(
                f,
                "SDK slug `{slug}` resolves to `{label}`, which no detection rule produces - the two sections \
                 fill the label independently, so this is a second framework name for one producer, and which \
                 one a span gets depends on whether its signals were detected or its SDK declared itself"
            ),
            Self::ShadowedRule { earlier, later } => write!(
                f,
                "rule `{later}` can never be reached: `{earlier}` is ranked ahead of it and every span `{later}` \
                 matches satisfies `{earlier}` too, so `{later}`'s answer is unreachable and reads as protection \
                 it does not give"
            ),
            Self::SubsumedLiteral {
                rule,
                dimension,
                dead,
                covering,
            } => write!(
                f,
                "detection rule `{rule}` declares `{dead}` in `{dimension}`, which `{covering}` in the same \
                 list already covers - the broader literal always fires first, so this one can never be why \
                 the rule matched, and it reads as precision the rule does not have"
            ),
            Self::UselessSupersedes {
                rule,
                target,
                detail,
            } => write!(
                f,
                "detection rule `{rule}` supersedes `{target}`, which {detail}. `supersedes` decides which \
                 of two matching rules wins, ahead of `legacy_rank`, and waives the overlap report that names \
                 which predicates are not yet sufficient. An edge that cannot take effect reads as one that does"
            ),
            Self::EmptyLiteral { rule, dimension } => write!(
                f,
                "detection rule `{rule}` has an empty value in `{dimension}`, which matches everything"
            ),
            Self::DuplicateRuleId { rule } => {
                write!(f, "detection rule id `{rule}` is declared more than once")
            }
            Self::DuplicateRank { first, second } => write!(
                f,
                "detection rules `{first}` and `{second}` share a rank: their relative order would \
                 depend on load order, and detection order is policy"
            ),
            Self::NoSignal { rule } => write!(
                f,
                "detection rule `{rule}` declares no signal, so it would claim every span"
            ),
            Self::DuplicateSlug { slug } => write!(
                f,
                "SDK slug `{slug}` is claimed by more than one framework, so a declaration naming it \
                 has no single answer"
            ),
            Self::BadTextSource { rule, source } => write!(
                f,
                "detection rule `{rule}` names text source `{source}`: expected `span_name` or \
                 `attr:<key>`"
            ),
        }
    }
}

/// Does this match spec declare any signal at all?
fn has_signal(spec: &DetectMatch) -> bool {
    !spec.span_name.is_empty()
        || !spec.span_name_exact.is_empty()
        || !spec.attr_prefix.is_empty()
        || !spec.attr_equals.is_empty()
        || !spec.attr_equals_ignore_case.is_empty()
        || !spec.attr_exists.is_empty()
        || !spec.service_name.is_empty()
        || !spec.span_attr_contains.is_empty()
        || !spec.resource_attr_contains.is_empty()
        || spec.text_contains.is_some()
}

/// Compile every asset's detection rules into one ordered plan.
pub fn compile(sources: &BTreeMap<String, Vec<u8>>) -> Result<DetectPlan, DetectCompileError> {
    let mut rules: Vec<CompiledDetect> = Vec::new();
    let mut seen_ids: HashMap<String, ()> = HashMap::new();
    let mut plan = DetectPlan::default();

    for (path, bytes) in sources {
        // Parsed, not skipped. Relying on the carrier compile to have rejected the same bytes made this
        // pass silently depend on another function's error path - and a caller compiling detection alone
        // would have quietly ignored a malformed asset.
        let file: RuleFile =
            serde_json::from_slice(bytes).map_err(|e| DetectCompileError::Parse {
                path: path.clone(),
                message: e.to_string(),
            })?;
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
        // One body of evidence - a rule's own `match`, or one of its `alternatives` - validated and compiled the
        // same way. Extracted so an alternative cannot get a weaker check than the primary: every refusal below
        // used to sit inline in a loop over rules, and an alternative added beside it would have skipped all of
        // them.
        let compile_one = |id: &str,
                           doc: Option<String>,
                           legacy_rank: i32,
                           label: &str,
                           supersedes: Vec<String>,
                           spec: &DetectMatch|
         -> Result<CompiledDetect, DetectCompileError> {
            if id.is_empty() || label.is_empty() {
                return Err(DetectCompileError::EmptyLiteral {
                    rule: id.to_string(),
                    dimension: "id/label",
                });
            }
            if !has_signal(spec) {
                return Err(DetectCompileError::NoSignal {
                    rule: id.to_string(),
                });
            }
            // Through the shared atom validator, not a copy of it: the two had drifted in both directions.
            if let Some(defect) = atom_literal_defect(spec) {
                // The variant a caller's own diagnostic wants: detection has a dedicated error for an
                // unreadable phrase source, and collapsing every defect into one variant made that
                // diagnostic worse than it was.
                return Err(match defect {
                    AtomDefect::BadTextSource(source) => DetectCompileError::BadTextSource {
                        rule: id.to_string(),
                        source,
                    },
                    // Names the two literals, because "a literal is subsumed" without saying which two sends
                    // the reader to re-derive the covering relation by hand.
                    AtomDefect::SubsumedLiteral {
                        dimension,
                        dead,
                        covering,
                    } => DetectCompileError::SubsumedLiteral {
                        rule: id.to_string(),
                        dimension,
                        dead,
                        covering,
                    },
                    other => DetectCompileError::EmptyLiteral {
                        rule: id.to_string(),
                        dimension: other.dimension(),
                    },
                });
            }
            let (mut span_source, mut attr_keys, mut needles) = (false, Vec::new(), Vec::new());
            if let Some(TextContains {
                sources,
                needles: n,
                first_present_source: _,
            }) = &spec.text_contains
            {
                for source in sources {
                    if source == "span_name" {
                        span_source = true;
                    } else if let Some(key) = source.strip_prefix("attr:") {
                        attr_keys.push(key.to_string());
                    } else {
                        return Err(DetectCompileError::BadTextSource {
                            rule: id.to_string(),
                            source: source.clone(),
                        });
                    }
                }
                needles = n.iter().map(|s| s.to_lowercase()).collect();
            }
            Ok(CompiledDetect {
                rule_file: file.id.clone(),
                rule_id: id.to_string(),
                doc,
                label: label.to_string(),
                legacy_rank,
                supersedes,
                match_spec: spec.clone(),
                span_name_is_a_text_source: span_source,
                text_attribute_keys: attr_keys,
                text_needles_lowered: needles,
                text_first_present_source: spec
                    .text_contains
                    .as_ref()
                    .is_some_and(|t| t.first_present_source),
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
                rule.legacy_rank,
                &rule.label,
                rule.supersedes.clone(),
                &rule.match_spec,
            )?);
            for alternative in &rule.alternatives {
                // The label and the overlap edges are the *rule's*, not the alternative's: an alternative is
                // further evidence for one producer, so declaring its own label would make it a separate rule
                // wearing a rule's id.
                rules.push(compile_one(
                    &alternative.id,
                    alternative.doc.clone(),
                    alternative.legacy_rank,
                    &rule.label,
                    rule.supersedes.clone(),
                    &alternative.match_spec,
                )?);
            }
        }
    }

    rules.sort_by_key(|r| r.legacy_rank);
    // A shared rank is refused: two rules that both match one span would be separated by load order,
    // and the whole reason rank is explicit is that this order is policy somebody has to own.
    for pair in rules.windows(2) {
        if pair[0].legacy_rank == pair[1].legacy_rank {
            return Err(DetectCompileError::DuplicateRank {
                first: pair[0].rule_id.clone(),
                second: pair[1].rule_id.clone(),
            });
        }
    }

    // Every `supersedes` edge must be able to take effect. The field waives the overlap *report*, and that
    // waiver is only consulted for the rule that already won by rank - so an edge from a higher-ranked rule
    // to a lower-ranked one is inspected by nobody, and an edge naming a rule that does not exist or itself
    // is inspected by nobody either. All three compiled silently, which is how a reader comes to believe the
    // field orders things.
    let rank_of: HashMap<&str, i32> = rules
        .iter()
        .map(|rule| (rule.rule_id.as_str(), rule.legacy_rank))
        .collect();
    for rule in &rules {
        let mut seen: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
        for target in &rule.supersedes {
            if !seen.insert(target.as_str()) {
                return Err(DetectCompileError::UselessSupersedes {
                    rule: rule.rule_id.clone(),
                    target: target.clone(),
                    detail: "is named twice by this rule",
                });
            }
            let detail = if target == &rule.rule_id {
                Some("is the rule itself")
            } else {
                match rank_of.get(target.as_str()) {
                    None => Some("no asset declares"),
                    // An edge pointing at a rule that **outranks** this one used to be refused, because the
                    // waiver was only read from whichever rule rank had already made the winner. Now that
                    // `supersedes` orders, that edge is the useful case: it is how a rule beats one ranked ahead
                    // of it without moving its own weaker signals up with it.
                    Some(_) => None,
                }
            };
            if let Some(detail) = detail {
                return Err(DetectCompileError::UselessSupersedes {
                    rule: rule.rule_id.clone(),
                    target: target.clone(),
                    detail,
                });
            }
        }
    }

    // A rule an earlier one always satisfies first can never answer. The same defect as a subsumed literal, one
    // level up - and a *detection* rule shadowed this way silently never attributes its producer at all.
    // Transitive domination, computed here because a rule that **supersedes** its shadower is reachable after all:
    // `supersedes` orders ahead of rank, so the broader rule loses to it. Without this the refusal rejected exactly
    // the shape `supersedes` exists for - a narrow rule ranked after the broad one it beats.
    let beats = |from: &str| -> std::collections::BTreeSet<&str> {
        let mut out: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
        let mut queue: Vec<&str> = vec![from];
        while let Some(current) = queue.pop() {
            let Some(rule) = rules.iter().find(|r| r.rule_id == current) else {
                continue;
            };
            for target in &rule.supersedes {
                if out.insert(target.as_str()) {
                    queue.push(target.as_str());
                }
            }
        }
        out
    };
    for (index, earlier) in rules.iter().enumerate() {
        for later in &rules[index + 1..] {
            if shadows(
                std::slice::from_ref(&earlier.match_spec),
                std::slice::from_ref(&later.match_spec),
            ) && !beats(later.rule_id.as_str()).contains(earlier.rule_id.as_str())
            {
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

    // Every id some rule claims to beat, so `resolve` keeps its early exit for the rules nothing contests.
    plan.superseded = rules
        .iter()
        .flat_map(|rule| rule.supersedes.iter().cloned())
        .collect();
    plan.rules = rules;
    Ok(plan)
}

/// A signal set compiled once, for a message rule's gate.
///
/// Built at compile time rather than per observation. The previous form cloned the predicate, rebuilt its
/// lowered needles and allocated an empty map on *every* gate evaluation - which is the opposite of what a
/// "typed plan compiled once" is for.
/// The retired shell's answer for a span, reachable from the oracle that holds the boolean grammar to it.
#[cfg(test)]
pub(crate) fn signals_hold_for_test(
    spec: &DetectMatch,
    span_name: &str,
    span_attrs: &HashMap<String, String>,
) -> bool {
    compiled_signals_hold(&compile_signals(spec), span_name, span_attrs)
}

pub(super) fn compile_signals(spec: &DetectMatch) -> CompiledDetect {
    probe_for(spec)
}

/// Whether a compiled signal set holds for a span.
///
/// One definition, shared with detection: two implementations of "does this span carry this marker" would
/// drift, and invisibly - a rule would claim a carrier on a span detection did not attribute to that
/// dialect.
pub(super) fn compiled_signals_hold(
    probe: &CompiledDetect,
    span_name: &str,
    span_attrs: &HashMap<String, String>,
) -> bool {
    // A message gate sees no resource attributes; dimensions that need them are refused at compile time,
    // so an empty map here cannot silently change an answer.
    static NO_RESOURCE: std::sync::OnceLock<HashMap<String, String>> = std::sync::OnceLock::new();
    probe.matches(&DetectContext {
        span_name,
        span_attrs,
        resource_attrs: NO_RESOURCE.get_or_init(HashMap::new),
    })
}

/// Which gate dimensions a message rule cannot use, because a message gate is given no resource
/// attributes - so such a predicate would be accepted and never hold.
pub(super) fn unavailable_gate_dimension(spec: &DetectMatch) -> Option<&'static str> {
    if !spec.service_name.is_empty() {
        return Some("service_name");
    }
    if !spec.resource_attr_contains.is_empty() {
        return Some("resource_attr_contains");
    }
    None
}

/// A first-present search over *mixed* sources, which cannot be honoured.
///
/// Compilation splits sources into "the span name" and a list of attribute keys, so the declared order between
/// the two is lost and the span name is always reached first. Refused rather than silently reordered - no asset
/// needs the mix, and the fix if one ever does is to compile an ordered list of sources rather than a flag and a
/// list. One definition, because the two compile paths refused it inconsistently.
pub(super) fn mixed_first_present_sources(spec: &DetectMatch) -> bool {
    spec.text_contains.as_ref().is_some_and(|text| {
        text.first_present_source
            && text.sources.iter().any(|s| s == "span_name")
            && text.sources.iter().any(|s| s != "span_name")
    })
}

/// Why a gate could never hold, where that is decidable from the declaration alone.
///
/// A gate is a *disjunction*, so an empty one is not "match anything" - it is `false`, and every rule carrying
/// it is dead. An empty needle inside a dimension is the opposite mistake: `span_name: [""]` matches every
/// span by prefix and `attr_exists: [""]` names a key nothing writes, so one is far broader than it reads and
/// the other narrower. Both compiled silently.
/// The literal defects of a span-match atom, wherever it is declared.
///
/// **One validator for detection and for every gate.** They were two, and had already drifted apart in both
/// directions: an empty `attr:` source was accepted by detection and refused by a gate, and an empty substring
/// *value* was refused by neither. A predicate whose meaning depends on where it was written is not a
/// predicate.
pub(super) enum AtomDefect {
    /// A literal that matches everything or nothing, on the named dimension.
    EmptyLiteral(&'static str),
    /// A phrase search naming a source the probe does not read, and which one.
    BadTextSource(String),
    /// A phrase search with nothing to search for.
    NoNeedle,
    /// A first-present search mixing the span name with attributes, whose order compilation loses.
    MixedFirstPresentSources,
    /// A substring search for `""`, which every present value contains.
    EmptySubstring(&'static str),
    /// A literal another literal in the same list already covers, so it can never be why a rule matched.
    SubsumedLiteral {
        dimension: &'static str,
        dead: String,
        covering: String,
    },
}

/// How one literal can cover another within a dimension.
#[derive(Debug, Clone, Copy)]
enum Subsumption {
    /// The covering literal is a prefix of the dead one.
    Prefix,
    /// The covering literal appears inside the dead one.
    Contains,
    /// Only an identical literal covers - a list of exact matches.
    Equal,
}

/// One condition a `DetectMatch` states, as the pair a comparison needs.
///
/// Flattened so implication between two rules can be asked atom by atom. `text_contains` is deliberately
/// **omitted**: its `first_present_source` mode searches only the first source that has a value, so whether one
/// phrase search implies another depends on which attributes a span happens to carry - and a wrong answer here
/// refuses a legitimate ruleset at build time, which is worse than missing a shadow.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Atom<'a> {
    SpanNamePrefix(&'a str),
    SpanNameExact(&'a str),
    AttrPrefix(&'a str),
    AttrExists(&'a str),
    ServiceNameContains(&'a str),
    AttrEquals(&'a str, &'a str),
    AttrEqualsIgnoreCase(&'a str, &'a str),
    SpanAttrContains(&'a str, &'a str),
    ResourceAttrContains(&'a str, &'a str),
}

impl<'a> Atom<'a> {
    /// The attribute key this condition is about, where it is about one.
    fn key(&self) -> Option<&'a str> {
        match self {
            Self::AttrExists(key)
            | Self::AttrEquals(key, _)
            | Self::AttrEqualsIgnoreCase(key, _)
            | Self::SpanAttrContains(key, _) => Some(key),
            _ => None,
        }
    }

    /// Whether satisfying `self` necessarily satisfies `other`.
    ///
    /// Conservative by construction: every arm is a containment or equality that holds for *every* span, never a
    /// judgement about which attributes a span carries. Anything not listed answers `false`, so an unrecognised
    /// pair means "no shadow proven" rather than a refusal nobody can explain.
    fn implies(&self, other: &Atom<'_>) -> bool {
        if self == other {
            return true;
        }
        match other {
            // Anything that reads a key proves the key is there.
            Atom::AttrExists(key) => self.key() == Some(*key),
            // A key under a longer prefix is under the shorter one.
            Atom::AttrPrefix(prefix) => self
                .key()
                .or(match self {
                    Atom::AttrPrefix(mine) => Some(mine),
                    _ => None,
                })
                .is_some_and(|key| key.starts_with(prefix)),
            Atom::SpanNamePrefix(prefix) => match self {
                Atom::SpanNameExact(name) | Atom::SpanNamePrefix(name) => name.starts_with(prefix),
                _ => false,
            },
            Atom::AttrEqualsIgnoreCase(key, value) => match self {
                Atom::AttrEquals(mine, mine_value) => {
                    mine == key && mine_value.eq_ignore_ascii_case(value)
                }
                _ => false,
            },
            Atom::ServiceNameContains(needle) => match self {
                Atom::ServiceNameContains(mine) => mine.contains(needle),
                _ => false,
            },
            Atom::SpanAttrContains(key, needle) => match self {
                Atom::SpanAttrContains(mine, mine_needle) => {
                    mine == key && mine_needle.contains(needle)
                }
                Atom::AttrEquals(mine, value) => mine == key && value.contains(needle),
                _ => false,
            },
            Atom::ResourceAttrContains(key, needle) => match self {
                Atom::ResourceAttrContains(mine, mine_needle) => {
                    mine == key && mine_needle.contains(needle)
                }
                _ => false,
            },
            _ => false,
        }
    }
}

/// Every condition a match spec states, flattened. `None` where it states one this cannot compare.
fn atoms_of(spec: &DetectMatch) -> Option<Vec<Atom<'_>>> {
    if spec.text_contains.is_some() {
        return None;
    }
    let mut out: Vec<Atom<'_>> = Vec::new();
    out.extend(spec.span_name.iter().map(|s| Atom::SpanNamePrefix(s)));
    out.extend(spec.span_name_exact.iter().map(|s| Atom::SpanNameExact(s)));
    out.extend(spec.attr_prefix.iter().map(|s| Atom::AttrPrefix(s)));
    out.extend(spec.attr_exists.iter().map(|s| Atom::AttrExists(s)));
    out.extend(
        spec.service_name
            .iter()
            .map(|s| Atom::ServiceNameContains(s)),
    );
    out.extend(
        spec.attr_equals
            .iter()
            .map(|kv| Atom::AttrEquals(&kv.key, &kv.value)),
    );
    out.extend(
        spec.attr_equals_ignore_case
            .iter()
            .map(|kv| Atom::AttrEqualsIgnoreCase(&kv.key, &kv.value)),
    );
    out.extend(
        spec.span_attr_contains
            .iter()
            .map(|kv| Atom::SpanAttrContains(&kv.key, &kv.value)),
    );
    out.extend(
        spec.resource_attr_contains
            .iter()
            .map(|kv| Atom::ResourceAttrContains(&kv.key, &kv.value)),
    );
    (!out.is_empty()).then_some(out)
}

/// Whether `later` can never be reached because `earlier` always holds first.
///
/// A rule that cannot fire reads as protection it does not give - the same defect as a subsumed literal, one level
/// up. `earlier` is a single disjunctive set, so it holds if **any** of its conditions does; `later` needs every
/// one of its conjuncts. So it suffices that *one* conjunct of `later` has all its conditions implying something
/// in `earlier`: whichever of them a span satisfies, `earlier` was already satisfied by it.
///
/// Sound rather than complete, deliberately, in both directions: a multi-conjunct `earlier` is not analysed, a
/// phrase search on either side is not analysed, and an unrecognised implication answers no. A false refusal
/// breaks a build for a reason nobody can act on; a missed shadow leaves things as they were.
pub(super) fn shadows(earlier: &[DetectMatch], later: &[DetectMatch]) -> bool {
    let [earlier] = earlier else {
        return false;
    };
    let Some(covering) = atoms_of(earlier) else {
        return false;
    };
    later.iter().any(|conjunct| {
        atoms_of(conjunct).is_some_and(|conditions| {
            conditions
                .iter()
                .all(|condition| covering.iter().any(|target| condition.implies(target)))
        })
    })
}

/// The first literal another in the same list already covers, as `(dead, covering)`.
fn subsumed_literal(values: &[String], kind: Subsumption) -> Option<(String, String)> {
    for (index, dead) in values.iter().enumerate() {
        for (other, covering) in values.iter().enumerate() {
            if index == other {
                continue;
            }
            let covered = match kind {
                Subsumption::Prefix => dead.starts_with(covering.as_str()),
                Subsumption::Contains => dead.contains(covering.as_str()),
                // Duplicates only. A later identical entry adds nothing, and the earlier one covers it - so the
                // *second* is the dead one, which is why the index comparison decides the tie.
                Subsumption::Equal => dead == covering && other < index,
            };
            if covered && !(matches!(kind, Subsumption::Equal) && dead != covering) {
                return Some((dead.clone(), covering.clone()));
            }
        }
    }
    None
}

impl AtomDefect {
    /// The dimension a diagnostic should name.
    pub(super) fn dimension(&self) -> &'static str {
        match self {
            Self::EmptyLiteral(dimension)
            | Self::EmptySubstring(dimension)
            | Self::SubsumedLiteral { dimension, .. } => dimension,
            Self::BadTextSource(_) | Self::NoNeedle => "text_contains",
            Self::MixedFirstPresentSources => "text_contains.first_present_source",
        }
    }

    /// Why, for a caller whose error type carries prose rather than a variant per cause.
    pub(super) fn reason(&self) -> &'static str {
        match self {
            Self::EmptyLiteral(_) => {
                "names an empty span-name prefix, attribute key or service name, which matches either \
                 everything or nothing rather than what it reads as"
            }
            Self::EmptySubstring(_) => {
                "searches for an empty substring, which every present value contains"
            }
            Self::BadTextSource(_) => {
                "declares a phrase search over a source that is neither `span_name` nor `attr:<key>` - so \
                 it can never hold"
            }
            Self::NoNeedle => {
                "declares a phrase search with no needle or no source, so it can never hold"
            }
            Self::MixedFirstPresentSources => {
                "searches the first source that has a value over a mix of the span name and attributes, and \
                 the declared order between those two is not preserved - name them separately, or use one \
                 kind"
            }
            Self::SubsumedLiteral { .. } => {
                "names a literal another literal in the same list already covers, so it can never be why the \
                 rule matched - remove it, or move it to the dimension that makes it mean something"
            }
        }
    }
}

pub(super) fn atom_literal_defect(spec: &DetectMatch) -> Option<AtomDefect> {
    for (dimension, values) in [
        ("span_name", &spec.span_name),
        ("span_name_exact", &spec.span_name_exact),
        ("attr_prefix", &spec.attr_prefix),
        ("attr_exists", &spec.attr_exists),
        ("service_name", &spec.service_name),
    ] {
        if values.iter().any(String::is_empty) {
            return Some(AtomDefect::EmptyLiteral(dimension));
        }
    }
    for (dimension, pairs) in [
        ("attr_equals", &spec.attr_equals),
        ("attr_equals_ignore_case", &spec.attr_equals_ignore_case),
        ("span_attr_contains", &spec.span_attr_contains),
        ("resource_attr_contains", &spec.resource_attr_contains),
    ] {
        if pairs.iter().any(|kv| kv.key.is_empty()) {
            return Some(AtomDefect::EmptyLiteral(dimension));
        }
    }
    // An empty **value** is refused for the substring dimensions and stays legal for equality. Every present
    // string contains `""`, so `span_attr_contains` with an empty value matches every span carrying the key at
    // all - a narrow rule turned catch-all wherever its rank sits. An attribute that genuinely holds the empty
    // string is something a producer can write, so `attr_equals` keeps it.
    for (dimension, pairs) in [
        ("span_attr_contains", &spec.span_attr_contains),
        ("resource_attr_contains", &spec.resource_attr_contains),
    ] {
        if pairs.iter().any(|kv| kv.value.is_empty()) {
            return Some(AtomDefect::EmptySubstring(dimension));
        }
    }
    // A literal another literal in the same list already covers can never be the reason a rule matched: the
    // broader one always fires first. Two shipped declarations were exactly that - `"LangGraph."` beside
    // `"LangGraph"` in a prefix list, and `"\"langgraph_"` beside `"langgraph_"` in a substring one - and both
    // read as precision the rule did not have.
    for (dimension, values, kind) in [
        ("span_name", &spec.span_name, Subsumption::Prefix),
        ("span_name_exact", &spec.span_name_exact, Subsumption::Equal),
        ("attr_prefix", &spec.attr_prefix, Subsumption::Prefix),
        ("attr_exists", &spec.attr_exists, Subsumption::Equal),
        ("service_name", &spec.service_name, Subsumption::Contains),
    ] {
        if let Some((dead, covering)) = subsumed_literal(values, kind) {
            return Some(AtomDefect::SubsumedLiteral {
                dimension,
                dead,
                covering,
            });
        }
    }
    for (dimension, pairs) in [
        ("span_attr_contains", &spec.span_attr_contains),
        ("resource_attr_contains", &spec.resource_attr_contains),
    ] {
        // Per key: two substrings of *different* attributes say nothing about each other.
        let mut by_key: std::collections::BTreeMap<&str, Vec<String>> =
            std::collections::BTreeMap::new();
        for pair in pairs.iter() {
            by_key
                .entry(&pair.key)
                .or_default()
                .push(pair.value.clone());
        }
        for values in by_key.values() {
            if let Some((dead, covering)) = subsumed_literal(values, Subsumption::Contains) {
                return Some(AtomDefect::SubsumedLiteral {
                    dimension,
                    dead,
                    covering,
                });
            }
        }
    }
    if mixed_first_present_sources(spec) {
        return Some(AtomDefect::MixedFirstPresentSources);
    }
    if let Some(text) = &spec.text_contains {
        // Every source has to be a form the probe reads. A misspelling like `span` is *silently dropped*
        // there, so a search naming only that one can never hold.
        if text.needles.is_empty()
            || text.needles.iter().any(String::is_empty)
            || text.sources.is_empty()
        {
            return Some(AtomDefect::NoNeedle);
        }
        if let Some(source) = text.sources.iter().find(|source| {
            *source != "span_name"
                && source
                    .strip_prefix("attr:")
                    .is_none_or(|key| key.is_empty())
        }) {
            return Some(AtomDefect::BadTextSource(source.clone()));
        }
    }
    None
}

pub(super) fn gate_defect(spec: &DetectMatch) -> Option<&'static str> {
    if let Some(defect) = atom_literal_defect(spec) {
        return Some(defect.reason());
    }
    let any_signal = !spec.span_name.is_empty()
        || !spec.span_name_exact.is_empty()
        || !spec.attr_prefix.is_empty()
        || !spec.attr_equals.is_empty()
        || !spec.attr_equals_ignore_case.is_empty()
        || !spec.attr_exists.is_empty()
        || !spec.service_name.is_empty()
        || !spec.span_attr_contains.is_empty()
        || !spec.resource_attr_contains.is_empty()
        || spec.text_contains.is_some();
    if !any_signal {
        return Some("declares no signal at all, and a gate with no signal never holds");
    }
    None
}

fn probe_for(spec: &DetectMatch) -> CompiledDetect {
    CompiledDetect {
        rule_file: String::new(),
        rule_id: String::new(),
        doc: None,
        label: String::new(),
        legacy_rank: 0,
        supersedes: Vec::new(),
        match_spec: spec.clone(),
        span_name_is_a_text_source: spec
            .text_contains
            .as_ref()
            .is_some_and(|t| t.sources.iter().any(|s| s == "span_name")),
        text_attribute_keys: spec
            .text_contains
            .as_ref()
            .map(|t| {
                t.sources
                    .iter()
                    .filter_map(|s| s.strip_prefix("attr:").map(str::to_string))
                    .collect()
            })
            .unwrap_or_default(),
        text_needles_lowered: spec
            .text_contains
            .as_ref()
            .map(|t| t.needles.iter().map(|n| n.to_lowercase()).collect())
            .unwrap_or_default(),
        text_first_present_source: spec
            .text_contains
            .as_ref()
            .is_some_and(|t| t.first_present_source),
    }
}

impl CompiledDetect {
    /// Any satisfied signal matches. Ordered cheapest-first: an equality probe is a hash lookup, while
    /// the prefix dimensions scan the span's keys.
    fn matches(&self, ctx: &DetectContext<'_>) -> bool {
        let spec = &self.match_spec;

        if spec
            .attr_equals
            .iter()
            .any(|KeyValue { key, value }| ctx.span_attrs.get(key).is_some_and(|v| v == value))
        {
            return true;
        }
        if spec
            .attr_equals_ignore_case
            .iter()
            .any(|KeyValue { key, value }| {
                ctx.span_attrs
                    .get(key)
                    .is_some_and(|v| v.eq_ignore_ascii_case(value))
            })
        {
            return true;
        }
        if spec
            .attr_exists
            .iter()
            .any(|key| ctx.span_attrs.contains_key(key))
        {
            return true;
        }
        if spec
            .span_name_exact
            .iter()
            .any(|name| ctx.span_name == name)
        {
            return true;
        }
        // Prefix only: `starts_with` subsumes its own equality, so the equality arm here could never be the
        // reason a rule matched, and it made a separator-suffixed literal dead beside the bare one.
        if spec
            .span_name
            .iter()
            .any(|prefix| ctx.span_name.starts_with(prefix))
        {
            return true;
        }
        if !spec.service_name.is_empty()
            && let Some(service) = ctx.resource_attrs.get(super::SERVICE_NAME_KEY)
            // Substring, and the equality arm this used to have beside it was dead: `contains` subsumes its own
            // equality. The breadth is deliberate - `my-app-openai-agents-v1` is a service a user names themselves
            // and it identifies the SDK.
            && spec
                .service_name
                .iter()
                .any(|declared| service.contains(declared.as_str()))
        {
            return true;
        }
        if spec
            .resource_attr_contains
            .iter()
            .any(|KeyValue { key, value }| {
                ctx.resource_attrs
                    .get(key)
                    .is_some_and(|v| v.contains(value.as_str()))
            })
        {
            return true;
        }
        if spec
            .span_attr_contains
            .iter()
            .any(|KeyValue { key, value }| {
                ctx.span_attrs
                    .get(key)
                    .is_some_and(|v| v.contains(value.as_str()))
            })
        {
            return true;
        }
        if !spec.attr_prefix.is_empty()
            && ctx
                .span_attrs
                .keys()
                .any(|k| spec.attr_prefix.iter().any(|p| k.starts_with(p.as_str())))
        {
            return true;
        }
        if !self.text_needles_lowered.is_empty() {
            let hit = |text: &str| {
                let lowered = text.to_lowercase();
                self.text_needles_lowered
                    .iter()
                    .any(|n| lowered.contains(n.as_str()))
            };
            // The **first source with a value** answers, where the declaration says so: two attributes may
            // hold two answers to one question, and searching both asks "does either say so" where the
            // question was "does the one that applies say so". The span name counts as present always, since a
            // span has one - which is why it is only ever declared first where this flag is set.
            if self.text_first_present_source {
                if self.span_name_is_a_text_source {
                    return hit(ctx.span_name);
                }
                return self
                    .text_attribute_keys
                    .iter()
                    .find_map(|k| ctx.span_attrs.get(k))
                    .is_some_and(|v| hit(v));
            }
            if self.span_name_is_a_text_source && hit(ctx.span_name) {
                return true;
            }
            if self
                .text_attribute_keys
                .iter()
                .filter_map(|k| ctx.span_attrs.get(k))
                .any(|v| hit(v))
            {
                return true;
            }
        }
        false
    }
}

impl DetectPlan {
    /// The rule that claims this span.
    ///
    /// Takes the first by `legacy_rank`, which is what reproduces the table this replaced. Where more
    /// than one rule matches, that choice is a *migration bridge* and the alternatives are recoverable
    /// through [`Self::overlapping_candidates`] - the design's target is that no span has two, and the
    /// only way to get there is to be able to see which spans do.
    pub fn resolve(&self, ctx: &DetectContext<'_>) -> Option<&CompiledDetect> {
        let first = self.rules.iter().find(|rule| rule.matches(ctx))?;
        // `supersedes` **orders**, which is what `legacy_rank`'s own doc says the accepted design is: "a declared
        // `supersedes` resolves a known overlap". It used to waive only the overlap *report* while rank decided
        // the winner regardless - so the field documented an ordering it took no part in, and could be deleted
        // from an asset without changing a single attribution.
        //
        // The fast path is the common one: nothing supersedes this rule, so no later rule can displace it and the
        // scan stops where it always did. Only a rule some other rule claims to beat pays for the second look.
        if !self.superseded.contains(first.rule_id.as_str()) {
            return Some(first);
        }
        // The winner is the matching rule that **no** other matching rule beats, lowest rank among those. Not
        // "the first matching rule that beats the rank-winner": in rank order that test is satisfied by the
        // rank-winner itself, so the edge ordered nothing. Domination is transitive, since `supersedes` is a DAG
        // and a rule that beats a rule which beats this one beats it too; compilation refuses a cycle.
        let matching: Vec<&CompiledDetect> =
            self.rules.iter().filter(|rule| rule.matches(ctx)).collect();
        let beaten: std::collections::BTreeSet<&str> = matching
            .iter()
            .flat_map(|rule| self.dominated_by(rule.rule_id.as_str()))
            .collect();
        matching
            .iter()
            .find(|rule| !beaten.contains(rule.rule_id.as_str()))
            .copied()
            // Every candidate beaten by another is only possible in a cycle, which compilation refuses - so this
            // is unreachable, and falling back to the rank-winner is what it would have answered anyway.
            .or(Some(first))
    }

    /// Every rule that matches, in rank order.
    ///
    /// The instrument for retiring `legacy_rank`: a span with one candidate needs no ordering, and one
    /// with several names exactly which predicates are not yet sufficient. A rule that `supersedes`
    /// another is not reported against it, because that overlap is already owned.
    pub fn overlapping_candidates<'p>(
        &'p self,
        ctx: &DetectContext<'_>,
    ) -> Vec<&'p CompiledDetect> {
        let matching: Vec<&CompiledDetect> =
            self.rules.iter().filter(|rule| rule.matches(ctx)).collect();
        if matching.len() < 2 {
            return Vec::new();
        }
        // **Transitively** dominated, not only directly. `supersedes` is a DAG, so a rule that supersedes a
        // rule which supersedes a third owns that overlap too - reading direct edges only reported an overlap
        // whose ordering is in fact declared, which is noise in the one instrument meant to say where ordering
        // is *not* yet declared.
        let dominated = self.dominated_by(matching[0].rule_id.as_str());
        let contested: Vec<&CompiledDetect> = matching
            .iter()
            .skip(1)
            .filter(|other| !dominated.contains(other.rule_id.as_str()))
            .copied()
            .collect();
        if contested.is_empty() {
            return Vec::new();
        }
        matching
    }

    /// Every rule the named one supersedes, directly or through another.
    ///
    /// The transitive closure, because precedence is a DAG: a rule that supersedes one which supersedes a third
    /// has settled its ordering against all of them. Compilation refuses a cycle, so this terminates.
    fn dominated_by(&self, rule_id: &str) -> std::collections::BTreeSet<&str> {
        let mut out: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
        let mut queue: Vec<&str> = vec![rule_id];
        while let Some(current) = queue.pop() {
            let Some(rule) = self
                .rules
                .iter()
                .find(|candidate| candidate.rule_id == current)
            else {
                continue;
            };
            for target in &rule.supersedes {
                if out.insert(target.as_str()) {
                    queue.push(target.as_str());
                }
            }
        }
        out
    }

    /// The label a declaration resolves to, when it names exactly one framework this server knows.
    ///
    /// A list is accepted because the SDKs accept one (`framework=[Strands, Bedrock]`), and it resolves
    /// only when a single *framework* is named: a provider slug is claimed by no asset and so
    /// contributes nothing, which is how `[Strands, Bedrock]` still resolves to Strands while two
    /// genuine frameworks resolve to nothing. Two answers is not an answer, and picking one would label
    /// a span with no evidence for it.
    pub fn label_from_declaration(&self, declared: &str) -> Option<&str> {
        let mut labels: Vec<&str> = declared
            .split(',')
            .filter_map(|slug| self.sdk_slugs.get(slug.trim()))
            .map(String::as_str)
            .collect();
        labels.dedup();
        match labels.as_slice() {
            [one] => Some(one),
            _ => None,
        }
    }

    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }

    pub fn rules(&self) -> impl Iterator<Item = &CompiledDetect> {
        self.rules.iter()
    }

    pub fn slug_count(&self) -> usize {
        self.sdk_slugs.len()
    }
}
