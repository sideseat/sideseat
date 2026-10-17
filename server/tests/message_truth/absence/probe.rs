//! A probe for deciding whether a gap could be declared, run by hand.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use super::haystack::Haystack;
use super::{prove, truth};

/// Prints the proof for facts of one truth over each of its fixtures, for deciding whether a gap could
/// be declared: `ABSENCE_PROBE=spring-ai/tool_use:fact-007,fact-008 cargo test --locked -p
/// sideseat-server --test message_goldens -- --ignored --nocapture absence_probe` (no fact list probes
/// every fact).
#[test]
#[ignore]
fn absence_probe() {
    let Ok(probe) = std::env::var("ABSENCE_PROBE") else {
        eprintln!("set ABSENCE_PROBE=<producer>/<scenario>[:fact-001,fact-002]");
        return;
    };
    let (key, ids) = probe.split_once(':').unwrap_or((probe.as_str(), ""));
    let truths = truth::load_all();
    let truth = truths.get(key).unwrap_or_else(|| panic!("no truth {key}"));
    let fixtures: BTreeMap<String, Vec<PathBuf>> = crate::discover_fixtures().into_iter().collect();
    let wanted: BTreeSet<&str> = ids.split(',').filter(|s| !s.is_empty()).collect();
    for fixture in &truth.fixtures {
        let Some(paths) = fixtures.get(fixture) else {
            continue;
        };
        let haystack = Haystack::of_fixture(paths);
        for fact in truth
            .facts
            .iter()
            .filter(|f| wanted.is_empty() || wanted.contains(f.id.as_str()))
        {
            eprintln!(
                "{fixture} {} {}: {:?}",
                fact.id,
                fact.kind,
                prove(fact, &haystack)
            );
        }
    }
}
