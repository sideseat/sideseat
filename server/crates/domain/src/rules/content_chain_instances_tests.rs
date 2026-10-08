//! `content_block_chain_instances`: `ContentBlockPlan` held to `server/specs/ContentBlockChain.tla`.
//!
//! The model and this test read one manifest, `server/specs/instances/ContentBlockChain.json`. Each instance's
//! cases are compiled into a real plan, and every block is realised as JSON. The model's answer at each position -
//! the first recognising case in priority order that builds, unless a recognising unwrap whose member cannot be
//! normalised stopped the position first - must be what `ContentBlockPlan::normalize` answers there; and the
//! positions taken in the engine's order (`before_provider_formats`, `message_envelope` for a message's own block
//! only, `provider_formats`, `after_provider_formats`) must give the manifest's hand-written answers.
//!
//! **The boundary.** The chain is the production one, `normalize_block_in`, over the plan under test: its order,
//! its envelope exclusion for a returned value, and an unwrap's recursion into its member through the same plan
//! are the engine's, not restated here. The canonical passthrough and the media and unknown fallbacks are the
//! chain's own steps, which the model reproduces from the manifest's `canonical` and `fallback` flags - so the
//! foreign blocks, values only those steps answer, are checked against the chain over an empty plan, and a block
//! no case answers must get exactly what that empty-plan chain gives it.
//!
//! The section of the specification between its `GENERATED` markers is rendered from the manifest here, and the
//! test fails while the committed one differs (`UPDATE_SPECS=1` rewrites it).

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::{Value as JsonValue, json};

use super::assets::ParsedAssets;
use super::content_blocks::{ContentBlockCompileError, ContentBlockPlan};
use super::schema::ChainPosition;

#[derive(Debug, Deserialize)]
struct Manifest {
    #[allow(dead_code)]
    doc: String,
    version: u32,
    positions: Vec<String>,
    blocks: Vec<Block>,
    instances: Vec<Instance>,
}

#[derive(Debug, Deserialize)]
struct Block {
    id: String,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    members: BTreeMap<String, JsonValue>,
    #[serde(default)]
    inner: Option<String>,
    #[serde(default)]
    foreign: Option<JsonValue>,
    #[serde(default)]
    canonical: bool,
    #[serde(default)]
    fallback: bool,
}

#[derive(Debug, Deserialize)]
struct Instance {
    id: String,
    #[allow(dead_code)]
    doc: String,
    cases: Vec<Case>,
    refused: bool,
    expect: Vec<Expectation>,
}

#[derive(Debug, Deserialize)]
struct Case {
    id: String,
    at: String,
    priority: i32,
    form: Form,
    recognises: Vec<String>,
    member: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Form {
    Text,
    Unwrap,
}

#[derive(Debug, Deserialize)]
struct Expectation {
    block: String,
    message: String,
    returned: String,
}

fn repository() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn manifest() -> Manifest {
    let path = repository().join("server/specs/instances/ContentBlockChain.json");
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

fn position(name: &str) -> ChainPosition {
    match name {
        "before_provider_formats" => ChainPosition::BeforeProviderFormats,
        "message_envelope" => ChainPosition::MessageEnvelope,
        "provider_formats" => ChainPosition::ProviderFormats,
        "after_provider_formats" => ChainPosition::AfterProviderFormats,
        other => panic!("`{other}` is not a chain position"),
    }
}

/// The positions in the order the engine consults them, the envelope only for a message's own block.
fn chain(manifest: &Manifest, message: bool) -> Vec<&str> {
    manifest
        .positions
        .iter()
        .map(String::as_str)
        .filter(|p| message || *p != "message_envelope")
        .collect()
}

// ----------------------------------------------------------------------------
// The model's definitions, restated.

/// What one position answers: the case that built the block, a stop, or nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Answer {
    Built(String),
    Stopped,
    Nothing,
}

struct Model<'m> {
    manifest: &'m Manifest,
    instance: &'m Instance,
}

impl Model<'_> {
    fn block(&self, id: &str) -> &Block {
        self.manifest
            .blocks
            .iter()
            .find(|b| b.id == id)
            .unwrap_or_else(|| panic!("no block `{id}`"))
    }

    fn recognises(case: &Case, block: &Block) -> bool {
        block.foreign.is_none()
            && block
                .kind
                .as_ref()
                .is_some_and(|kind| case.recognises.contains(kind))
            && (case.form == Form::Text || block.inner.is_some())
    }

    /// The first recognising case in priority order that builds; a recognising unwrap whose member does not
    /// normalise ends the position. The member is normalised as the same kind of value as the block.
    fn at(&self, position: &str, block: &Block, message: bool) -> Answer {
        let mut cases: Vec<&Case> = self
            .instance
            .cases
            .iter()
            .filter(|c| c.at == position)
            .collect();
        cases.sort_by_key(|c| c.priority);
        for case in cases.into_iter().filter(|c| Self::recognises(c, block)) {
            match case.form {
                Form::Text => {
                    if block
                        .members
                        .get(&case.member)
                        .is_some_and(JsonValue::is_string)
                    {
                        return Answer::Built(case.id.clone());
                    }
                }
                Form::Unwrap => {
                    let inner =
                        self.block(block.inner.as_deref().expect("recognised with a member"));
                    return if self.chain(inner, message) == "none" {
                        Answer::Stopped
                    } else {
                        Answer::Built(case.id.clone())
                    };
                }
            }
        }
        Answer::Nothing
    }

    /// The whole chain: the passthrough, the positions in order, then the fallbacks. A declared block is typed,
    /// so the unknown fallback answers it where no case does.
    fn chain(&self, block: &Block, message: bool) -> String {
        if block.canonical {
            return "canonical".to_string();
        }
        for position in chain(self.manifest, message) {
            if let Answer::Built(case) = self.at(position, block, message) {
                return case;
            }
        }
        if block.fallback || block.foreign.is_none() {
            "fallback".to_string()
        } else {
            "none".to_string()
        }
    }
}

fn refused(instance: &Instance) -> bool {
    instance
        .cases
        .iter()
        .any(|c| c.form == Form::Unwrap && c.member == "$")
}

// ----------------------------------------------------------------------------
// The instance, as a real plan, and the blocks as JSON.

fn realise(manifest: &Manifest, block: &Block) -> JsonValue {
    if let Some(foreign) = &block.foreign {
        return foreign.clone();
    }
    let mut object = serde_json::Map::new();
    object.insert(
        "type".to_string(),
        json!(block.kind.as_deref().expect("a declared block has a kind")),
    );
    for (member, value) in &block.members {
        object.insert(member.clone(), value.clone());
    }
    if let Some(inner) = &block.inner {
        let inner = manifest
            .blocks
            .iter()
            .find(|b| &b.id == inner)
            .expect("the inner block is declared");
        object.insert("inner".to_string(), realise(manifest, inner));
    }
    JsonValue::Object(object)
}

fn asset(instance: &Instance) -> Vec<u8> {
    let cases: Vec<JsonValue> = instance
        .cases
        .iter()
        .map(|case| {
            let kinds = json!({"path": "$.type", "one_of": case.recognises});
            let id = format!("{}.{}", instance.id, case.id);
            let path = if case.member == "$" {
                "$".to_string()
            } else {
                format!("$.{}", case.member)
            };
            match case.form {
                Form::Text => json!({
                    "id": id, "doc": "d", "at": case.at, "priority": case.priority, "where": kinds,
                    "text": {"text": path}
                }),
                Form::Unwrap => json!({
                    "id": id, "doc": "d", "at": case.at, "priority": case.priority,
                    // A root path is present by definition, so the presence test is left out there.
                    "where": if path == "$" { kinds } else { json!({"all": [kinds, {"path": path, "exists": true}]}) },
                    "unwrap": {"from": path}
                }),
            }
        })
        .collect();
    serde_json::to_vec(&json!({"id": instance.id, "doc": "d", "content_blocks": cases}))
        .expect("serialises")
}

/// What the chain answers for a block, derived from the model: the case it names, built from the manifest, or
/// the chain's own steps - which are the engine's, so asked of the chain over no cases.
fn expected(
    model: &Model<'_>,
    empty: &ContentBlockPlan,
    block: &Block,
    message: bool,
) -> Option<JsonValue> {
    match model.chain(block, message).as_str() {
        "none" | "fallback" | "canonical" => crate::sideml::content::normalize_block_in(
            empty,
            &realise(model.manifest, block),
            message,
        ),
        case => Some(built(model, empty, case, block, message)),
    }
}

/// The block a case builds, as the engine writes it: a text case from its member, an unwrap from what the model
/// says its member is - under the same `message`, so an envelope inside a returned value is visible here.
fn built(
    model: &Model<'_>,
    empty: &ContentBlockPlan,
    case: &str,
    block: &Block,
    message: bool,
) -> JsonValue {
    let case = model
        .instance
        .cases
        .iter()
        .find(|c| c.id == case)
        .expect("an instance case");
    match case.form {
        Form::Text => json!({"type": "text", "text": block.members[&case.member]}),
        Form::Unwrap => {
            let inner = model.block(
                block
                    .inner
                    .as_deref()
                    .expect("an unwrap's block has a member"),
            );
            expected(model, empty, inner, message).expect("a built unwrap's member normalises")
        }
    }
}

#[test]
fn content_block_chain_instances() {
    let manifest = manifest();

    // The foreign blocks are the chain's own steps' business, and the model's flags must say what they do.
    let empty = ContentBlockPlan::default();
    for block in manifest.blocks.iter().filter(|b| b.foreign.is_some()) {
        let answered =
            crate::sideml::content::normalize_block_in(&empty, &realise(&manifest, block), true)
                .is_some();
        assert_eq!(
            answered,
            block.canonical || block.fallback,
            "the manifest says `{}` is {} by the engine's own steps",
            block.id,
            if block.canonical || block.fallback {
                "answered"
            } else {
                "not answered"
            }
        );
    }

    let mut checked = 0usize;
    for instance in &manifest.instances {
        assert_eq!(
            refused(instance),
            instance.refused,
            "{}: the manifest and the model disagree",
            instance.id
        );
        let assets = ParsedAssets::parse(&BTreeMap::from([(
            format!("{}.json", instance.id),
            asset(instance),
        )]))
        .unwrap_or_else(|error| panic!("{}: does not parse: {error}", instance.id));
        let compiled = ContentBlockPlan::compile(assets.files());
        if instance.refused {
            assert!(
                matches!(
                    compiled,
                    Err(ContentBlockCompileError::UnwrapsWholeBlock { .. })
                ),
                "{}: an unwrap of the whole block must be refused, and compiled to {compiled:?}",
                instance.id
            );
            continue;
        }
        let plan =
            compiled.unwrap_or_else(|error| panic!("{}: does not compile: {error}", instance.id));
        let model = Model {
            manifest: &manifest,
            instance,
        };
        for block in manifest.blocks.iter().filter(|b| b.foreign.is_none()) {
            let json = realise(&manifest, block);
            for message in [true, false] {
                for name in &manifest.positions {
                    let engine = plan.normalize_with(&json, position(name), message);
                    let expected = match model.at(name, block, message) {
                        Answer::Built(case) => Some(built(&model, &empty, &case, block, message)),
                        Answer::Stopped | Answer::Nothing => None,
                    };
                    assert_eq!(
                        engine,
                        expected,
                        "{}: `{}` at {name}, as a {}",
                        instance.id,
                        block.id,
                        if message {
                            "message's block"
                        } else {
                            "returned value"
                        }
                    );
                    checked += 1;
                }
            }
        }
        for expectation in &instance.expect {
            let block = model.block(&expectation.block);
            for (message, wanted) in [(true, &expectation.message), (false, &expectation.returned)]
            {
                assert_eq!(
                    &model.chain(block, message),
                    wanted,
                    "{}: the definition answers `{}` differently",
                    instance.id,
                    block.id
                );
                // The production chain itself, entered as a message's block or as a returned value. Where no case
                // answers, the block gets what the chain's own steps give it, which is the chain over no cases.
                if block.foreign.is_none() {
                    let json = realise(&manifest, block);
                    let engine = crate::sideml::content::normalize_block_in(&plan, &json, message);
                    let expected = expected(&model, &empty, block, message);
                    assert_eq!(
                        engine, expected,
                        "{}: `{}` through the chain",
                        instance.id, block.id
                    );
                }
            }
        }
    }
    assert!(checked >= 40, "only {checked} block positions were checked");

    let path = repository().join("server/specs/ContentBlockChain.tla");
    let spec = std::fs::read_to_string(&path).expect("the specification exists");
    let begin = spec
        .find("\\* BEGIN GENERATED FROM")
        .expect("the specification marks its generated section");
    let end_marker = "\\* END GENERATED FROM instances/ContentBlockChain.json\n";
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

fn tla_set<'a>(items: impl IntoIterator<Item = &'a String>) -> String {
    let items: Vec<String> = items.into_iter().map(|item| tla_string(item)).collect();
    format!("{{{}}}", items.join(", "))
}

fn tla_seq<'a>(items: impl IntoIterator<Item = &'a String>) -> String {
    let items: Vec<String> = items.into_iter().map(|item| tla_string(item)).collect();
    if items.is_empty() {
        "<<>>".to_string()
    } else {
        format!("<<{}>>", items.join(", "))
    }
}

fn tla_bool(value: bool) -> &'static str {
    if value { "TRUE" } else { "FALSE" }
}

fn generated_section(manifest: &Manifest) -> String {
    let mut out = String::new();
    out.push_str("\\* BEGIN GENERATED FROM instances/ContentBlockChain.json\n");
    out.push_str(&format!(
        "ManifestPositions == {}\n",
        tla_seq(&manifest.positions)
    ));
    let blocks: Vec<String> = manifest
        .blocks
        .iter()
        .map(|block| {
            // The members a text case can build from: the ones holding a string.
            let strings: Vec<String> =
                block.members.iter().filter(|(_, v)| v.is_string()).map(|(k, _)| k.clone()).collect();
            format!(
                "    [id |-> {}, kind |-> {}, strings |-> {}, inner |-> {}, foreign |-> {}, canonical |-> {}, fallback |-> {}]",
                tla_string(&block.id),
                tla_string(block.kind.as_deref().unwrap_or("")),
                tla_set(&strings),
                tla_string(block.inner.as_deref().unwrap_or("none")),
                tla_bool(block.foreign.is_some()),
                tla_bool(block.canonical),
                tla_bool(block.fallback)
            )
        })
        .collect();
    out.push_str(&format!(
        "ManifestBlocks == {{\n{}\n}}\n",
        blocks.join(",\n")
    ));
    let instances: Vec<String> = manifest
        .instances
        .iter()
        .map(|instance| {
            let cases: Vec<String> = instance
                .cases
                .iter()
                .map(|case| {
                    format!(
                        "[id |-> {}, at |-> {}, prio |-> {}, form |-> {}, recognises |-> {}, member |-> {}]",
                        tla_string(&case.id),
                        tla_string(&case.at),
                        case.priority,
                        tla_string(match case.form {
                            Form::Text => "text",
                            Form::Unwrap => "unwrap",
                        }),
                        tla_set(&case.recognises),
                        tla_string(&case.member)
                    )
                })
                .collect();
            let expect: Vec<String> = instance
                .expect
                .iter()
                .map(|e| {
                    format!(
                        "[block |-> {}, message |-> {}, returned |-> {}]",
                        tla_string(&e.block),
                        tla_string(&e.message),
                        tla_string(&e.returned)
                    )
                })
                .collect();
            format!(
                "    [id |-> {}, cases |-> {{{}}}, refused |-> {}, expect |-> {}]",
                tla_string(&instance.id),
                cases.join(", "),
                tla_bool(instance.refused),
                if expect.is_empty() { "<<>>".to_string() } else { format!("<< {} >>", expect.join(", ")) }
            )
        })
        .collect();
    out.push_str(&format!(
        "ManifestInstances == <<\n{}\n>>\n",
        instances.join(",\n")
    ));
    out.push_str("\\* END GENERATED FROM instances/ContentBlockChain.json\n");
    out
}
