//! `value_dependent_claiming_instances`: `MessagePlan` held to `server/specs/ValueDependentClaiming.tla`.
//!
//! The model and this test read one manifest, `server/specs/instances/ValueDependentClaiming.json`. Each abstract
//! instance - reads, claims and composes over the carriers `a`, `b`, `c`, with a stage-fallback read - is
//! realised as a real asset and compiled once; then every span the manifest's bounds allow (each carrier absent,
//! unparseable or readable, each rule's gate held or not) is run through `MessagePlan::run` and
//! `MessagePlan::fallback`, and who owns each carrier, and which rules' emissions were kept, must equal the
//! model's greedy definition, restated below. Refused instances must be refused by the compiler, for the reason
//! the model gives. The hand-written `expect` answers are checked against the engine and the definition alike.
//!
//! The section of the specification between its `GENERATED` markers is rendered from the manifest here, and the
//! test fails while the committed one differs (`UPDATE_SPECS=1` rewrites it), so the model checks exactly the
//! instances this test does.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use serde::Deserialize;

use super::assets::ParsedAssets;
use super::message_rules::{MessageCompileError, MessageContext, OwnedCarrier, compile};

#[derive(Debug, Deserialize)]
struct Manifest {
    #[allow(dead_code)]
    doc: String,
    version: u32,
    carriers: Vec<String>,
    instances: Vec<Instance>,
}

#[derive(Debug, Deserialize)]
struct Instance {
    id: String,
    #[allow(dead_code)]
    doc: String,
    rules: Vec<Rule>,
    fallback: Vec<String>,
    refused: bool,
    expect: Vec<Expectation>,
}

#[derive(Debug, Deserialize)]
struct Rule {
    id: String,
    priority: i32,
    kind: Kind,
    candidates: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Kind {
    Read,
    Claim,
    Compose,
}

#[derive(Debug, Deserialize)]
struct Expectation {
    #[allow(dead_code)]
    doc: String,
    values: BTreeMap<String, Value>,
    gates: BTreeSet<String>,
    owners: BTreeMap<String, String>,
    kept: Vec<String>,
    fallback_kept: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Value {
    Absent,
    Bad,
    Ok,
}

const VALUES: [Value; 3] = [Value::Absent, Value::Bad, Value::Ok];
const FALLBACK: &str = "fallback";

fn repository() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn manifest() -> Manifest {
    let path = repository().join("server/specs/instances/ValueDependentClaiming.json");
    let manifest: Manifest = serde_json::from_slice(
        &std::fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display())),
    )
    .unwrap_or_else(|e| panic!("decode {}: {e}", path.display()));
    assert_eq!(
        manifest.version, 1,
        "a new manifest version needs this reader updated"
    );
    manifest
}

// ----------------------------------------------------------------------------
// The model's definitions, restated.

/// One span: each carrier's value, and the rules whose gates hold.
struct Span<'m> {
    values: &'m BTreeMap<String, Value>,
    gates: &'m BTreeSet<String>,
}

impl Span<'_> {
    fn value(&self, carrier: &str) -> Value {
        self.values.get(carrier).copied().unwrap_or(Value::Absent)
    }
}

/// What a rule's reading would own if it is kept, or `None` where it yields nothing.
///
/// A read commits to its first *present* candidate and yields only if that one parses; a compose yields the
/// members that parse, and nothing if none does. A gate that fails yields nothing.
fn reading(rule: &Rule, span: &Span<'_>) -> Option<BTreeSet<String>> {
    if !span.gates.contains(&rule.id) {
        return None;
    }
    match rule.kind {
        Kind::Read | Kind::Claim => {
            let first = rule
                .candidates
                .iter()
                .find(|c| span.value(c) != Value::Absent)?;
            (span.value(first) == Value::Ok).then(|| BTreeSet::from([first.clone()]))
        }
        Kind::Compose => {
            let parsed: BTreeSet<String> = rule
                .candidates
                .iter()
                .filter(|c| span.value(c) == Value::Ok)
                .cloned()
                .collect();
            (!parsed.is_empty()).then_some(parsed)
        }
    }
}

/// The rank-ordered greedy outcome: each reading, in priority order, is kept only if none of what it would own
/// is already owned, and is then the owner of all of it. The fallback read comes last, against what the dialect
/// rules own.
fn greedy(instance: &Instance, span: &Span<'_>) -> (BTreeMap<String, String>, Vec<String>, bool) {
    let mut rules: Vec<&Rule> = instance.rules.iter().collect();
    rules.sort_by_key(|rule| rule.priority);
    let mut owners = BTreeMap::new();
    let mut kept = Vec::new();
    for rule in rules {
        let Some(owns) = reading(rule, span) else {
            continue;
        };
        if owns.iter().any(|c| owners.contains_key(c)) {
            continue;
        }
        for carrier in owns {
            owners.insert(carrier, rule.id.clone());
        }
        kept.push(rule.id.clone());
    }
    let fallback = instance
        .fallback
        .iter()
        .find(|c| span.value(c) != Value::Absent)
        .filter(|c| span.value(c) == Value::Ok && !owners.contains_key(*c))
        .cloned();
    let fallback_kept = fallback.is_some();
    if let Some(carrier) = fallback {
        owners.insert(carrier, FALLBACK.to_string());
    }
    (owners, kept, fallback_kept)
}

/// The compile-time refusal: a rule ranked ahead of a compose reads one of its members, so the compose would be
/// dropped whole wherever that rule took it.
fn starves(instance: &Instance) -> bool {
    instance.rules.iter().any(|later| {
        later.kind == Kind::Compose
            && later.candidates.len() > 1
            && instance.rules.iter().any(|earlier| {
                earlier.priority < later.priority
                    && earlier
                        .candidates
                        .iter()
                        .any(|c| later.candidates.contains(c))
            })
    })
}

// ----------------------------------------------------------------------------
// The instance, as a real asset.

fn key(carrier: &str) -> String {
    format!("vdc.{carrier}")
}

fn first_of(candidates: &[String]) -> serde_json::Value {
    match candidates {
        [one] => serde_json::json!(key(one)),
        many => serde_json::json!({"first_of": many.iter().map(|c| key(c)).collect::<Vec<_>>()}),
    }
}

fn asset(instance: &Instance) -> Vec<u8> {
    let gate =
        |rule: &str| serde_json::json!({"source": format!("attr:vdc.gate.{rule}"), "exists": true});
    let mut messages: Vec<serde_json::Value> = instance
        .rules
        .iter()
        .map(|rule| {
            let id = format!("{}.{}", instance.id, rule.id);
            // A tag of its own, outside the carrier namespace: two readers that can own different spellings must
            // not report one carrier name, which the compiler refuses.
            let tag = format!("vdc.tag.{}", rule.id);
            match rule.kind {
                Kind::Read => serde_json::json!({
                    "id": id, "doc": "d", "read": {"attribute": first_of(&rule.candidates)}, "parse": "json",
                    "where": gate(&rule.id), "emit": "message", "priority": rule.priority, "tag_as": tag,
                    "wrap": {"role": "user"}
                }),
                Kind::Claim => serde_json::json!({
                    "id": id, "doc": "d", "read": {"attribute": first_of(&rule.candidates)}, "parse": "json",
                    "where": gate(&rule.id), "emit": "claim", "priority": rule.priority, "tag_as": tag
                }),
                Kind::Compose => serde_json::json!({
                    "id": id, "doc": "d",
                    "compose": {
                        "tag": tag,
                        "members": rule.candidates.iter().map(|c| serde_json::json!({
                            "as": format!("m_{c}"), "from": key(c), "parse": "json"
                        })).collect::<Vec<_>>(),
                        "trailing": {"role": "user"}
                    },
                    "where": gate(&rule.id), "emit": "message", "priority": rule.priority
                }),
            }
        })
        .collect();
    if !instance.fallback.is_empty() {
        messages.push(serde_json::json!({
            "id": format!("{}.{FALLBACK}", instance.id), "doc": "d",
            "source": {"span": {"stage": "fallback"}},
            "read": {"attribute": first_of(&instance.fallback)}, "parse": "json", "emit": "message",
            "priority": 900, "tag_as": "vdc.tag.fallback", "wrap": {"role": "user"}
        }));
    }
    serde_json::to_vec(&serde_json::json!({"id": instance.id, "doc": "d", "messages": messages}))
        .expect("the asset serialises")
}

fn attrs(span: &Span<'_>, carriers: &[String], rules: &[Rule]) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for carrier in carriers {
        match span.value(carrier) {
            Value::Absent => {}
            Value::Bad => {
                out.insert(key(carrier), "{".to_string());
            }
            Value::Ok => {
                out.insert(key(carrier), "\"x\"".to_string());
            }
        }
    }
    for rule in rules.iter().filter(|rule| span.gates.contains(&rule.id)) {
        out.insert(format!("vdc.gate.{}", rule.id), "1".to_string());
    }
    out
}

/// What the engine answered: owners, the dialect rules whose emissions were kept, and whether the fallback was.
fn engine(
    plan: &super::message_rules::MessagePlan,
    instance: &Instance,
    attrs: &HashMap<String, String>,
) -> (BTreeMap<String, String>, Vec<String>, bool) {
    let ctx = MessageContext::for_span("span", attrs, false);
    let local = |rule_id: &str| {
        rule_id
            .strip_prefix(&format!("{}.", instance.id))
            .expect("an instance rule")
            .to_string()
    };
    let mut owners = BTreeMap::new();
    let mut kept: Vec<String> = Vec::new();
    let mut read: HashSet<OwnedCarrier> = HashSet::new();
    for emission in plan.run(&ctx) {
        let rule = local(emission.rule_id);
        for owned in &emission.owns {
            let carrier = owned
                .name
                .strip_prefix("vdc.")
                .expect("a carrier")
                .to_string();
            let previous = owners.insert(carrier.clone(), rule.clone());
            assert!(
                previous.is_none() || previous.as_deref() == Some(rule.as_str()),
                "{}: `{carrier}` kept by two rules",
                instance.id
            );
            read.insert(owned.clone());
        }
        if kept.last() != Some(&rule) {
            kept.push(rule);
        }
    }
    let mut fallback_kept = false;
    for emission in plan.fallback(&ctx, &read) {
        fallback_kept = true;
        for owned in &emission.owns {
            let carrier = owned
                .name
                .strip_prefix("vdc.")
                .expect("a carrier")
                .to_string();
            assert!(
                owners
                    .insert(carrier.clone(), FALLBACK.to_string())
                    .is_none(),
                "{}: the fallback kept `{carrier}`, which a dialect rule owns",
                instance.id
            );
        }
    }
    (owners, kept, fallback_kept)
}

#[test]
fn value_dependent_claiming_instances() {
    let manifest = manifest();
    let mut spans_checked = 0usize;
    for instance in &manifest.instances {
        let assets = ParsedAssets::parse(&BTreeMap::from([(
            format!("{}.json", instance.id),
            asset(instance),
        )]))
        .unwrap_or_else(|error| panic!("{}: the instance does not parse: {error}", instance.id));
        let compiled = compile(&assets);
        assert_eq!(
            starves(instance),
            instance.refused,
            "{}: the manifest and the model disagree on whether the instance is refused",
            instance.id
        );
        if instance.refused {
            assert!(
                matches!(compiled, Err(MessageCompileError::StarvedReading { .. })),
                "{}: the compiler must refuse a starved compose, and answered {compiled:?}",
                instance.id
            );
            continue;
        }
        let plan =
            compiled.unwrap_or_else(|error| panic!("{}: does not compile: {error}", instance.id));
        for expectation in &instance.expect {
            let span = Span {
                values: &expectation.values,
                gates: &expectation.gates,
            };
            let expected = (
                expectation.owners.clone(),
                expectation.kept.clone(),
                expectation.fallback_kept,
            );
            assert_eq!(
                greedy(instance, &span),
                expected,
                "{}: the definition disagrees with the manifest",
                instance.id
            );
            let attrs = attrs(&span, &manifest.carriers, &instance.rules);
            assert_eq!(
                engine(&plan, instance, &attrs),
                expected,
                "{}: the engine disagrees with the manifest",
                instance.id
            );
        }
        // Every span the bounds allow.
        let carriers = &manifest.carriers;
        let rule_ids: Vec<&String> = instance.rules.iter().map(|rule| &rule.id).collect();
        for assignment in 0..VALUES.len().pow(carriers.len() as u32) {
            let mut values = BTreeMap::new();
            let mut rest = assignment;
            for carrier in carriers {
                values.insert(carrier.clone(), VALUES[rest % VALUES.len()]);
                rest /= VALUES.len();
            }
            for held in 0..(1usize << rule_ids.len()) {
                let gates: BTreeSet<String> = rule_ids
                    .iter()
                    .enumerate()
                    .filter(|(index, _)| held & (1 << index) != 0)
                    .map(|(_, id)| (*id).clone())
                    .collect();
                let span = Span {
                    values: &values,
                    gates: &gates,
                };
                let attrs = attrs(&span, carriers, &instance.rules);
                assert_eq!(
                    engine(&plan, instance, &attrs),
                    greedy(instance, &span),
                    "{}: values {values:?}, gates {gates:?}",
                    instance.id
                );
                spans_checked += 1;
            }
        }
    }
    assert!(
        spans_checked > 300,
        "only {spans_checked} spans were checked"
    );

    let path = repository().join("server/specs/ValueDependentClaiming.tla");
    let spec = std::fs::read_to_string(&path).expect("the specification exists");
    let begin = spec
        .find("\\* BEGIN GENERATED FROM")
        .expect("the specification marks its generated section");
    let end_marker = "\\* END GENERATED FROM instances/ValueDependentClaiming.json\n";
    let end = spec
        .find(end_marker)
        .expect("the generated section is closed")
        + end_marker.len();
    let rendered = generated_section(&manifest);
    if spec[begin..end] != rendered {
        if std::env::var_os("UPDATE_SPECS").is_some() {
            let updated = format!("{}{rendered}{}", &spec[..begin], &spec[end..]);
            std::fs::write(&path, updated).expect("the specification is writable");
        } else {
            panic!("the specification's generated section is stale; rerun with UPDATE_SPECS=1");
        }
    }
}

// ----------------------------------------------------------------------------
// The generated section of the specification.

fn tla_string(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

fn tla_seq<'a>(items: impl IntoIterator<Item = &'a String>) -> String {
    let items: Vec<String> = items.into_iter().map(|item| tla_string(item)).collect();
    if items.is_empty() {
        "<<>>".to_string()
    } else {
        format!("<<{}>>", items.join(", "))
    }
}

fn tla_set<'a>(items: impl IntoIterator<Item = &'a String>) -> String {
    let items: Vec<String> = items.into_iter().map(|item| tla_string(item)).collect();
    format!("{{{}}}", items.join(", "))
}

fn tla_function(pairs: Vec<(String, String)>) -> String {
    if pairs.is_empty() {
        return "<<>>".to_string();
    }
    let parts: Vec<String> = pairs
        .into_iter()
        .map(|(k, v)| format!("{k} :> {v}"))
        .collect();
    format!("({})", parts.join(" @@ "))
}

fn generated_section(manifest: &Manifest) -> String {
    let value = |value: Value| match value {
        Value::Absent => "\"absent\"",
        Value::Bad => "\"bad\"",
        Value::Ok => "\"ok\"",
    };
    let kind = |kind: Kind| match kind {
        Kind::Read => "\"read\"",
        Kind::Claim => "\"claim\"",
        Kind::Compose => "\"compose\"",
    };
    let mut out = String::new();
    out.push_str("\\* BEGIN GENERATED FROM instances/ValueDependentClaiming.json\n");
    out.push_str(&format!(
        "ManifestCarriers == {}\n",
        tla_set(&manifest.carriers)
    ));
    out.push_str("ManifestInstances == <<\n");
    let records: Vec<String> = manifest
        .instances
        .iter()
        .map(|instance| {
            let rules: Vec<String> = instance
                .rules
                .iter()
                .map(|rule| {
                    format!(
                        "[id |-> {}, prio |-> {}, kind |-> {}, cands |-> {}]",
                        tla_string(&rule.id),
                        rule.priority,
                        kind(rule.kind),
                        tla_seq(&rule.candidates)
                    )
                })
                .collect();
            let expect: Vec<String> = instance
                .expect
                .iter()
                .map(|e| {
                    let values = tla_function(
                        manifest
                            .carriers
                            .iter()
                            .map(|c| {
                                (tla_string(c), value(e.values.get(c).copied().unwrap_or(Value::Absent)).to_string())
                            })
                            .collect(),
                    );
                    let owners = tla_function(e.owners.iter().map(|(c, r)| (tla_string(c), tla_string(r))).collect());
                    format!(
                        "[values |-> {values}, gates |-> {}, owners |-> {owners}, kept |-> {}, fallback |-> {}]",
                        tla_set(&e.gates),
                        tla_seq(&e.kept),
                        if e.fallback_kept { "TRUE" } else { "FALSE" }
                    )
                })
                .collect();
            let rules = if rules.is_empty() { "<<>>".to_string() } else { format!("<< {} >>", rules.join(", ")) };
            let expect = if expect.is_empty() { "<<>>".to_string() } else { format!("<< {} >>", expect.join(", ")) };
            format!(
                "    [id |-> {}, rules |-> {rules}, fallback |-> {}, refused |-> {}, expect |-> {expect}]",
                tla_string(&instance.id),
                tla_seq(&instance.fallback),
                if instance.refused { "TRUE" } else { "FALSE" }
            )
        })
        .collect();
    out.push_str(&records.join(",\n"));
    out.push_str("\n>>\n");
    out.push_str("\\* END GENERATED FROM instances/ValueDependentClaiming.json\n");
    out
}
