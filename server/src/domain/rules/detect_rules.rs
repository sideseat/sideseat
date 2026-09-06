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
    pub rank: i32,
    pub match_spec: DetectMatch,
    /// `text_contains` sources split once, at compile time, into "the span name" and attribute keys.
    span_name_is_a_text_source: bool,
    text_attribute_keys: Vec<String>,
    /// Needles pre-lowercased, so a match does not lower-case a constant per span.
    text_needles_lowered: Vec<String>,
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
    DuplicateRuleId { rule: String },
    DuplicateRank { first: String, second: String },
    NoSignal { rule: String },
    DuplicateSlug { slug: String },
    BadTextSource { rule: String, source: String },
}

impl std::fmt::Display for DetectCompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
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
        || !spec.attr_exists.is_empty()
        || !spec.service_name.is_empty()
        || !spec.metadata_contains.is_empty()
        || !spec.resource_attr_contains.is_empty()
        || spec.text_contains.is_some()
}

/// Compile every asset's detection rules into one ordered plan.
pub fn compile(sources: &BTreeMap<String, Vec<u8>>) -> Result<DetectPlan, DetectCompileError> {
    let mut rules: Vec<CompiledDetect> = Vec::new();
    let mut seen_ids: HashMap<String, ()> = HashMap::new();
    let mut plan = DetectPlan::default();

    for bytes in sources.values() {
        // Parse failures are reported by the carrier compile, which reads the same assets; a second
        // error type for the same byte string would say the same thing twice.
        let Ok(file) = serde_json::from_slice::<RuleFile>(bytes) else {
            continue;
        };
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
            if !has_signal(&rule.match_spec) {
                return Err(DetectCompileError::NoSignal {
                    rule: rule.id.clone(),
                });
            }
            let (mut span_source, mut attr_keys, mut needles) = (false, Vec::new(), Vec::new());
            if let Some(TextContains {
                sources,
                needles: n,
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
                rank: rule.rank,
                match_spec: rule.match_spec.clone(),
                span_name_is_a_text_source: span_source,
                text_attribute_keys: attr_keys,
                text_needles_lowered: needles,
            });
        }
    }

    rules.sort_by_key(|r| r.rank);
    // A shared rank is refused: two rules that both match one span would be separated by load order,
    // and the whole reason rank is explicit is that this order is policy somebody has to own.
    for pair in rules.windows(2) {
        if pair[0].rank == pair[1].rank {
            return Err(DetectCompileError::DuplicateRank {
                first: pair[0].rule_id.clone(),
                second: pair[1].rule_id.clone(),
            });
        }
    }

    plan.rules = rules;
    Ok(plan)
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
        if !spec.metadata_contains.is_empty()
            && let Some(metadata) = ctx.span_attrs.get(super::METADATA_KEY)
            && spec
                .metadata_contains
                .iter()
                .any(|s| metadata.contains(s.as_str()))
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
    /// The rule that claims this span, in rank order.
    pub fn resolve(&self, ctx: &DetectContext<'_>) -> Option<&CompiledDetect> {
        self.rules.iter().find(|rule| rule.matches(ctx))
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
