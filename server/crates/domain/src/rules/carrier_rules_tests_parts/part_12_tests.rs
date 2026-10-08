/// The vocabulary SideSeat itself writes: every member name and type tag a canonical SideML message, block, role
/// and finish reason serialises with. Production Rust owns these constructors, so naming them is its job.
///
/// Derived by serialising one of each, not listed: a list would drift from the types, and a word on it that the
/// types do not write would be an exemption nothing earned. `every_block_kind_is_sampled` holds the samples to
/// the enum, so a new block kind cannot be left out of what Rust is allowed to spell.
fn canonical_sideml_vocabulary() -> std::collections::BTreeSet<String> {
    use crate::sideml::{
        CacheControl, ChatMessage, ChatRole, FinishReason, JsonSchemaDetails, ResponseFormat,
        ToolChoice,
    };
    use strum::VariantArray;

    fn collect(value: &serde_json::Value, out: &mut std::collections::BTreeSet<String>) {
        match value {
            serde_json::Value::Object(members) => {
                for (key, member) in members {
                    out.insert(key.clone());
                    collect(member, out);
                }
            }
            serde_json::Value::Array(items) => items.iter().for_each(|item| collect(item, out)),
            // Type tags, and enum values a canonical type serialises as a bare string - a role, a finish reason,
            // a tool choice - are spellings of SideSeat's own; free text in a sample is not, and the samples hold
            // none (every free-text field is `"x"`).
            serde_json::Value::String(text) if text != "x" => {
                out.insert(text.clone());
            }
            _ => {}
        }
    }

    let x = || "x".to_string();
    let any = || serde_json::json!("x");
    let mut out = std::collections::BTreeSet::new();
    for block in canonical_block_samples() {
        collect(
            &serde_json::to_value(&block).expect("a block serialises"),
            &mut out,
        );
    }
    let mut message = ChatMessage::new(ChatRole::User);
    message.name = Some(x());
    message.content = canonical_block_samples();
    message.tool_use_id = Some(x());
    message.finish_reason = Some(FinishReason::Stop);
    message.index = Some(0);
    message.tool_choice = Some(ToolChoice::Function { name: x() });
    message.response_format = Some(ResponseFormat::JsonSchema {
        json_schema: JsonSchemaDetails {
            name: Some(x()),
            schema: Some(any()),
            strict: Some(true),
        },
    });
    message.model = Some(x());
    message.cache_control = Some(CacheControl { cache_type: x() });
    message.stop = Some(vec![x()]);
    message.parallel_tool_calls = Some(true);
    collect(
        &serde_json::to_value(&message).expect("a message serialises"),
        &mut out,
    );
    for choice in [ToolChoice::Auto, ToolChoice::None, ToolChoice::Required] {
        collect(
            &serde_json::to_value(&choice).expect("serialises"),
            &mut out,
        );
    }
    for format in [ResponseFormat::Text, ResponseFormat::JsonObject] {
        collect(
            &serde_json::to_value(&format).expect("serialises"),
            &mut out,
        );
    }
    out.extend(
        ChatRole::VARIANTS
            .iter()
            .map(|role| role.as_str().to_string()),
    );
    out.extend(
        FinishReason::VARIANTS
            .iter()
            .map(|reason| reason.as_str().to_string()),
    );
    // What a media block's `source` holds is SideSeat's own vocabulary too, but the type holds it as a `String`,
    // so no serialisation names the values: a stored reference, inline bytes, a location, a provider file id.
    out.extend(["file", "base64", "url", "file_id"].map(String::from));
    out
}

/// One of every canonical block kind.
fn canonical_block_samples() -> Vec<crate::sideml::ContentBlock> {
    use crate::sideml::ContentBlock as B;
    let x = || "x".to_string();
    let any = || serde_json::json!("x");
    vec![
        B::Text { text: x() },
        B::Image {
            media_type: Some(x()),
            source: x(),
            data: x(),
            detail: Some(x()),
        },
        B::Audio {
            media_type: Some(x()),
            source: x(),
            data: x(),
        },
        B::Document {
            media_type: Some(x()),
            name: Some(x()),
            source: x(),
            data: x(),
        },
        B::Video {
            media_type: Some(x()),
            source: x(),
            data: x(),
        },
        B::File {
            media_type: Some(x()),
            name: Some(x()),
            source: x(),
            data: x(),
        },
        B::ToolUse {
            id: Some(x()),
            name: x(),
            input: any(),
        },
        B::ToolResult {
            tool_use_id: Some(x()),
            name: Some(x()),
            content: any(),
            is_error: true,
        },
        B::ToolDefinitions {
            tools: vec![any()],
            tool_choice: Some(any()),
        },
        B::Context {
            data: any(),
            context_type: Some(x()),
        },
        B::Refusal { message: x() },
        B::Json { data: any() },
        B::Thinking {
            text: x(),
            signature: Some(x()),
        },
        B::RedactedThinking { data: x() },
        B::Unknown { raw: any() },
    ]
}

/// The samples hold every block kind, which is what lets the canonical vocabulary be derived from them.
#[test]
fn every_block_kind_is_sampled() {
    use crate::sideml::ContentBlock as B;
    // Exhaustive, with no wildcard: a new kind fails to compile here until it has an index, and the count
    // below then fails until it has a sample.
    let index = |block: &B| match block {
        B::Text { .. } => 0,
        B::Image { .. } => 1,
        B::Audio { .. } => 2,
        B::Document { .. } => 3,
        B::Video { .. } => 4,
        B::File { .. } => 5,
        B::ToolUse { .. } => 6,
        B::ToolResult { .. } => 7,
        B::ToolDefinitions { .. } => 8,
        B::Context { .. } => 9,
        B::Refusal { .. } => 10,
        B::Json { .. } => 11,
        B::Thinking { .. } => 12,
        B::RedactedThinking { .. } => 13,
        B::Unknown { .. } => 14,
    };
    let seen: std::collections::BTreeSet<usize> =
        canonical_block_samples().iter().map(index).collect();
    assert_eq!(seen, (0..15).collect(), "a block kind has no sample");
}

/// Every word a producer or vocabulary asset declares as a payload member, a block type, a role spelling or a
/// span, event or scope name - the telemetry vocabulary that is not a published convention and not SideSeat's
/// own.
///
/// Taken from the assets by position. Every JSONPath contributes its member segments and filter operands; every
/// other string contributes itself, **except** under the keys listed in `NOT_PRODUCER_WORDS` - documentation,
/// clause identities, the grammar's own enum values, and the targets a rule maps producer words *onto*, which
/// are SideSeat's vocabulary. A blocklist rather than a list of producer-word keys, so a key the grammar gains
/// is counted until someone states why it holds no producer word. Dotted names are left to
/// `no_production_module_spells_a_framework_attribute_key`, and words that are not word-shaped - prose, patterns,
/// templates - are left out, since a production literal equal to one is no leak of a member name.
fn declared_producer_vocabulary() -> std::collections::BTreeMap<String, String> {
    fn is_word(text: &str) -> bool {
        // JSON's own literals, which a map keyed by a payload's text writes when the text is a flag: no
        // producer owns the spelling `true`.
        !matches!(text, "true" | "false" | "null")
            && text.len() >= 2
            && !text.contains('.')
            && text
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && text
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | ':'))
    }
    /// The member names a JSONPath selects by name - `.name`, `['name']` - and the quoted operands its filters
    /// compare against, which are payload values: a block type, a role.
    fn path_words(path: &str, out: &mut Vec<String>) {
        let chars: Vec<char> = path.chars().collect();
        let mut index = 0;
        while index < chars.len() {
            match chars[index] {
                '.' => {
                    let start = index + 1;
                    let mut end = start;
                    while end < chars.len() && (chars[end].is_alphanumeric() || chars[end] == '_') {
                        end += 1;
                    }
                    if end > start {
                        out.push(chars[start..end].iter().collect());
                    }
                    index = end.max(index + 1);
                }
                quote @ ('\'' | '"') => {
                    let start = index + 1;
                    let mut end = start;
                    while end < chars.len() && chars[end] != quote {
                        end += 1;
                    }
                    out.push(chars[start..end.min(chars.len())].iter().collect());
                    index = end + 1;
                }
                _ => index += 1,
            }
        }
    }
    /// Keys whose free-string values hold no producer word, each group with its reason. Enumerated grammar values
    /// need no entry here: the schema says where those are (`grammar_value_positions`).
    const NOT_PRODUCER_WORDS: &[&str] = &[
        // Prose and identity.
        "doc",
        "id",
        "$schema",
        "rule",
        "supersedes",
        "then_fragment",
        "ordering_family",
        "label",
        "slug",
        // What a rule maps a producer's words onto, where the schema states it as a free string: SideSeat's own
        // observation types, categories and block members.
        "result",
        "replaces_legacy_result",
        "as",
        "capture_as",
        "key_as",
        "as_member",
        "model",
        // Text a rule matches or splits on rather than a member it names, and the id templates it builds.
        "needles",
        "repr_markers",
        "name_label",
        "description_label",
        "arguments_label",
        "split_on",
        "tag_prefix",
        "prepend",
        "join",
        "template",
        "strip_prefix",
        "default",
        "name_default",
        "content_default",
        "media_type_default",
        "type_default",
        // Published namespaces, which the conventions own.
        "convention_namespaces",
    ];
    fn walk(
        value: &serde_json::Value,
        key: Option<&str>,
        at: &str,
        grammar: &std::collections::BTreeSet<String>,
        grammar_at: &std::collections::BTreeSet<String>,
        out: &mut Vec<String>,
    ) {
        match value {
            serde_json::Value::String(text) => {
                // Release provenance in an `observed_in` range - a package name, a capture configuration - which
                // the matrix names and no payload carries. Only in that position: the same member anywhere else
                // is counted, and the framework-name sweep still sees a package name in code.
                if matches!(key, Some("package" | "profile")) && at.contains(".observed_in[") {
                    return;
                }
                if key.is_some_and(|key| NOT_PRODUCER_WORDS.contains(&key))
                    || grammar_at.contains(at)
                {
                    return;
                }
                if text.starts_with('$') || text.starts_with('@') {
                    path_words(text, out);
                } else if let Some(rest) = text.strip_prefix("attr:") {
                    out.push(rest.to_string());
                } else if key == Some("sources") {
                    // A text source is `attr:<key>` or one of the engine's own source names.
                } else {
                    out.push(text.clone());
                }
            }
            serde_json::Value::Array(items) => {
                for (index, item) in items.iter().enumerate() {
                    walk(
                        item,
                        key,
                        &format!("{at}[{index}]"),
                        grammar,
                        grammar_at,
                        out,
                    );
                }
            }
            serde_json::Value::Object(members) => {
                for (member, inner) in members {
                    // A member the grammar defines is the grammar's; any other is a map's key - a role map's,
                    // a type map's - which is a producer's spelling. Decided where the key *is*, so a grammar
                    // word a producer also writes as data is still counted where it is data.
                    if !grammar.contains(member) {
                        out.push(member.clone());
                    }
                    walk(
                        inner,
                        Some(member),
                        &format!("{at}.{member}"),
                        grammar,
                        grammar_at,
                        out,
                    );
                }
            }
            _ => {}
        }
    }
    // The rule grammar's own words: every property and enum value of the generated schema.
    fn grammar_words(value: &serde_json::Value, out: &mut std::collections::BTreeSet<String>) {
        match value {
            serde_json::Value::Object(members) => {
                if let Some(properties) = members.get("properties").and_then(|p| p.as_object()) {
                    out.extend(properties.keys().cloned());
                }
                for key in ["enum", "const"] {
                    match members.get(key) {
                        Some(serde_json::Value::Array(values)) => {
                            out.extend(values.iter().filter_map(|v| v.as_str()).map(str::to_string))
                        }
                        Some(serde_json::Value::String(value)) => {
                            out.insert(value.clone());
                        }
                        _ => {}
                    }
                }
                members.values().for_each(|inner| grammar_words(inner, out));
            }
            serde_json::Value::Array(items) => {
                items.iter().for_each(|item| grammar_words(item, out))
            }
            _ => {}
        }
    }
    let mut ours: std::collections::BTreeSet<String> = canonical_sideml_vocabulary();
    let generator = schemars::generate::SchemaSettings::draft2020_12().into_generator();
    let grammar_at = crate::rules::schema_census::grammar_value_positions();
    let mut grammar = std::collections::BTreeSet::new();
    grammar_words(
        &serde_json::to_value(generator.into_root_schema_for::<crate::rules::schema::RuleFile>())
            .expect("the schema serialises"),
        &mut grammar,
    );
    // SideSeat's own intermediate shape for a tool call - the `tool_calls` list of `{id, function: {name,
    // arguments}}` the message rules build and every reader consumes - and the member a tool message's call id
    // is carried in on the way to SideML's `tool_use_id`. Engine vocabulary that one convention's spelling
    // happens to share; the producers' other spellings of the same things are declared (`alias_of`).
    ours.extend(["tool_calls", "tool_call_id"].map(String::from));
    // The same for a tool definition - `{name, description, parameters}` - and the JSON Schema primitive types
    // its parameters are written in, a published standard the tool-shape rules convert producers' types onto.
    ours.extend(["description", "parameters"].map(String::from));
    ours.extend(
        crate::rules::schema::UnknownType::PRIMITIVES
            .iter()
            .map(|primitive| primitive.to_string()),
    );
    // The classifications' answers, which `classify` states are ours.
    ours.extend(
        crate::rules::classify::OBSERVATION_TYPES
            .iter()
            .chain(crate::rules::classify::SPAN_CATEGORIES)
            .map(|word| word.to_string()),
    );

    let mut declared: std::collections::BTreeMap<String, String> = Default::default();
    let mut published: std::collections::BTreeSet<String> = Default::default();
    for (path, bytes) in crate::rules::schema::embedded_sources() {
        let id = path
            .rsplit('/')
            .next()
            .unwrap_or(&path)
            .trim_end_matches(".json")
            .to_string();
        let value: serde_json::Value = serde_json::from_slice(&bytes).expect("the asset parses");
        let mut words = Vec::new();
        walk(&value, None, &path, &grammar, &grammar_at, &mut words);
        let words = words.into_iter().filter(|word| is_word(word));
        if path.starts_with("conventions/") {
            published.extend(words);
        } else {
            for word in words {
                declared.entry(word).or_insert_with(|| id.clone());
            }
        }
    }
    declared.retain(|word, _| !published.contains(word) && !ours.contains(word));
    declared
}

/// Source files that are modules compiled only for tests: declared `#[cfg(test)] mod name;` or behind this crate's
/// test-support gate, and every file beneath one. `production_names` strips an inline gated item, and a gated
/// module **file** has no attribute of its own to strip - so without this, an oracle file reads as production.
fn test_only_module_files(repository: &std::path::Path) -> std::collections::BTreeSet<String> {
    let mut gated: Vec<std::path::PathBuf> = Vec::new();
    walk_production_rust_sources(&mut |path, source| {
        let lines: Vec<&str> = source.lines().map(str::trim).collect();
        for (index, line) in lines.iter().enumerate() {
            if *line != "#[cfg(test)]" && *line != "#[cfg(any(test, feature = \"test-support\"))]" {
                continue;
            }
            let mut next = index + 1;
            let mut explicit: Option<&str> = None;
            while let Some(attribute) = lines.get(next).filter(|l| l.starts_with("#[")) {
                if let Some(rest) = attribute.strip_prefix("#[path = \"") {
                    explicit = rest.split('"').next();
                }
                next += 1;
            }
            let Some(declaration) = lines.get(next) else {
                continue;
            };
            let declaration = declaration
                .trim_start_matches("pub(crate) ")
                .trim_start_matches("pub(super) ")
                .trim_start_matches("pub ");
            let Some(name) = declaration
                .strip_prefix("mod ")
                .and_then(|rest| rest.strip_suffix(';'))
            else {
                continue;
            };
            let directory = path.parent().expect("a source file has a directory");
            let is_mod_rs = matches!(
                path.file_name().and_then(|f| f.to_str()),
                Some("mod.rs" | "lib.rs" | "main.rs")
            );
            let base = if is_mod_rs {
                directory.to_path_buf()
            } else {
                directory.join(path.file_stem().expect("a file stem"))
            };
            match explicit {
                Some(file) => gated.push(directory.join(file)),
                None => {
                    gated.push(base.join(format!("{name}.rs")));
                    gated.push(base.join(name).join("mod.rs"));
                }
            }
        }
    });
    let relative = |path: &std::path::Path| {
        path.strip_prefix(repository)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/")
    };
    let mut out = std::collections::BTreeSet::new();
    walk_production_rust_sources(&mut |path, _| {
        let file = relative(path);
        let under = gated.iter().any(|gate| {
            let gate = relative(gate);
            file == gate
                || gate
                    .strip_suffix(".rs")
                    .is_some_and(|stem| file.starts_with(&format!("{stem}/")))
                || gate
                    .strip_suffix("/mod.rs")
                    .is_some_and(|stem| file.starts_with(&format!("{stem}/")))
        });
        if under {
            out.insert(file);
        }
    });
    out
}

/// No production Rust spells a word only a producer's or a vocabulary's asset declares.
///
/// The third sweep, for what the other two cannot see: `no_production_module_names_a_framework` catches a
/// framework's name and `no_production_module_spells_a_framework_attribute_key` its dotted attribute keys, and
/// neither saw `"toolUse"`, `"cachePoint"` or `"functionCall"` - a payload's member names, block types and role
/// spellings, which is where most of the remaining knowledge lived. The vocabulary is derived from the assets
/// (see `declared_producer_vocabulary`), so a word becomes forbidden in Rust the day an asset declares it.
///
/// Exempt, each for a stated reason: a published convention's words (the conventions assets), SideSeat's own
/// canonical vocabulary (derived from the SideML types, the classification answers and the rule grammar), a
/// priced or connected provider's name, and test code; and `ENTITLED`, a short list of sites where the word is a
/// published format's or SideSeat's own at that site, each with its reason. Zero tolerance beyond that. What it
/// cannot see: a word assembled at runtime, a word no asset declares yet, and prose.
#[test]
fn no_production_module_spells_a_declared_producer_word() {
    let vocabulary = declared_producer_vocabulary();
    assert!(
        vocabulary.len() > 200,
        "only {} producer words were derived from the assets, so the derivation is wrong",
        vocabulary.len()
    );
    let repository = repository_root();
    let mut found: std::collections::BTreeMap<(String, String), String> = Default::default();
    let mut checked = 0_usize;
    // The crates that interpret a producer's telemetry. The rest build SideSeat's own storage, queries and API,
    // whose member names are SideSeat's - `input_tokens` is a stored column there, whatever a producer calls
    // its counter. Every crate is classified, so a new one is a decision rather than an omission.
    const INTERPRETS_TELEMETRY: &[&str] = &["domain", "ingestion"];
    const OWN_VOCABULARY_ONLY: &[&str] = &[
        "core",
        "ports",
        "messaging",
        "query-sql",
        "rule-assets",
        "api",
        "adapter-blob-storage",
        "adapter-cache",
        "adapter-clickhouse",
        "adapter-duckdb",
        "adapter-postgres",
        "adapter-pricing",
        "adapter-provider-probe",
        "adapter-registrations-memory",
        "adapter-secrets",
        "adapter-sqlite",
        "adapter-topics",
    ];
    for entry in
        std::fs::read_dir(repository.join("server/crates")).expect("the crates directory reads")
    {
        let name = entry
            .expect("a crate entry")
            .file_name()
            .to_string_lossy()
            .to_string();
        assert!(
            INTERPRETS_TELEMETRY.contains(&name.as_str())
                || OWN_VOCABULARY_ONLY.contains(&name.as_str()),
            "the crate `{name}` is not classified: does it interpret a producer's telemetry?"
        );
    }
    // Provider connectors name the provider they connect to, as `no_production_module_names_a_framework` allows.
    const CONNECTORS: &[&str] = &["server/crates/domain/src/providers/"];
    // And pricing names the providers it prices, as AGENTS.md allows - there, and nowhere else in parsing.
    const PRICING: &[&str] = &["server/crates/domain/src/pricing/"];
    let is_provider = |word: &str| {
        !crate::pricing::builtin_provider(&word.to_lowercase().replace('-', "_")).is_empty()
    };
    let test_only = test_only_module_files(&repository);
    for oracle in [
        "server/crates/domain/src/sideml/content/provider_formats.rs",
        "server/crates/domain/src/rules/schema_census.rs",
        "server/crates/ingestion/src/traces/extract/framework_oracle.rs",
    ] {
        assert!(
            test_only.contains(oracle),
            "`{oracle}` is a test-only module and the gate detection missed it: {test_only:?}"
        );
    }
    walk_production_rust_sources(&mut |path, source| {
        let relative = path
            .strip_prefix(&repository)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        if relative.contains("_tests.rs") || relative.ends_with("/tests.rs") {
            return;
        }
        let interprets = INTERPRETS_TELEMETRY
            .iter()
            .any(|krate| relative.starts_with(&format!("server/crates/{krate}/src/")));
        if !interprets
            || CONNECTORS.iter().any(|prefix| relative.starts_with(prefix))
            || test_only.contains(&relative)
        {
            return;
        }
        checked += 1;
        for (line, text) in production_names(source, &relative) {
            // String literals only: an identifier, and a run of literals joined for the name sweep, are not.
            let is_string =
                (text.starts_with('"') || text.starts_with("r\"") || text.starts_with("r#"))
                    && (text.ends_with('"') || text.ends_with('#'));
            if !is_string {
                continue;
            }
            let literal = literal_text(&text);
            let literal = literal.as_ref();
            if PRICING.iter().any(|prefix| relative.starts_with(prefix)) && is_provider(literal) {
                continue;
            }
            if let Some(asset) = vocabulary.get(literal) {
                found
                    .entry((relative.clone(), literal.to_string()))
                    .or_insert_with(|| format!("  {relative}:{line}: \"{literal}\" <- `{asset}`"));
            }
        }
    });
    assert!(checked > 50, "the sweep checked only {checked} files");

    /// Sites that spell a word an asset also declares, and are entitled to: the word is a published format's or
    /// SideSeat's own at that site, whatever a producer's asset uses it for. Permanent, each with its reason.
    const ENTITLED: &[(&str, &str, &str)] = &[
        (
            "server/crates/ingestion/src/otlp.rs",
            "double",
            "OTLP's own AnyValue kind, tagging a value no JSON number holds",
        ),
        (
            "server/crates/ingestion/src/otlp.rs",
            "bytes",
            "OTLP's own AnyValue kind, tagging a bytes value",
        ),
        (
            "server/crates/ingestion/src/traces/persist/flattening.rs",
            "status",
            "the OTLP span record's own member",
        ),
        (
            "server/crates/ingestion/src/traces/persist/flattening.rs",
            "events",
            "the OTLP span record's own member",
        ),
        (
            "server/crates/domain/src/rules/message_rules/build.rs",
            "capture",
            "the engine's own section subject, which a `skip_when` predicate reads",
        ),
        (
            "server/crates/domain/src/rules/message_rules/build.rs",
            "body",
            "the engine's own section subject, which a `skip_when` predicate reads",
        ),
        (
            "server/crates/domain/src/sideml/content/python_repr.rs",
            "__python_constructor",
            "the engine's own tree for a parsed constructor, which assets select from",
        ),
        (
            "server/crates/domain/src/sideml/normalize/categorization.rs",
            "documents",
            "one of SideSeat's own extraction roles, which the assets map retrieved material onto; the role-authority \
             asset declares its authority, not a producer's spelling",
        ),
        (
            "server/crates/domain/src/sideml/mod.rs",
            "conversation_history",
            "SideSeat's own context type for a history message, which one producer's asset names as its target",
        ),
        (
            "server/crates/ingestion/src/metrics/extract.rs",
            "value",
            "the OTLP summary quantile's own member",
        ),
        (
            "server/crates/domain/src/pricing/mod.rs",
            "chat",
            "the pricing catalogue's default model mode, as the price list it loads spells it",
        ),
        (
            "server/crates/domain/src/sideml/content/python_repr.rs",
            "__python_args",
            "the engine's own tree for a parsed constructor, which assets select from",
        ),
        (
            "server/crates/domain/src/rules/content_blocks.rs",
            "signature",
            "the normalised thinking block's own member, which a SideML view states as `signed`: the engine \
             builds it, whatever a producer's asset names the same",
        ),
        (
            "server/crates/domain/src/sideml/content.rs",
            "signature",
            "the normalised thinking block's own member, read to keep a block whose text was withheld",
        ),
        (
            "server/crates/domain/src/rate_limit.rs",
            "files",
            "SideSeat's own rate-limit bucket for its file API, whatever member of a message a producer's asset \
             names the same",
        ),
    ];
    let listed = |site: &(String, String)| {
        ENTITLED
            .iter()
            .any(|(file, word, _)| site.0 == *file && site.1 == *word)
    };
    let offences: Vec<&String> = found
        .iter()
        .filter(|(site, _)| !listed(site))
        .map(|(_, line)| line)
        .collect();
    let stale: Vec<String> = ENTITLED
        .iter()
        .filter(|(file, word, _)| !found.contains_key(&(file.to_string(), word.to_string())))
        .map(|(file, word, _)| format!("  {file}: \"{word}\""))
        .collect();
    assert!(
        offences.is_empty(),
        "production Rust spells words only a producer's or vocabulary's asset declares. Read them through the \
         asset that declares them:\n{}",
        offences
            .iter()
            .map(|line| line.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert!(
        stale.is_empty(),
        "these entitled sites no longer spell their word - an entitlement for a word a file does not use is \
         how such a list grows until it covers something that matters:\n{}",
        stale.join("\n")
    );
}
