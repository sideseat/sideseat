//! The marks a span carries to the read, decided when it is ingested.
//!
//! A mark is one declared fact about a span, stored as a bit beside its extracted columns because the read path
//! holds a span's messages and not its attributes (`schema::SpanMarkRule`). This module assigns the bits, decides
//! them for a span, and answers which bit a read-time condition means by name.

use std::collections::{BTreeMap, HashMap};

use super::diagnostics::ClauseDefect;
use super::schema::RuleFile;
use super::span_conditions::{self, Readable, SpanExpr, SpanSubject};

/// How many marks the stored width holds.
///
/// Sixteen, as two bytes: one byte would make the eighth mark a storage change rather than an asset change, and a
/// wider word would cost every span bytes for bits nothing sets. The bound exists so the cost per span is a
/// constant a reader can see, and declaring more is refused at startup.
pub const MARK_LIMIT: usize = 16;

/// The declared marks, compiled once with the rest of the ruleset.
#[derive(Debug, Default)]
pub struct SpanMarkPlan {
    /// In bit order, which is the order of the ids.
    marks: Vec<CompiledMark>,
}

#[derive(Debug)]
struct CompiledMark {
    id: String,
    condition: SpanExpr,
}

impl SpanMarkPlan {
    pub fn compile(files: &[RuleFile]) -> Result<Self, ClauseDefect> {
        // Sorted by id rather than by the order the assets happen to load, so a mark's bit is the same in every
        // build of the same corpus. The stored bits are a cache a re-parse rebuilds, so a reassignment is not a
        // stored-data question - but a bit that moved between two runs of one build would make a diagnostic
        // unreadable.
        let mut declared: BTreeMap<String, (&str, &super::schema::SpanMarkRule)> = BTreeMap::new();
        for file in files {
            for rule in &file.span_marks {
                let defect =
                    |reason: String| ClauseDefect::new(&[&rule.id], Some(&file.id), reason);
                if rule.id.is_empty() {
                    return Err(defect(
                        "a span mark has an empty id, and a read-time condition names a mark by its id"
                            .to_string(),
                    ));
                }
                if rule.because.trim().is_empty() {
                    return Err(defect(format!(
                        "span mark `{}` states no reason. A mark is storage on every span, so why the fact \
                         cannot be read where it is needed is required beside it",
                        rule.id
                    )));
                }
                if let Some((other, _)) = declared.insert(rule.id.clone(), (&file.id, rule)) {
                    return Err(defect(format!(
                        "span mark `{}` is declared by both `{other}` and `{}`, and the marks are one shared \
                         set because they share a stored word",
                        rule.id, file.id
                    )));
                }
            }
        }
        if declared.len() > MARK_LIMIT {
            let first = declared.keys().next().cloned().unwrap_or_default();
            return Err(ClauseDefect::new(
                &[&first],
                None,
                format!(
                    "{} span marks are declared and the stored width holds {MARK_LIMIT}. Widening it costs \
                     every span, so a new mark replaces one that is no longer read rather than adding to them",
                    declared.len()
                ),
            ));
        }
        let mut marks = Vec::with_capacity(declared.len());
        for (id, (file, rule)) in declared {
            // The mark's own condition reads the span, never another mark: the marks are decided in one pass, so
            // one reading another would depend on which bit came first.
            let condition = super::detect_rules::checked_condition(&rule.condition, Readable::ALL)
                .map_err(|refusal| {
                    ClauseDefect::new(&[&id], Some(file), format!("span mark `{id}`: {refusal}"))
                })?;
            marks.push(CompiledMark { id, condition });
        }
        Ok(Self { marks })
    }

    /// The bit a read-time condition means by this name, or `None` where no asset declares it.
    pub fn bit_of(&self, id: &str) -> Option<u8> {
        self.marks
            .iter()
            .position(|mark| mark.id == id)
            .map(|at| at as u8)
    }

    /// Every declared mark's name, in bit order.
    pub fn ids(&self) -> impl Iterator<Item = &str> {
        self.marks.iter().map(|mark| mark.id.as_str())
    }

    /// The marks this span carries, as the stored word.
    ///
    /// Only a condition that is *true* sets its bit: a span whose attributes leave the question unanswered is
    /// not marked, which is the same answer a condition over the span itself would give.
    pub fn marks_of(
        &self,
        span_name: &str,
        attrs: &HashMap<String, String>,
        scope_name: Option<&str>,
        scope_version: Option<&str>,
    ) -> u16 {
        if self.marks.is_empty() {
            return 0;
        }
        let subject = SpanSubject {
            span_name,
            attrs,
            scope_name,
            scope_version,
            resource: None,
            marks: 0,
        };
        let mut word = 0u16;
        for (at, mark) in self.marks.iter().enumerate() {
            if span_conditions::holds(&mark.condition, &subject) {
                word |= 1 << at;
            }
        }
        word
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe(marks: serde_json::Value) -> RuleFile {
        serde_json::from_value(serde_json::json!({"id": "probe", "span_marks": marks}))
            .expect("the probe asset parses")
    }

    fn plan(marks: serde_json::Value) -> Result<SpanMarkPlan, ClauseDefect> {
        SpanMarkPlan::compile(&[probe(marks)])
    }

    fn attrs(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn mark(id: &str, condition: serde_json::Value) -> serde_json::Value {
        serde_json::json!({"id": id, "because": "a probe", "where": condition})
    }

    /// **A mark is the answer to its condition, as one bit, and the bits are the order of the ids.** A span the
    /// condition does not hold for is unmarked, whether the answer was false or unknown.
    #[test]
    fn a_mark_is_set_for_the_spans_its_condition_holds_for() {
        let plan = plan(serde_json::json!([
            mark(
                "probe.streamed",
                serde_json::json!({"source": "attr:probe_options", "parses": "json",
                    "member": {"path": "$.stream", "equals": true}})
            ),
            mark(
                "probe.aggregate",
                serde_json::json!({"source": "attr:aggregate", "equals": "true"})
            ),
        ]))
        .expect("the probe compiles");
        assert_eq!(
            plan.bit_of("probe.aggregate"),
            Some(0),
            "ids order the bits"
        );
        assert_eq!(plan.bit_of("probe.streamed"), Some(1));
        assert_eq!(plan.bit_of("probe.unknown"), None);
        let word = |pairs: &[(&str, &str)]| plan.marks_of("span", &attrs(pairs), None, None);
        assert_eq!(word(&[("probe_options", r#"{"stream": true}"#)]), 0b10);
        assert_eq!(word(&[("aggregate", "true")]), 0b01);
        assert_eq!(
            word(&[
                ("probe_options", r#"{"stream": true}"#),
                ("aggregate", "true")
            ]),
            0b11
        );
        assert_eq!(
            word(&[("probe_options", r#"{"stream": false}"#)]),
            0,
            "a false answer leaves the bit clear"
        );
        assert_eq!(
            word(&[]),
            0,
            "and so does an unknown one: an unmarked span is one the condition did not hold for"
        );
    }

    /// What compilation refuses: an unnamed mark, one with no reason, one name twice, a condition reading a
    /// mark, and more marks than the stored width holds.
    #[test]
    fn a_mark_that_cannot_be_stored_or_read_is_refused() {
        let named = |id: &str| mark(id, serde_json::json!({"source": "attr:a", "exists": true}));
        // A mark with no reason at all is refused by the schema itself, before any of this: `because` is a
        // required member, so an asset that omits it does not parse.
        assert!(
            serde_json::from_value::<RuleFile>(serde_json::json!({
                "id": "probe",
                "span_marks": [{"id": "probe.a", "where": {"source": "attr:a", "exists": true}}]
            }))
            .is_err(),
            "a mark with no reason is not an asset"
        );
        for (why, marks) in [
            (
                "no id",
                serde_json::json!([mark(
                    "",
                    serde_json::json!({"source": "attr:a", "exists": true})
                )]),
            ),
            (
                "a blank reason",
                serde_json::json!([{"id": "probe.a", "because": "  ",
                    "where": {"source": "attr:a", "exists": true}}]),
            ),
            (
                "one id twice",
                serde_json::json!([named("probe.a"), named("probe.a")]),
            ),
            (
                "a condition that reads a mark",
                serde_json::json!([mark(
                    "probe.a",
                    serde_json::json!({"source": "mark:probe.b", "exists": true})
                )]),
            ),
            (
                "more marks than the width holds",
                serde_json::Value::Array(
                    (0..=MARK_LIMIT)
                        .map(|at| named(&format!("probe.m{at:02}")))
                        .collect(),
                ),
            ),
        ] {
            assert!(plan(marks).is_err(), "{why}: accepted");
        }
        // Exactly the width is accepted: the bound is a limit, not a margin.
        let full = serde_json::Value::Array(
            (0..MARK_LIMIT)
                .map(|at| named(&format!("probe.m{at:02}")))
                .collect(),
        );
        let plan = plan(full).expect("the full width compiles");
        assert_eq!(plan.ids().count(), MARK_LIMIT);
        assert_eq!(plan.bit_of("probe.m15"), Some(15), "the last bit is used");
    }

    /// Two assets may not declare one mark, because the marks share a stored word.
    #[test]
    fn two_assets_may_not_declare_one_mark() {
        let rule = serde_json::json!([{
            "id": "shared.mark", "because": "a probe",
            "where": {"source": "attr:a", "exists": true}
        }]);
        let other: RuleFile =
            serde_json::from_value(serde_json::json!({"id": "other", "span_marks": rule.clone()}))
                .expect("the probe asset parses");
        assert!(SpanMarkPlan::compile(&[probe(rule), other]).is_err());
    }
}
