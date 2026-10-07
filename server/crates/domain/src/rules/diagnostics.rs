//! One rendering for every ruleset compile defect.
//!
//! Each section compiler keeps its own typed error, because tests and callers match on the variant. What they
//! share is how a defect is *reported*: the section it was found in, the asset and clause it is about, and the
//! reason. [`RulesetDiagnostics`] is that report for the whole ruleset, so one run names every broken section
//! rather than only the first one compiled.
//!
//! Aggregation is across sections. A section still stops at its own first defect: its later checks often
//! depend on earlier ones having held (ranks are compared only between rules that compiled, shadowing only
//! between rules with valid conditions), so continuing inside a section would report consequences as if they
//! were causes.

use std::fmt;

use super::assets::ParsedAssets;
use super::schema::RuleFile;

/// The asset section a defect was found in, spelled as the section's key in an asset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RuleSection {
    Carriers,
    Detect,
    Messages,
    MessageProjections,
    MessageEvents,
    LogEvents,
    EventRoles,
    RoleAuthority,
    ContentBlocks,
    SpanFacts,
    SpanFields,
    ToolShapes,
    /// `observation_types` and `span_categories`, which compile as one ordered plan with one id space.
    Classification,
    MessageMembers,
    ProviderAliases,
    FinishReasons,
    SyntheticCallIds,
    EventCategories,
}

impl RuleSection {
    pub fn key(self) -> &'static str {
        match self {
            Self::Carriers => "carriers",
            Self::Detect => "detect",
            Self::Messages => "messages",
            Self::MessageProjections => "message_projections",
            Self::MessageEvents => "message_events",
            Self::LogEvents => "log_events",
            Self::EventRoles => "event_roles",
            Self::RoleAuthority => "role_authority",
            Self::ContentBlocks => "content_blocks",
            Self::SpanFacts => "span_facts",
            Self::SpanFields => "span_fields",
            Self::ToolShapes => "tool_shapes",
            Self::Classification => "observation_types/span_categories",
            Self::MessageMembers => "message_members",
            Self::ProviderAliases => "provider_aliases",
            Self::FinishReasons => "finish_reasons",
            Self::SyntheticCallIds => "synthetic_call_ids",
            Self::EventCategories => "event_categories",
        }
    }
}

/// Where a defect is: an asset, a clause in it, or both.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct RuleLocation {
    /// The asset's path; `None` where no asset declares the clause (an id the error synthesised).
    pub asset_path: Option<String>,
    pub asset_id: Option<String>,
    pub clause: Option<String>,
}

impl fmt::Display for RuleLocation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (&self.asset_path, &self.asset_id, &self.clause) {
            (Some(path), Some(id), Some(clause)) => write!(f, "{path} (`{id}`) clause `{clause}`"),
            (Some(path), Some(id), None) => write!(f, "{path} (`{id}`)"),
            (Some(path), None, Some(clause)) => write!(f, "{path} clause `{clause}`"),
            (Some(path), None, None) => write!(f, "{path}"),
            (None, _, Some(clause)) => write!(f, "clause `{clause}`"),
            (None, _, None) => write!(f, "an unlocated declaration"),
        }
    }
}

/// One compile defect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleDiagnostic {
    pub section: RuleSection,
    /// Every place the defect involves: one for most, two for a pair (a duplicate id, a shared rank, a
    /// shadowed rule). Empty where the error names nothing an asset declares.
    pub locations: Vec<RuleLocation>,
    pub reason: String,
}

impl fmt::Display for RuleDiagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}]", self.section.key())?;
        for (index, location) in self.locations.iter().enumerate() {
            write!(f, "{}{location}", if index == 0 { " " } else { " and " })?;
        }
        write!(f, ": {}", self.reason)
    }
}

/// Every defect found while compiling a ruleset, in section order.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub struct RulesetDiagnostics {
    pub diagnostics: Vec<RuleDiagnostic>,
}

impl fmt::Display for RulesetDiagnostics {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let count = self.diagnostics.len();
        write!(
            f,
            "{count} rule defect{}",
            if count == 1 { "" } else { "s" }
        )?;
        for diagnostic in &self.diagnostics {
            write!(f, "\n  {diagnostic}")?;
        }
        Ok(())
    }
}

/// What a section error says about where it is.
///
/// The clause ids a variant names, which the collector resolves to the assets declaring them. Resolution is by
/// lookup rather than carried in every variant because the compilers construct errors in about sixty places
/// and only the clause id is at hand in most of them; an id several assets declare resolves to all of them,
/// so an ambiguous location is reported as ambiguous rather than guessed.
pub(crate) trait SectionDefect: fmt::Display {
    /// The clause ids this defect is about, most specific first.
    fn clauses(&self) -> Vec<&str>;

    /// Asset paths the error names directly, where it knows them.
    fn asset_paths(&self) -> Vec<&str> {
        Vec::new()
    }
}

/// Collects section results, keeping every defect.
pub(crate) struct Collector<'a> {
    assets: &'a ParsedAssets,
    diagnostics: Vec<RuleDiagnostic>,
}

impl<'a> Collector<'a> {
    pub(crate) fn new(assets: &'a ParsedAssets) -> Self {
        Self {
            assets,
            diagnostics: Vec::new(),
        }
    }

    /// The section's value, or `None` with its defect recorded.
    pub(crate) fn take<T, E: SectionDefect>(
        &mut self,
        section: RuleSection,
        result: Result<T, E>,
    ) -> Option<T> {
        match result {
            Ok(value) => Some(value),
            Err(error) => {
                self.record(section, &error);
                None
            }
        }
    }

    fn record(&mut self, section: RuleSection, error: &dyn SectionDefect) {
        let mut locations: Vec<RuleLocation> = Vec::new();
        for clause in error.clauses() {
            let declaring: Vec<(&str, &RuleFile)> = self
                .assets
                .iter()
                .filter(|(_, file)| declares(file, clause))
                .collect();
            if declaring.is_empty() {
                locations.push(RuleLocation {
                    asset_path: None,
                    asset_id: None,
                    clause: Some(clause.to_owned()),
                });
            }
            for (path, file) in declaring {
                locations.push(RuleLocation {
                    asset_path: Some(path.to_owned()),
                    asset_id: Some(file.id.clone()),
                    clause: Some(clause.to_owned()),
                });
            }
        }
        for named in error.asset_paths() {
            if locations
                .iter()
                .any(|location| location.asset_path.as_deref() == Some(named))
            {
                continue;
            }
            let id = self
                .assets
                .iter()
                .find(|(path, _)| *path == named)
                .map(|(_, file)| file.id.clone());
            locations.push(RuleLocation {
                asset_path: Some(named.to_owned()),
                asset_id: id,
                clause: None,
            });
        }
        locations.dedup();
        self.diagnostics.push(RuleDiagnostic {
            section,
            locations,
            reason: error.to_string(),
        });
    }

    /// Every defect recorded, in section order.
    pub(crate) fn into_diagnostics(mut self) -> RulesetDiagnostics {
        // Section order, then location: the compile order is an implementation detail and the report should not
        // reshuffle when it changes.
        self.diagnostics
            .sort_by(|a, b| (a.section, &a.locations).cmp(&(b.section, &b.locations)));
        RulesetDiagnostics {
            diagnostics: self.diagnostics,
        }
    }
}

/// Whether this file declares a clause with this id, in any section.
fn declares(file: &RuleFile, clause: &str) -> bool {
    let mut top_level = file
        .carriers
        .iter()
        .map(|rule| rule.id.as_str())
        .chain(file.detect.iter().map(|rule| rule.id.as_str()))
        .chain(file.content_blocks.iter().map(|rule| rule.id.as_str()))
        .chain(file.tool_shapes.iter().map(|rule| rule.id.as_str()))
        .chain(file.message_members.iter().map(|rule| rule.id.as_str()))
        .chain(file.observation_types.iter().map(|rule| rule.id.as_str()))
        .chain(file.span_categories.iter().map(|rule| rule.id.as_str()));
    top_level.any(|id| id == clause)
        || file
            .clause_ids()
            .iter()
            .any(|(owner, ids)| owner == clause || ids.iter().any(|id| id == clause))
}

impl SectionDefect for String {
    fn clauses(&self) -> Vec<&str> {
        Vec::new()
    }
}

impl SectionDefect for super::carrier_rules::CompileError {
    fn clauses(&self) -> Vec<&str> {
        use super::carrier_rules::CompileError as E;
        match self {
            E::IncoherentFacts { clause, .. }
            | E::UnknownObservationType { clause, .. }
            | E::UnknownPreset { clause, .. }
            | E::NoPrimaryKey { clause }
            | E::DuplicateClauseId { clause }
            | E::EmptyLiteral { clause } => vec![clause],
            E::Ambiguous { first, second, .. } => vec![first, second],
        }
    }
}

impl SectionDefect for super::detect_rules::DetectCompileError {
    fn clauses(&self) -> Vec<&str> {
        use super::detect_rules::DetectCompileError as E;
        match self {
            E::EmptyLiteral { rule, .. }
            | E::DuplicateRuleId { rule }
            | E::Condition { rule, .. } => vec![rule],
            E::SubsumedLiteral { rule, .. } => vec![rule],
            E::UselessSupersedes { rule, .. } => vec![rule],
            E::ShadowedRule { earlier, later } => vec![later, earlier],
            E::DuplicatePriority { first, second } => vec![first, second],
            E::SlugLabelNoRuleProduces { .. } | E::DuplicateSlug { .. } => Vec::new(),
        }
    }
}

impl SectionDefect for super::message_rules::MessageCompileError {
    fn clauses(&self) -> Vec<&str> {
        use super::message_rules::MessageCompileError as E;
        match self {
            E::NotExactlyOneCarrier { rule }
            | E::DuplicateRuleId { rule }
            | E::EmptyCarrier { rule }
            | E::Inexpressible { rule, .. }
            | E::Condition { rule, .. } => vec![rule],
            E::UnknownFragment { .. } => Vec::new(),
            E::StarvedReading { starved, taker, .. } => vec![starved, taker],
            E::ContestedCarrier { first, second, .. } => vec![first, second],
        }
    }
}

impl SectionDefect for super::classify::ClassifyCompileError {
    fn clauses(&self) -> Vec<&str> {
        use super::classify::ClassifyCompileError as E;
        match self {
            E::DeadCondition { rule, .. }
            | E::NoResult { rule, .. }
            | E::UnknownResult { rule, .. }
            | E::DuplicateId { rule, .. } => vec![rule],
            E::ShadowedRule { earlier, later } => vec![later, earlier],
            E::SharedPriority { first, second, .. } => vec![first, second],
        }
    }

    fn asset_paths(&self) -> Vec<&str> {
        use super::classify::ClassifyCompileError as E;
        match self {
            E::DeadCondition { file, .. }
            | E::NoResult { file, .. }
            | E::UnknownResult { file, .. } => vec![file],
            E::DuplicateId { first, second, .. } => vec![first, second],
            E::ShadowedRule { .. } | E::SharedPriority { .. } => Vec::new(),
        }
    }
}

impl SectionDefect for super::span_fields::FieldCompileError {
    fn clauses(&self) -> Vec<&str> {
        use super::span_fields::FieldCompileError as E;
        match self {
            E::NoSources { rule, .. }
            | E::SourceReadsNothing { rule, .. }
            | E::SourceReadsTwoThings { rule, .. }
            | E::UngatedLiteral { rule, .. }
            | E::JsonNamesNoMember { rule, .. }
            | E::ReductionThatCannotYield { rule, .. }
            | E::ReductionWithoutAPath { rule, .. }
            | E::FoldWithoutText { rule, .. }
            | E::ScalarOnlyWithoutAPath { rule, .. }
            | E::EmptyAttribute { rule, .. }
            | E::MergeIntoScalar { rule, .. }
            | E::DeadGate { rule, .. }
            | E::DuplicateId { rule, .. } => vec![rule],
            E::DuplicateTarget { first, second, .. } => vec![first, second],
        }
    }

    fn asset_paths(&self) -> Vec<&str> {
        use super::span_fields::FieldCompileError as E;
        match self {
            E::NoSources { file, .. }
            | E::SourceReadsNothing { file, .. }
            | E::SourceReadsTwoThings { file, .. }
            | E::UngatedLiteral { file, .. }
            | E::JsonNamesNoMember { file, .. }
            | E::ReductionThatCannotYield { file, .. }
            | E::ReductionWithoutAPath { file, .. }
            | E::FoldWithoutText { file, .. }
            | E::ScalarOnlyWithoutAPath { file, .. }
            | E::EmptyAttribute { file, .. }
            | E::MergeIntoScalar { file, .. }
            | E::DeadGate { file, .. } => vec![file],
            E::DuplicateId { first, second, .. } => vec![first, second],
            E::DuplicateTarget { .. } => Vec::new(),
        }
    }
}

impl SectionDefect for super::members::MemberCompileError {
    fn clauses(&self) -> Vec<&str> {
        use super::members::MemberCompileError as E;
        match self {
            E::EmptyMember { rule, .. }
            | E::NoMembers { rule, .. }
            | E::ContentWithoutShape { rule, .. }
            | E::SaysNothing { rule, .. }
            | E::QuestionWithoutAPriority { rule, .. }
            | E::PriorityWithoutAQuestion { rule, .. }
            | E::TwoOrderedQuestions { rule, .. }
            | E::UnknownAliasTarget { rule, .. } => vec![rule],
            E::SharedPriority { first, second, .. } | E::DuplicateMember { first, second, .. } => {
                vec![first, second]
            }
        }
    }

    fn asset_paths(&self) -> Vec<&str> {
        use super::members::MemberCompileError as E;
        match self {
            E::EmptyMember { file, .. }
            | E::NoMembers { file, .. }
            | E::ContentWithoutShape { file, .. }
            | E::SaysNothing { file, .. }
            | E::QuestionWithoutAPriority { file, .. }
            | E::PriorityWithoutAQuestion { file, .. }
            | E::TwoOrderedQuestions { file, .. }
            | E::UnknownAliasTarget { file, .. } => vec![file],
            E::SharedPriority { .. } | E::DuplicateMember { .. } => Vec::new(),
        }
    }
}

impl SectionDefect for super::finish_reasons::FinishReasonCompileError {
    fn clauses(&self) -> Vec<&str> {
        use super::finish_reasons::FinishReasonCompileError as E;
        match self {
            E::EmptySpelling { id } => vec![id],
            E::Repeated { first, second, .. } => vec![first, second],
        }
    }
}

impl SectionDefect for super::tool_shapes::ToolShapeError {
    fn clauses(&self) -> Vec<&str> {
        use super::tool_shapes::ToolShapeError as E;
        match self {
            E::TwoAnswers { id }
            | E::NoName { id }
            | E::EmptyCarry { id }
            | E::NoParameterPath { id }
            | E::Inexpressible { id, .. } => vec![id],
            E::SharedPriority { first, second, .. } => vec![first, second],
        }
    }
}

impl SectionDefect for super::content_blocks::ContentBlockCompileError {
    fn clauses(&self) -> Vec<&str> {
        use super::content_blocks::ContentBlockCompileError as E;
        match self {
            E::TargetForms { rule, .. }
            | E::Predicate { rule, .. }
            | E::NoCondition { rule }
            | E::SelfSelectingContent { rule }
            | E::UnwrapsNothing { rule }
            | E::UnwrapsWholeBlock { rule }
            | E::EmptyRequiredSelector { rule, .. }
            | E::SpliceOutsideMessageContent { rule, .. }
            | E::IdTemplate { rule, .. }
            | E::Source { rule, .. } => vec![rule],
            E::SharedPriority { first, second, .. } => vec![first, second],
        }
    }
}

impl SectionDefect for super::SpanFactCompileError {
    fn clauses(&self) -> Vec<&str> {
        use super::SpanFactCompileError as E;
        match self {
            E::Condition { rule, .. } => vec![rule],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The embedded corpus plus probe files, so every section but the probed ones compiles.
    fn embedded_with(probes: &[(&str, serde_json::Value)]) -> ParsedAssets {
        let mut sources = super::super::schema::embedded_sources();
        for (path, body) in probes {
            sources.insert(
                (*path).to_string(),
                serde_json::to_vec(body).expect("serialises"),
            );
        }
        ParsedAssets::parse(&sources).expect("the probes parse")
    }

    #[test]
    fn defects_in_two_sections_are_both_reported() {
        let assets = embedded_with(&[
            (
                "probe/blocks.json",
                serde_json::json!({"id": "probe-blocks", "content_blocks": [
                    {"id": "probe.nothing", "at": "after_provider_formats", "priority": 1}
                ]}),
            ),
            (
                "probe/fields.json",
                serde_json::json!({"id": "probe-fields", "span_fields": [
                    {"id": "probe.field", "target": "user_id", "sources": []}
                ]}),
            ),
        ]);
        let report = super::super::Ruleset::build(&assets)
            .err()
            .expect("two defective sections must be refused");
        let sections: Vec<RuleSection> = report.diagnostics.iter().map(|d| d.section).collect();
        assert_eq!(
            sections,
            [RuleSection::ContentBlocks, RuleSection::SpanFields]
        );
        assert_eq!(
            report.to_string(),
            "2 rule defects\n  \
             [content_blocks] probe/blocks.json (`probe-blocks`) clause `probe.nothing`: content-block rule \
             `probe.nothing` declares 0 target forms; exactly one is required\n  \
             [span_fields] probe/fields.json (`probe-fields`) clause `probe.field`: span field rule \
             `probe.field` in `probe/fields.json` declares no source"
        );
    }

    #[test]
    fn a_pair_defect_names_both_clauses_and_their_assets() {
        let assets = embedded_with(&[
            (
                "probe/one.json",
                serde_json::json!({
                    "id": "probe-one",
                    "content_blocks": [
                        {
                            "id": "probe.a",
                            "at": "after_provider_formats",
                            "priority": 1,
                            "where": {
                                "path": "$.type",
                                "one_of": [
                                    "probe_a"
                                ]
                            },
                            "text": {
                                "text": [
                                    "$.value"
                                ]
                            }
                        }
                    ]
                }),
            ),
            (
                "probe/two.json",
                serde_json::json!({
                    "id": "probe-two",
                    "content_blocks": [
                        {
                            "id": "probe.b",
                            "at": "after_provider_formats",
                            "priority": 1,
                            "where": {
                                "path": "$.type",
                                "one_of": [
                                    "probe_b"
                                ]
                            },
                            "text": {
                                "text": [
                                    "$.value"
                                ]
                            }
                        }
                    ]
                }),
            ),
        ]);
        let report = super::super::Ruleset::build(&assets)
            .err()
            .expect("a shared rank must be refused");
        assert_eq!(report.diagnostics.len(), 1, "{report}");
        let paths: Vec<Option<&str>> = report.diagnostics[0]
            .locations
            .iter()
            .map(|location| location.asset_path.as_deref())
            .collect();
        assert_eq!(
            paths,
            [Some("probe/one.json"), Some("probe/two.json")],
            "{report}"
        );
    }

    #[test]
    fn a_clause_no_asset_declares_is_reported_without_a_guessed_asset() {
        let location = RuleLocation {
            asset_path: None,
            asset_id: None,
            clause: Some("synthetic".to_string()),
        };
        assert_eq!(location.to_string(), "clause `synthetic`");
    }
}
