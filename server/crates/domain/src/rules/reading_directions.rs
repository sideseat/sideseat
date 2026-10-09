//! What a reading's declared side may say, checked against the carriers it reads.
//!
//! A reading states its side (`Alternative::direction`) only where it differs from the carrier's: one payload
//! that holds a request's turns beside the answer. Two declarations would say something else and are refused,
//! because each is the carrier's fact written in the wrong place - a reading restating its carrier's side, and a
//! rule whose readings all state the same side. So is a fragment case contradicting the selection point that
//! applied it, which would leave the side to whichever the engine consulted first.

use super::carrier_rules::{CarrierContext, CarrierPlan};
use super::diagnostics::ClauseDefect;
use super::message_rules::{CompiledMessageRule, MessagePlan};
use super::schema::{Alternative, ReadingDirection};

/// Every rule's declared sides, against the carriers it reports under.
pub(super) fn check(messages: &MessagePlan, carriers: &CarrierPlan) -> Result<(), ClauseDefect> {
    for rule in messages.rules() {
        check_rule(rule, carriers)?;
    }
    Ok(())
}

fn check_rule(rule: &CompiledMessageRule, carriers: &CarrierPlan) -> Result<(), ClauseDefect> {
    let defect = |reading: &str, reason: String| {
        ClauseDefect::new(&[&rule.rule_id, reading], Some(&rule.rule_file), reason)
    };
    let readings: Vec<(&Alternative, &[Alternative])> = rule
        .alternatives
        .iter()
        .chain(&rule.also)
        .chain(&rule.fallback)
        .map(|reading| (&reading.spec, reading.fragment_cases.as_slice()))
        .collect();
    let declared: Vec<(&str, ReadingDirection)> = readings
        .iter()
        .flat_map(|(spec, cases)| {
            std::iter::once(*spec)
                .chain(cases.iter())
                .filter_map(|reading| reading.direction.map(|side| (reading.id.as_str(), side)))
        })
        .collect();
    if declared.is_empty() {
        return Ok(());
    }

    // A case that disagrees with the selection point applying it: the side would be whichever the engine
    // consults first, which is no statement at all.
    for (spec, cases) in &readings {
        if let Some(outer) = spec.direction
            && let Some(case) = cases
                .iter()
                .find(|case| case.direction.is_some_and(|inner| inner != outer))
        {
            return Err(defect(
                &case.id,
                format!(
                    "fragment case `{}` declares a side its selection point `{}` contradicts, so which one \
                     an observation is on would depend on which the engine asked first",
                    case.id, spec.id
                ),
            ));
        }
    }

    // Every top-level reading on one side: that is what the carrier holds, and saying it per reading hides a
    // carrier fact where no carrier change can find it.
    let top: Vec<Option<ReadingDirection>> =
        readings.iter().map(|(spec, _)| spec.direction).collect();
    if top.len() > 1 && top.iter().all(|side| side.is_some() && *side == top[0]) {
        return Err(defect(
            &readings[0].0.id,
            "every reading of this rule declares the same side, which is a fact about its carrier - declare \
             it on the carrier and leave the readings to say where they differ"
                .to_string(),
        ));
    }

    // A reading restating its carrier's side. Judged by the carrier's generic clause, the one that holds with
    // no span context: a clause qualified by span name or observation type cannot be resolved here, so this
    // refuses only what the carrier says everywhere.
    let carrier_sides: Vec<Option<ReadingDirection>> = reported_carriers(rule)
        .into_iter()
        .map(|(event, attribute)| {
            carriers
                .resolve(&CarrierContext::carrier_only(event, attribute))
                .map(|clause| clause.semantics)
                .and_then(|semantics| {
                    match (
                        semantics.carrier_holds_span_input,
                        semantics.carrier_holds_span_output,
                    ) {
                        (true, false) => Some(ReadingDirection::Input),
                        (false, true) => Some(ReadingDirection::Output),
                        _ => None,
                    }
                })
        })
        .collect();
    for (reading, side) in &declared {
        if !carrier_sides.is_empty() && carrier_sides.iter().all(|carrier| *carrier == Some(*side))
        {
            return Err(defect(
                reading,
                format!(
                    "reading `{reading}` declares the side its carrier already holds, which says nothing - and \
                     would keep saying it after the carrier changed. A reading's side is for where it differs"
                ),
            ));
        }
    }
    Ok(())
}

/// The carriers a rule's observations are reported under: its tag where it declares one, else each key or
/// event it reads.
fn reported_carriers(rule: &CompiledMessageRule) -> Vec<(Option<&str>, Option<&str>)> {
    if let Some(tag) = rule.tag_as.as_deref() {
        return vec![(None, Some(tag))];
    }
    let events = rule.source.event_names();
    if !events.is_empty() {
        return events
            .iter()
            .map(|name| (Some(name.as_str()), None))
            .collect();
    }
    rule.read
        .keys
        .iter()
        .flat_map(|keys| keys.iter())
        .map(|key| (None, Some(key.as_str())))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use crate::rules::Ruleset;
    use crate::rules::assets::ParsedAssets;

    /// A probe corpus: one carrier holding a span's output, and one message rule reading it with the given
    /// readings - and the fragment they may apply.
    fn compiles(readings: serde_json::Value) -> Result<(), String> {
        let asset = serde_json::json!({
            "id": "probe",
            "carriers": [{
                "id": "probe.out.carrier",
                "doc": "What the probe span produced.",
                "match": {"attribute": "probe.out"},
                "facts": {"preset": "emission", "carrier_holds_span_output": true}
            }],
            "fragments": {
                "turn": {
                    "doc": "A probe turn.",
                    "cases": [{"id": "as_input", "where": {"path": "$.role", "exists": true},
                               "direction": "input"}]
                }
            },
            "messages": [{
                "id": "probe.rule",
                "read": {"attribute": "probe.out"},
                "parse": "json",
                "emit": "message",
                "priority": 1,
                "also": readings
            }]
        });
        let sources = BTreeMap::from([(
            "producers/probe.json".to_string(),
            serde_json::to_vec(&asset).map_err(|e| e.to_string())?,
        )]);
        let assets = ParsedAssets::parse(&sources).map_err(|e| e.to_string())?;
        Ruleset::build(&assets)
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    /// **A reading declares its side where it differs from its carrier's, and only there.**
    #[test]
    fn a_reading_declares_its_side_only_where_it_differs_from_its_carrier() {
        compiles(serde_json::json!([
            {"id": "turns", "select": "$.messages[*]", "direction": "input"},
            {"id": "answer", "select": "$.raw", "wrap": {"role": "assistant"}}
        ]))
        .expect("turns on the input side of an output carrier, beside the answer");

        let refused = |readings: serde_json::Value, why: &str, reason: &str| {
            let error = compiles(readings).expect_err(why);
            assert!(
                error.contains("probe.rule") && error.contains(reason),
                "{why}: refused for another reason: {error}"
            );
        };
        refused(
            serde_json::json!([
                {"id": "turns", "select": "$.messages[*]", "direction": "input"},
                {"id": "answer", "select": "$.raw", "direction": "output"}
            ]),
            "a reading restating its carrier's side",
            "already holds",
        );
        refused(
            serde_json::json!([
                {"id": "turns", "select": "$.messages[*]", "direction": "input"},
                {"id": "more", "select": "$.history[*]", "direction": "input"}
            ]),
            "every reading on one side, which is the carrier's fact",
            "the same side",
        );
        refused(
            serde_json::json!([
                {"id": "turns", "select": "$.messages[*]", "then_fragment": "probe.turn",
                 "direction": "output"},
                {"id": "answer", "select": "$.raw", "wrap": {"role": "assistant"}}
            ]),
            "a fragment case contradicting its selection point",
            "contradicts",
        );
    }
}
