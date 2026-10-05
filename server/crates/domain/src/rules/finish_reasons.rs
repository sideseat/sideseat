//! Finish-reason spellings, compiled from the `finish_reasons` section of the engine's vocabulary asset.
//!
//! The categories are this engine's ([`FinishReason`]); every word a provider or instrumentation writes for
//! one is the asset's. Lookup canonicalises case and word boundaries on both sides, because `end_turn`,
//! `END_TURN` and Bedrock's camelCase `endTurn` differ only in how a language spells identifiers - a table that
//! listed each casing would be a table of which SDK serialised the enum.

use std::collections::HashMap;

use thiserror::Error;

use super::schema::RuleFile;
use crate::sideml::FinishReason;

/// Every declared spelling, by its folded form.
#[derive(Debug, Default)]
pub struct FinishReasonPlan {
    by_spelling: HashMap<String, FinishReason>,
}

/// Why a `finish_reasons` declaration could not take effect.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum FinishReasonCompileError {
    #[error("`{id}` declares a spelling with nothing left once case and separators are folded")]
    EmptySpelling { id: String },
    #[error(
        "`{spelling}` is declared by `{first}` and again by `{second}` (spellings are compared with case and \
         separators folded), so one of the declarations states nothing"
    )]
    Repeated {
        spelling: String,
        first: String,
        second: String,
    },
}

impl FinishReasonPlan {
    pub fn compile(files: &[RuleFile]) -> Result<Self, FinishReasonCompileError> {
        let mut by_spelling = HashMap::new();
        let mut declared_by: HashMap<String, String> = HashMap::new();
        for entry in files.iter().flat_map(|file| &file.finish_reasons) {
            for spelling in &entry.spellings {
                let folded = fold(spelling);
                if folded.is_empty() {
                    return Err(FinishReasonCompileError::EmptySpelling {
                        id: entry.id.clone(),
                    });
                }
                // Refused even when both declarations mean the same: a duplicate is either redundant or, after
                // a later edit, a conflict whose winner is load order.
                if let Some(first) = declared_by.insert(folded.clone(), entry.id.clone()) {
                    return Err(FinishReasonCompileError::Repeated {
                        spelling: spelling.clone(),
                        first,
                        second: entry.id.clone(),
                    });
                }
                by_spelling.insert(folded, entry.means);
            }
        }
        Ok(Self { by_spelling })
    }

    /// The category a producer's spelling means, or `None` for a spelling no asset declares.
    pub fn lookup(&self, spelling: &str) -> Option<FinishReason> {
        self.by_spelling.get(&fold(spelling)).copied()
    }
}

/// The spelling with case and word boundaries canonicalised: lower case, a camelCase boundary and every run of
/// `_`, `-` or whitespace become one `_`, and none leads or trails.
///
/// A boundary is kept rather than deleted, so `endTurn` is `end_turn` while a run-together `endturn` stays a
/// different word that nothing declares.
fn fold(spelling: &str) -> String {
    let mut out = String::with_capacity(spelling.len() + 4);
    let mut previous_lower = false;
    let mut pending_boundary = false;
    for c in spelling.chars() {
        if matches!(c, '_' | '-') || c.is_whitespace() {
            pending_boundary = true;
            previous_lower = false;
            continue;
        }
        if (pending_boundary || (c.is_uppercase() && previous_lower)) && !out.is_empty() {
            out.push('_');
        }
        pending_boundary = false;
        previous_lower = c.is_lowercase() || c.is_ascii_digit();
        out.extend(c.to_lowercase());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(entries: serde_json::Value) -> RuleFile {
        serde_json::from_value(serde_json::json!({"id": "t", "finish_reasons": entries}))
            .expect("the test asset parses")
    }

    #[test]
    fn spellings_match_across_case_and_separators() {
        let plan = FinishReasonPlan::compile(&[file(serde_json::json!([
            {"id": "f.stop", "means": "stop", "spellings": ["end_turn"]},
            {"id": "f.tool", "means": "tool_use", "spellings": ["tool_use"]}
        ]))])
        .expect("compiles");
        for spelling in [
            "end_turn",
            "END_TURN",
            "endTurn",
            "end-turn",
            "End Turn",
            "_end__turn_",
        ] {
            assert_eq!(
                plan.lookup(spelling),
                Some(FinishReason::Stop),
                "{spelling}"
            );
        }
        assert_eq!(plan.lookup("toolUse"), Some(FinishReason::ToolUse));
        // A boundary is information about where the words are; a run-together spelling is another word.
        assert_eq!(plan.lookup("endturn"), None);
        assert_eq!(plan.lookup(""), None);
    }

    #[test]
    fn a_spelling_declared_twice_is_refused_even_with_one_meaning() {
        let err = FinishReasonPlan::compile(&[file(serde_json::json!([
            {"id": "f.a", "means": "stop", "spellings": ["end_turn"]},
            {"id": "f.b", "means": "stop", "spellings": ["endTurn"]}
        ]))])
        .expect_err("refused");
        assert_eq!(
            err,
            FinishReasonCompileError::Repeated {
                spelling: "endTurn".into(),
                first: "f.a".into(),
                second: "f.b".into()
            }
        );
    }

    #[test]
    fn a_spelling_that_folds_to_nothing_is_refused() {
        let err = FinishReasonPlan::compile(&[file(serde_json::json!([
            {"id": "f.a", "means": "stop", "spellings": ["_-"]}
        ]))])
        .expect_err("refused");
        assert_eq!(
            err,
            FinishReasonCompileError::EmptySpelling { id: "f.a".into() }
        );
    }

    #[test]
    fn only_the_vocabulary_asset_may_say_what_a_spelling_means() {
        let producer = file(serde_json::json!([
            {"id": "f.a", "means": "stop", "spellings": ["done"]}
        ]));
        let defect = producer.declaration_defect().expect("refused");
        assert!(defect.contains("`finish_reasons`"), "{defect}");
        let mut vocabulary = producer;
        vocabulary.id = super::super::schema::FINISH_REASONS_ASSET.to_string();
        assert_eq!(vocabulary.declaration_defect(), None);
    }

    #[test]
    fn the_embedded_table_reads_every_corpus_spelling() {
        let plan = &crate::rules::ruleset().finish_reasons;
        let cases = [
            ("stop", FinishReason::Stop),
            ("end_turn", FinishReason::Stop),
            ("endTurn", FinishReason::Stop),
            ("STOP", FinishReason::Stop),
            ("max_tokens", FinishReason::Length),
            ("maxTokens", FinishReason::Length),
            ("tool_calls", FinishReason::ToolUse),
            ("tool-calls", FinishReason::ToolUse),
            ("toolUse", FinishReason::ToolUse),
            ("tool_call", FinishReason::ToolUse),
            ("content_filter", FinishReason::ContentFilter),
            ("failed", FinishReason::Error),
        ];
        for (spelling, means) in cases {
            assert_eq!(plan.lookup(spelling), Some(means), "{spelling}");
        }
        assert_eq!(plan.lookup("unknown"), None);
    }
}
