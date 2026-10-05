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
        for (path, bytes) in sources {
            // A file is never skipped for being malformed: one quietly dropped for a typo is how a whole dialect's
            // rules once vanished with every test still green.
            let file = match decode(bytes) {
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
            digest: schema::digest_of(sources),
        })
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
        let ruleset = super::super::Ruleset::build(&parsed);
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
}
