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
    BadTextSource {
        rule: String,
        source: String,
    },
}

impl std::fmt::Display for DetectCompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Parse { path, message } => write!(f, "{path}: {message}"),
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
        for rule in &file.detect {
            if seen_ids.insert(rule.id.clone(), ()).is_some() {
                return Err(DetectCompileError::DuplicateRuleId {
                    rule: rule.id.clone(),
                });
            }
            if rule.id.is_empty() || rule.label.is_empty() {
                return Err(DetectCompileError::EmptyLiteral {
                    rule: rule.id.clone(),
                    dimension: "id/label",
                });
            }
            if !has_signal(&rule.match_spec) {
                return Err(DetectCompileError::NoSignal {
                    rule: rule.id.clone(),
                });
            }
            // Through the shared atom validator, not a copy of it: the two had drifted in both directions.
            let spec = &rule.match_spec;
            if let Some(defect) = atom_literal_defect(spec) {
                // The variant a caller's own diagnostic wants: detection has a dedicated error for an
                // unreadable phrase source, and collapsing every defect into one variant made that
                // diagnostic worse than it was.
                return Err(match defect {
                    AtomDefect::BadTextSource(source) => DetectCompileError::BadTextSource {
                        rule: rule.id.clone(),
                        source,
                    },
                    other => DetectCompileError::EmptyLiteral {
                        rule: rule.id.clone(),
                        dimension: other.dimension(),
                    },
                });
            }
            let (mut span_source, mut attr_keys, mut needles) = (false, Vec::new(), Vec::new());
            if let Some(TextContains {
                sources,
                needles: n,
                first_present_source: _,
            }) = &rule.match_spec.text_contains
            {
                for source in sources {
                    if source == "span_name" {
                        span_source = true;
                    } else if let Some(key) = source.strip_prefix("attr:") {
                        attr_keys.push(key.to_string());
                    } else {
                        return Err(DetectCompileError::BadTextSource {
                            rule: rule.id.clone(),
                            source: source.clone(),
                        });
                    }
                }
                needles = n.iter().map(|s| s.to_lowercase()).collect();
            }
            rules.push(CompiledDetect {
                rule_file: file.id.clone(),
                rule_id: rule.id.clone(),
                doc: rule.doc.clone(),
                label: rule.label.clone(),
                legacy_rank: rule.legacy_rank,
                supersedes: rule.supersedes.clone(),
                match_spec: rule.match_spec.clone(),
                span_name_is_a_text_source: span_source,
                text_attribute_keys: attr_keys,
                text_needles_lowered: needles,
                text_first_present_source: rule
                    .match_spec
                    .text_contains
                    .as_ref()
                    .is_some_and(|t| t.first_present_source),
            });
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
}

impl AtomDefect {
    /// The dimension a diagnostic should name.
    pub(super) fn dimension(&self) -> &'static str {
        match self {
            Self::EmptyLiteral(dimension) | Self::EmptySubstring(dimension) => dimension,
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
        }
    }
}

pub(super) fn atom_literal_defect(spec: &DetectMatch) -> Option<AtomDefect> {
    for (dimension, values) in [
        ("span_name", &spec.span_name),
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
            .span_name
            .iter()
            .any(|p| ctx.span_name == p || ctx.span_name.starts_with(p))
        {
            return true;
        }
        if !spec.service_name.is_empty()
            && let Some(service) = ctx.resource_attrs.get(super::SERVICE_NAME_KEY)
            && spec
                .service_name
                .iter()
                .any(|s| service == s || service.contains(s.as_str()))
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
        self.rules.iter().find(|rule| rule.matches(ctx))
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
        let winner = matching[0];
        let contested: Vec<&CompiledDetect> = matching
            .iter()
            .skip(1)
            .filter(|other| !winner.supersedes.contains(&other.rule_id))
            .copied()
            .collect();
        if contested.is_empty() {
            return Vec::new();
        }
        matching
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
