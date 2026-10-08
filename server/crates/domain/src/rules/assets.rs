//! The rule assets, parsed once.
//!
//! Every section compiler reads the same [`ParsedAssets`] rather than the asset bytes. Bytes are accepted in one
//! place only, so a section cannot parse a file differently from its neighbours, skip one, or pay for another
//! parse: there is no bytes-taking compiler left to call.

use std::collections::BTreeMap;

use super::schema::{self, RuleFile};

/// One asset that could not be accepted: malformed JSON, a schema mismatch, or a declaration defect.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{path}: {message}")]
pub struct AssetDefect {
    /// The asset's path, which locates the file; its declared id does not.
    pub path: String,
    pub message: String,
}

/// Every asset defect found, in path order.
///
/// All of them rather than the first: a corpus with two broken files reports both in one run.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{}", render(.defects))]
pub struct AssetParseError {
    pub defects: Vec<AssetDefect>,
}

fn render(defects: &[AssetDefect]) -> String {
    defects
        .iter()
        .map(AssetDefect::to_string)
        .collect::<Vec<_>>()
        .join("; ")
}

/// The asset corpus, each file parsed and checked for declaration defects exactly once.
///
/// Fields are private so the only way to obtain one is [`ParsedAssets::parse`], which is what makes "every
/// section saw a validated file" a property of the type rather than of each caller.
#[derive(Debug)]
pub struct ParsedAssets {
    /// Asset paths, parallel to `files`, in the sources' (deterministic) path order.
    paths: Vec<String>,
    files: Vec<RuleFile>,
    /// Every `observed_in` annotation, detached from `files` before any section can see it: release ranges are
    /// evidence for the coverage join, and a section that cannot read one cannot be decided by one.
    observed: Vec<schema::ObservedLeaf>,
    /// BLAKE3 of the original paths and bytes. Computed from the bytes, never from a re-serialised `RuleFile`, so
    /// a formatting-only edit still changes the reconstruction cache key exactly as before.
    digest: String,
}

impl ParsedAssets {
    /// Parse every asset, refusing malformed files, declaration defects and two assets sharing an id.
    pub fn parse(sources: &BTreeMap<String, Vec<u8>>) -> Result<Self, AssetParseError> {
        Self::parse_with(sources, |bytes| {
            serde_json::from_slice(bytes).map_err(|error| error.to_string())
        })
    }

    /// [`ParsedAssets::parse`] with the decoder supplied, so a test can count decodes without a global counter
    /// that parallel tests would race on.
    fn parse_with(
        sources: &BTreeMap<String, Vec<u8>>,
        mut decode: impl FnMut(&[u8]) -> Result<RuleFile, String>,
    ) -> Result<Self, AssetParseError> {
        let mut defects = Vec::new();
        let mut paths = Vec::with_capacity(sources.len());
        let mut files = Vec::with_capacity(sources.len());
        let mut observed = Vec::new();
        for (path, bytes) in sources {
            // A file is never skipped for being malformed: one quietly dropped for a typo is how a whole dialect's
            // rules once vanished with every test still green.
            let mut file = match decode(bytes) {
                Ok(file) => file,
                Err(message) => {
                    defects.push(AssetDefect {
                        path: path.clone(),
                        message,
                    });
                    continue;
                }
            };
            // Declaration defects are properties of one file that several section compilers would each have to
            // remember to ask; asked here, no section can see a file that has one.
            if let Some(message) = file.declaration_defect() {
                defects.push(AssetDefect {
                    path: path.clone(),
                    message,
                });
                continue;
            }
            match detach_observations(&mut file) {
                Ok(leaves) => observed.extend(leaves),
                Err(message) => {
                    defects.push(AssetDefect {
                        path: path.clone(),
                        message,
                    });
                    continue;
                }
            }
            paths.push(path.clone());
            files.push(file);
        }
        // An asset id is the provenance every diagnostic reports, and `declaration_defect` is per file - so two
        // files declaring one id would make their clauses indistinguishable precisely where a reader looks.
        let mut first_path: BTreeMap<&str, &str> = BTreeMap::new();
        for (path, file) in paths.iter().zip(&files) {
            if let Some(first) = first_path.insert(file.id.as_str(), path.as_str()) {
                defects.push(AssetDefect {
                    path: path.clone(),
                    message: format!(
                        "declares the id `{}`, which `{first}` already declares, so their clauses share a \
                         provenance path and nothing can tell them apart",
                        file.id
                    ),
                });
            }
        }
        if !defects.is_empty() {
            return Err(AssetParseError { defects });
        }
        Ok(Self {
            paths,
            files,
            observed,
            digest: schema::digest_of(sources),
        })
    }

    /// Every clause's `observed_in` ranges, detached from the files the sections compile.
    pub fn observed(&self) -> &[schema::ObservedLeaf] {
        &self.observed
    }

    /// Every parsed file, in path order.
    pub fn files(&self) -> &[RuleFile] {
        &self.files
    }

    /// Every parsed file with its path, in path order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &RuleFile)> {
        self.paths.iter().map(String::as_str).zip(&self.files)
    }

    /// BLAKE3 of the asset paths and bytes, hex-encoded.
    pub fn digest(&self) -> &str {
        &self.digest
    }
}

/// Take every `observed_in` annotation out of `file`, validated, so that no section compiles one.
///
/// Offered on message rules and their readings, whose firing the coverage join attributes through their
/// emissions' evidence, and refused on a fragment's cases and a selection point's extra cases, which several rules
/// share, so a range there would be evidence about no one clause.
/// What `detach_observations` does with one clause's ranges: validate them and keep them aside.
type Take<'a> = dyn FnMut(String, &mut Vec<schema::Observation>) -> Result<(), String> + 'a;

fn detach_observations(file: &mut RuleFile) -> Result<Vec<schema::ObservedLeaf>, String> {
    let mut out: Vec<(String, Vec<schema::Observation>)> = Vec::new();
    let mut take = |leaf: String, ranges: &mut Vec<schema::Observation>| -> Result<(), String> {
        if ranges.is_empty() {
            return Ok(());
        }
        for range in ranges.iter() {
            if let Some(defect) = range.defect() {
                return Err(format!(
                    "clause `{leaf}` has an `observed_in` range that {defect}"
                ));
            }
        }
        out.push((leaf, std::mem::take(ranges)));
        Ok(())
    };
    fn message_rule(rule: &mut schema::MessageRule, take: &mut Take<'_>) -> Result<(), String> {
        take(rule.id.clone(), &mut rule.observed_in)?;
        for reading in rule
            .alternatives
            .iter_mut()
            .chain(&mut rule.also)
            .chain(&mut rule.fallback)
        {
            take(
                format!("{}/{}", rule.id, reading.id),
                &mut reading.observed_in,
            )?;
            if reading
                .extra_cases
                .iter()
                .any(|case| !case.observed_in.is_empty())
            {
                return Err(format!(
                    "an extra case of `{}/{}` states `observed_in`; ranges belong to the selection point",
                    rule.id, reading.id
                ));
            }
        }
        if let Some(set) = &mut rule.branch_set {
            for sub in set
                .primary
                .iter_mut()
                .chain(&mut set.fallback_if_primary_empty)
                .chain(&mut set.always)
            {
                message_rule(sub, take)?;
            }
        }
        Ok(())
    }
    for rule in &mut file.messages {
        message_rule(rule, &mut take)?;
    }
    for (name, fragment) in &file.fragments {
        if fragment
            .cases
            .iter()
            .any(|case| !case.observed_in.is_empty())
        {
            return Err(format!(
                "a case of fragment `{name}` states `observed_in`; every rule using the fragment shares it, so the \
                 range belongs to each selection point instead"
            ));
        }
    }
    Ok(out
        .into_iter()
        .map(|(leaf, ranges)| schema::ObservedLeaf {
            asset: file.id.clone(),
            leaf,
            ranges,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sources(entries: &[(&str, &str)]) -> BTreeMap<String, Vec<u8>> {
        entries
            .iter()
            .map(|(path, body)| (path.to_string(), body.as_bytes().to_vec()))
            .collect()
    }

    #[test]
    fn every_asset_is_decoded_exactly_once() {
        let embedded = schema::embedded_sources();
        let mut decodes = 0usize;
        let parsed = ParsedAssets::parse_with(&embedded, |bytes| {
            decodes += 1;
            serde_json::from_slice(bytes).map_err(|error| error.to_string())
        })
        .expect("the embedded assets parse");
        assert_eq!(decodes, embedded.len());
        assert_eq!(parsed.files().len(), embedded.len());
        // The whole ruleset compiles from that one parse: `Ruleset::build` accepts no bytes.
        let ruleset = super::super::Ruleset::build(&parsed).expect("the embedded ruleset compiles");
        assert_eq!(ruleset.digest, schema::digest_of(&embedded));
    }

    #[test]
    fn every_defect_is_reported_not_only_the_first() {
        let error = ParsedAssets::parse(&sources(&[
            ("a.json", "{"),
            ("b.json", r#"{"id":"b","unknown_section":[]}"#),
            ("c.json", r#"{"id":"same"}"#),
            ("d.json", r#"{"id":"same"}"#),
        ]))
        .expect_err("three defects");
        let paths: Vec<&str> = error.defects.iter().map(|d| d.path.as_str()).collect();
        assert_eq!(paths, ["a.json", "b.json", "d.json"]);
        assert!(error.defects[2].message.contains("`c.json`"), "{error}");
    }

    #[test]
    fn paths_and_digest_survive_parsing() {
        let input = sources(&[
            ("x/one.json", r#"{"id":"one"}"#),
            ("two.json", r#"{"id":"two"}"#),
        ]);
        let parsed = ParsedAssets::parse(&input).expect("parses");
        let seen: Vec<(&str, &str)> = parsed.iter().map(|(p, f)| (p, f.id.as_str())).collect();
        assert_eq!(seen, [("two.json", "two"), ("x/one.json", "one")]);
        assert_eq!(parsed.digest(), schema::digest_of(&input));
    }

    /// **`observed_in` decides nothing.** A valid range is written onto every clause of every kind that may carry
    /// one, across the whole embedded corpus; the parse must hand the sections exactly the files it hands them
    /// without the ranges - compared whole, so no compiler can be reached by one - and keep every range aside.
    #[test]
    fn an_observed_range_reaches_no_compiler() {
        let range = serde_json::json!([{"package": "probe", "scheme": "pep440", "since": "1.0", "before": "2.0"}]);
        let mut injected = 0usize;
        fn message(rule: &mut serde_json::Value, range: &serde_json::Value, injected: &mut usize) {
            rule["observed_in"] = range.clone();
            *injected += 1;
            for list in ["alternatives", "also", "fallback"] {
                for reading in rule
                    .get_mut(list)
                    .and_then(|v| v.as_array_mut())
                    .into_iter()
                    .flatten()
                {
                    reading["observed_in"] = range.clone();
                    *injected += 1;
                }
            }
            if let Some(set) = rule.get_mut("branch_set") {
                for list in ["primary", "fallback_if_primary_empty", "always"] {
                    for sub in set
                        .get_mut(list)
                        .and_then(|v| v.as_array_mut())
                        .into_iter()
                        .flatten()
                    {
                        message(sub, range, injected);
                    }
                }
            }
        }
        let embedded = schema::embedded_sources();
        let annotated: BTreeMap<String, Vec<u8>> = embedded
            .iter()
            .map(|(path, bytes)| {
                let mut asset: serde_json::Value =
                    serde_json::from_slice(bytes).expect("the asset parses");
                for rule in asset
                    .get_mut("messages")
                    .and_then(|v| v.as_array_mut())
                    .into_iter()
                    .flatten()
                {
                    message(rule, &range, &mut injected);
                }
                (
                    path.clone(),
                    serde_json::to_vec(&asset).expect("serialises"),
                )
            })
            .collect();
        let plain = ParsedAssets::parse(&embedded).expect("the embedded assets parse");
        let marked = ParsedAssets::parse(&annotated).expect("the annotated assets parse");
        assert!(injected > 200, "{injected} clauses were annotated");
        assert_eq!(
            marked.observed().len(),
            injected,
            "every range is kept aside"
        );
        assert_eq!(
            format!("{:?}", marked.files()),
            format!("{:?}", plain.files()),
            "a section would see an annotation"
        );
        super::super::Ruleset::build(&marked).expect("the annotated corpus compiles");
    }

    #[test]
    fn an_observed_range_that_cannot_be_checked_is_refused() {
        let asset = |observed: &str, where_: &str| {
            sources(&[(
                "producers/p.json",
                &format!(
                    r#"{{"id":"p","messages":[{{"id":"p.m","read":{{"attribute":"p.k"}},"parse":"json","emit":"message","priority":9100{where_},"observed_in":{observed}}}]}}"#
                ),
            )])
        };
        assert!(
            ParsedAssets::parse(&asset(
                r#"[{"package":"p","scheme":"semver","since":"1"}]"#,
                ""
            ))
            .is_ok()
        );
        for (observed, why) in [
            (r#"[{"package":"p","scheme":"pep440"}]"#, "no bound"),
            (
                r#"[{"package":"","scheme":"pep440","since":"1"}]"#,
                "no package",
            ),
            (
                r#"[{"package":"p","scheme":"pep440","since":"2","before":"1"}]"#,
                "a reversed range",
            ),
            (
                r#"[{"package":"p","scheme":"semver","since":"one"}]"#,
                "a bound that is not a version",
            ),
            (
                r#"[{"package":"p","scheme":"pep440","since":"1","profile":" "}]"#,
                "an empty profile",
            ),
        ] {
            assert!(ParsedAssets::parse(&asset(observed, "")).is_err(), "{why}");
        }
        let fragment = sources(&[(
            "producers/f.json",
            r#"{"id":"f","fragments":{"shapes":{"cases":[{"id":"c","observed_in":[{"package":"p","scheme":"pep440","since":"1"}]}]}}}"#,
        )]);
        assert!(
            ParsedAssets::parse(&fragment).is_err(),
            "a fragment's case is shared by every rule using it"
        );
    }
}
