use super::*;

/// One release range a clause was observed firing in: positive evidence, never a gate.
///
/// Written on an executable leaf with a stable id - a message rule or reading - where the shape it reads is one
/// some releases write. Read by nothing that decides an answer: the parse
/// detaches every annotation before any section compiles, so a clause behaves alike with or without it. What
/// reads it is the coverage join over the captured matrix - each range must be exercised by a fixture inside it,
/// and a range no release inside the window falls in has aged out.
#[derive(Debug, Deserialize, Clone, PartialEq, Eq)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct Observation {
    #[serde(default)]
    pub doc: Option<String>,
    /// The package whose releases the range counts, as the matrix's provenance names it.
    pub package: String,
    /// How that package numbers its releases: declared here, because nothing else says which scheme a
    /// package's versions are in.
    pub scheme: crate::rules::versions::VersionScheme,
    /// The first release observed. Half-open: `since <= v < before`.
    #[serde(default)]
    pub since: Option<String>,
    /// The first release no longer observed.
    #[serde(default)]
    pub before: Option<String>,
    /// The configuration of the release that was observed (`OTEL_SEMCONV_STABILITY_OPT_IN`, an instrumentation
    /// setting), as the matrix names it. Absent: any.
    #[serde(default)]
    pub profile: Option<String>,
}

impl Observation {
    /// Why the range cannot be checked: a missing package, a bound that is not a version in its scheme, an
    /// empty range, or no bound at all.
    pub fn defect(&self) -> Option<String> {
        use crate::rules::versions::Version;
        if self.package.trim().is_empty() {
            return Some("names no package".to_string());
        }
        if self.profile.as_deref().is_some_and(|p| p.trim().is_empty()) {
            return Some("names an empty profile".to_string());
        }
        let parse = |text: &Option<String>| match text {
            None => Ok(None),
            Some(text) => Version::parse_bound(self.scheme, text)
                .map(Some)
                .ok_or_else(|| format!("`{text}` is not a version in the declared scheme")),
        };
        let since = match parse(&self.since) {
            Ok(since) => since,
            Err(why) => return Some(why),
        };
        let before = match parse(&self.before) {
            Ok(before) => before,
            Err(why) => return Some(why),
        };
        match (since, before) {
            (None, None) => Some(
                "states no bound: a range of every release is not evidence about any of them"
                    .to_string(),
            ),
            (Some(low), Some(high)) if low.compare(&high).is_none_or(std::cmp::Ordering::is_ge) => {
                Some("`since` is not below `before`, so no release is inside it".to_string())
            }
            _ => None,
        }
    }

    /// Whether `version`, in this range's scheme, is inside it. `None` where it is not a version in the scheme.
    pub fn contains(&self, version: &str) -> Option<bool> {
        use crate::rules::versions::Version;
        let version = Version::parse(self.scheme, version)?;
        let at_or_after = |bound: &Option<String>| {
            bound
                .as_ref()
                .and_then(|text| Version::parse_bound(self.scheme, text))
                .map(|bound| {
                    version
                        .compare(&bound)
                        .is_some_and(std::cmp::Ordering::is_ge)
                })
        };
        Some(
            at_or_after(&self.since).unwrap_or(true) && !at_or_after(&self.before).unwrap_or(false),
        )
    }
}

/// The annotations of one clause, detached from the files the sections compile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedLeaf {
    /// The asset that declares the clause.
    pub asset: String,
    /// The clause, as the coverage evidence names the one that fired: a message rule's id, or its id and its
    /// reading's (`rule/reading`).
    pub leaf: String,
    pub ranges: Vec<Observation>,
}
