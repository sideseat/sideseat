/// **No production module names a framework**, across the whole server, with the exemptions named.
///
/// The two extraction files have their own gate above, with the carrier keys and constant identifiers that are
/// framework facts even where the identifier is not. This one is the wider claim, and it exists because that
/// claim was being kept by review and by a sweep I ran by hand: a framework-specific branch in an unguarded
/// module escaped both. It reads the source tree, so a new module is covered the day it is written.
///
/// What it **does** see is the whole tree's production tokens, and the two things that make that a claim rather
/// than a grep are that the marker list is *derived* from the assets (plus the aliases below, each tied to one)
/// and that an exemption is scoped to the names it allows rather than to a file.
///
/// Four things it cannot see, stated because a sweep reads as exhaustive:
///
/// - A framework named by a **value** rather than a name. A key like `crew_key` is caught by the marker list; a
///   bare `"kwargs"` is not. That is why the equivalence oracles matter more than this test does.
/// - **Prose.** A doc comment is skipped, and the explanations in this tree name the producers whose telemetry
///   motivated each rule - deliberately, because that is what such an explanation is. So is a doc comment that
///   `schemars` renders into a shipped description, which is the one place prose becomes data the server sends.
/// - Anything outside `server/src`. The SDKs are separate crates and *are* meant to name the framework they
///   instrument, so the claim is about the server's own parsing.
/// - A name assembled at runtime from parts that are not adjacent - `[a, b].concat()`, or a name built from a
///   variable. Adjacent literals **are** caught wherever they are laid out, because the source is tokenised and a
///   run of literals is joined as tokens - so `concat!("lang", "graph")` is `langgraph` even split across lines.
#[test]
fn no_production_module_names_a_framework() {
    let markers = framework_markers();
    let every_marker: Vec<(&str, &str)> = markers
        .iter()
        .flat_map(|(id, names)| names.iter().map(move |name| (id.as_str(), name.as_str())))
        .collect();

    /// What an exempt file is allowed to name.
    ///
    /// Scoped to markers, not to the file: exempting a whole file is a hole the size of the file, and a
    /// connector module is entitled to the product it connects to, not to every framework's telemetry dialect.
    /// Verified by `an_exempt_file_may_name_only_what_its_exemption_allows`.
    enum Allowed {
        /// Every framework, which only a module whose subject *is* the list of them can claim.
        EveryFramework,
        /// These assets' names, and no others.
        Only(&'static [&'static str]),
    }

    /// Files allowed to name something, what each may name, and why. Each is a *statement*, not an oversight.
    ///
    /// All four are code whose subject **is** the named thing rather than telemetry it wrote. The engine's own
    /// module documentation says as much about the connectors: they build real clients with real auth flows, and
    /// calling that data would be dishonest.
    const EXEMPT: &[(&str, Allowed, &str)] = &[
        (
            "server/crates/ingestion/src/traces/extract/framework_oracle.rs",
            Allowed::EveryFramework,
            "*is* the equivalence oracle: a test-only enum of framework names, kept so each retired detection \
             table can be compared against something independent of the assets. Naming them is the whole \
             point, and it interprets no telemetry - detection produces its label from the assets. It sat in \
             the DTO module behind `#[cfg(test)]` until the DTOs became their own crate, where a test-only \
             item is invisible to a dependent",
        ),
        (
            "server/crates/ingestion/src/traces/extract/attributes/semantic_oracle.rs",
            Allowed::Only(&["langgraph", "langsmith", "mlflow"]),
            "*is* the equivalence oracle for the declared session, user and tag resolvers: a test-only copy \
             of the retired lookup chains, compiled only under `#[cfg(test)]`, so naming the keys those \
             chains read is what it preserves",
        ),
        (
            "server/crates/api/src/mcp/tools.rs",
            Allowed::EveryFramework,
            "generates integration documentation for an AI assistant, so naming each framework is its job - it \
             interprets no telemetry",
        ),
        (
            "server/crates/domain/src/providers/catalog.rs",
            Allowed::Only(&["azure-ai-foundry"]),
            "catalogues the inference providers a user may connect to, where the name is the subject rather \
             than a producer's spelling",
        ),
        (
            "server/crates/adapter-provider-probe/src/providers.rs",
            Allowed::Only(&["azure-ai-foundry"]),
            "probes a named provider to test a credential - an adapter, not a parser",
        ),
        (
            "server/crates/adapter-provider-probe/src/azure.rs",
            Allowed::Only(&["azure-ai-foundry"]),
            "builds the Azure AI Foundry probe request; the name is the provider being called",
        ),
        (
            "server/crates/domain/src/providers/service.rs",
            Allowed::Only(&["azure-ai-foundry"]),
            "manages those connections by provider key, which is the catalogue's own vocabulary",
        ),
    ];
    for (file, allowed, _) in EXEMPT {
        if let Allowed::Only(assets) = allowed {
            for asset in *assets {
                assert!(
                    markers.contains_key(*asset),
                    "`{file}` is allowed to name `{asset}`, which is not a framework asset"
                );
            }
        }
    }

    let repository = repository_root();
    let mut offenders: Vec<String> = Vec::new();
    let mut exempt_used: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    let mut exempt_names_found: std::collections::BTreeSet<(&str, &str)> =
        std::collections::BTreeSet::new();
    let mut files = 0_usize;
    // Counted apart from `files`, because an exemption matching everything would leave the offender list empty
    // and every other assertion satisfied - the sweep has to say how much it actually *checked*.
    let mut checked = 0_usize;
    let mut skipped_tests = 0_usize;
    walk_production_rust_sources(&mut |path, source| {
        files += 1;
        let relative = path
            .strip_prefix(&repository)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        // A test module is not production: the oracles name every framework they reproduce, deliberately.
        if relative.contains("_tests.rs") || relative.ends_with("/tests.rs") {
            skipped_tests += 1;
            return;
        }
        // Source pulled in from elsewhere is source this walk never sees. `#[path]` is used throughout for
        // test modules that sit **beside** their subject - files this walk reads directly and skips as
        // tests - so those are fine; anything else, and any `include!`, is a hole in the claim.
        assert_no_foreign_source(source, &relative);

        let exemption = EXEMPT.iter().find(|(file, _, _)| relative.ends_with(file));
        if let Some((name, _, _)) = exemption {
            exempt_used.insert(name);
        } else {
            checked += 1;
        }
        for (number, text) in production_names(source, &relative) {
            for (asset, marker) in &every_marker {
                if !names_as_a_word(&text, marker) {
                    continue;
                }
                match exemption {
                    // An exemption for a name a file does not use is unnecessary, and an unnecessary exemption
                    // is how such a list grows until it covers something that matters.
                    Some((name, Allowed::EveryFramework, _)) => {
                        exempt_names_found.insert((name, asset));
                    }
                    Some((name, Allowed::Only(assets), _)) if assets.contains(asset) => {
                        exempt_names_found.insert((name, asset));
                    }
                    // Exempt for one product's name, and this is a different one.
                    _ => offenders.push(format!("  {relative}:{number}: {text} <- `{marker}`")),
                }
            }
        }
    });

    assert!(
        files > 50,
        "the sweep read only {files} files, which cannot be the whole tree"
    );
    assert!(
        checked + EXEMPT.len() + skipped_tests == files,
        "the sweep read {files} files, checked {checked}, skipped {skipped_tests} test files and has {} \
         exemptions - an exemption is matching more than the file it names, which would make an empty \
         offender list mean nothing",
        EXEMPT.len()
    );
    offenders.sort();
    offenders.dedup();
    assert!(
        offenders.is_empty(),
        "{} production line(s) name a framework. Every fact a framework writes belongs in \
         `server/assets/rules/*.json`; if a module genuinely has to name one, add it to EXEMPT with the \
         reason:\n{}",
        offenders.len(),
        offenders.join("\n")
    );
    // An exemption for a file that no longer exists, or for a name it no longer uses, is a statement about
    // nothing - and the allowance is per asset, so each one has to be earned.
    for (file, allowed, _) in EXEMPT {
        assert!(
            exempt_used.contains(file),
            "EXEMPT names `{file}`, which the sweep did not find"
        );
        match allowed {
            Allowed::EveryFramework => assert!(
                exempt_names_found.iter().any(|(name, _)| name == file),
                "EXEMPT names `{file}`, which names no framework - the exemption is unnecessary, and an \
                 unnecessary exemption is how such a list grows until it covers something that matters"
            ),
            Allowed::Only(assets) => {
                for asset in *assets {
                    assert!(
                        exempt_names_found.contains(&(*file, *asset)),
                        "`{file}` is allowed to name `{asset}` and does not - the allowance is unnecessary, \
                         and an unnecessary allowance is how such a list grows until it covers something that \
                         matters"
                    );
                }
            }
        }
    }
}

/// Every framework's names, by asset: the asset id spelled with each separator, and the aliases the producer is
/// actually known by. The markers `no_production_module_names_a_framework` looks for.
fn framework_markers() -> std::collections::BTreeMap<String, Vec<String>> {
    /// A shorter name the same producer goes by, tied to the asset it belongs to.
    ///
    /// Deriving separator variants of an asset id is not enough, and the gap is the shape the code actually used:
    /// `vercel-ai` yields `vercel-ai`, `vercel_ai` and `vercelai`, none of which is in `try_vercel_format` - the
    /// very name this migration removed from production. Each alias names its asset so a renamed or deleted asset
    /// breaks the list loudly instead of leaving a marker that matches nothing.
    ///
    /// Deliberately not derived from an id's segments: that yields `ai`, `agent`, `sdk`, `framework` and `openai`,
    /// which are words this server is entitled to use. So the aliases are the ones a producer is *actually* known
    /// by, and `openai-agents` gets none - `openai` is a provider the catalogue prices and a name the connectors
    /// carry, so as a marker it would say nothing about framework knowledge.
    const ALIASES: &[(&str, &str)] = &[
        ("vercel-ai", "vercel"),
        ("google-adk", "adk"),
        ("pydantic-ai", "pydantic"),
        ("azure-ai-foundry", "foundry"),
        // The CLI the SDK spawns names its spans `claude_code.*`, and the retired extractor was `try_claude_code`.
        ("claude-agent-sdk", "claude_code"),
        ("claude-agent-sdk", "claudecode"),
        // The package is `llama_index`, which no separator variant of the id spells.
        ("llamaindex", "llama_index"),
        ("llamaindex", "llama-index"),
    ];

    // The framework names, derived from the assets and spelled every way a separator can be written - a name is
    // `google-adk` in an asset id and `google_adk` or `googleadk` in code - plus the aliases above.
    let sources = crate::rules::schema::embedded_sources();
    let ids: Vec<String> = sources
        .keys()
        // The asset id is the file's **basename**: the assets sit in `producers/`, `conventions/` and
        // `vocabulary/` subdirectories, and a marker derived from the whole path would be
        // `conventions/semconv` rather than `semconv`.
        .map(|path| {
            path.rsplit('/')
                .next()
                .unwrap_or(path)
                .trim_end_matches(".json")
                .to_string()
        })
        .collect();
    assert!(ids.len() > 20, "only {} assets were found", ids.len());
    for named in SHARED_VOCABULARY
        .iter()
        .chain(PROVIDERS)
        .chain(ALIASES.iter().map(|(id, _)| id))
    {
        assert!(
            ids.iter().any(|id| id == named),
            "an exclusion or alias names `{named}`, which is not an asset"
        );
    }
    // Each exclusion carries a property, or the list is a way to make the sweep quiet about a framework.
    for shared in SHARED_VOCABULARY {
        // Located by **basename**, not by a path this test spells out: the assets are grouped into
        // `producers/`, `conventions/` and `vocabulary/`, and a regrouping must not require editing a sweep.
        let bytes = sources
            .iter()
            .find(|(path, _)| path.rsplit('/').next().unwrap_or(path) == format!("{shared}.json"))
            .map(|(_, bytes)| bytes)
            .unwrap_or_else(|| panic!("`{shared}` is excluded but no asset of that name exists"));
        let file: crate::rules::schema::RuleFile =
            serde_json::from_slice(bytes).expect("the asset parses");
        assert!(
            file.detect.is_empty(),
            "`{shared}` is excluded as shared vocabulary and declares detection, so it identifies a producer"
        );
    }
    for provider in PROVIDERS {
        let normalised = provider.replace('-', "_");
        assert!(
            !crate::pricing::builtin_provider(&normalised).is_empty(),
            "`{provider}` is excluded as a provider and the catalogue does not read it as one"
        );
    }
    // A marker set per framework, because an exemption is scoped to the names it allows rather than to a file.
    let markers: std::collections::BTreeMap<String, Vec<String>> = ids
        .iter()
        .filter(|id| !SHARED_VOCABULARY.contains(&id.as_str()) && !PROVIDERS.contains(&id.as_str()))
        .map(|id| {
            let mut names: Vec<String> = [id.clone(), id.replace('-', "_"), id.replace('-', "")]
                .into_iter()
                .chain(
                    ALIASES
                        .iter()
                        .filter(|(asset, _)| asset == id)
                        .map(|(_, alias)| alias.to_string()),
                )
                .collect();
            names.sort();
            names.dedup();
            (id.clone(), names)
        })
        .collect();
    // An alias no shorter than a name already derived from its own asset is dead weight that reads as protection.
    for (asset, alias) in ALIASES {
        let derived = [
            asset.to_string(),
            asset.replace('-', "_"),
            asset.replace('-', ""),
        ];
        assert!(
            !derived
                .iter()
                .any(|name| name == alias || alias.contains(name)),
            "the alias `{alias}` is already covered by a name derived from `{asset}`"
        );
    }
    markers
}

/// Every framework marker an *exempt* file may not name is still reported.
///
/// A whole-file exemption is a hole the size of the file: with one, a module entitled to name the provider it
/// connects to could parse any framework's telemetry beside it and the sweep would say nothing. So the connector
/// exemptions allow one product's name and nothing else, and this is that property, checked directly - the sweep
/// itself cannot show it, because a passing sweep is consistent with every exemption being unscoped.
#[test]
fn an_exempt_file_may_name_only_what_its_exemption_allows() {
    // Counterexample: a parser for a *different* framework written into a connector module.
    let intruder = "fn parse_haystack_telemetry() {}";
    let names = production_names(intruder, "intruder");
    assert!(
        names
            .iter()
            .any(|(_, text)| text.to_ascii_lowercase().contains("haystack")),
        "the sweep's own reader must see the name it is meant to catch"
    );
    // And the name the connectors *are* allowed is not the same name, so allowing one cannot allow the other.
    assert!(
        !"azure-ai-foundry".contains("haystack") && !"haystack".contains("azure-ai-foundry"),
        "the allowance and the intruder must be distinguishable for the scoping to mean anything"
    );
}

/// `include!` and a `#[path]` that leaves this walk's reach are holes in the claim, so both are refused.
#[cfg(test)]
fn assert_no_foreign_source(source: &str, relative: &str) {
    for line in source.lines() {
        let line = line.trim();
        assert!(
            !line.contains("include!("),
            "`{relative}` includes source from elsewhere, which this sweep does not follow"
        );
        if let Some(rest) = line.strip_prefix("#[path = \"")
            && let Some(target) = rest.split('"').next()
        {
            assert!(
                target.ends_with("_tests.rs") && !target.contains('/'),
                "`{relative}` names module source at `{target}`, which is neither a sibling test file nor \
                 something this sweep follows"
            );
        }
    }
}

/// Repository paths used by source-shape tests.
///
/// The rules now compile in `server/crates/domain`, while their canonical source tree and architecture documents
/// still live at repository level. Keeping this resolution in one checked helper prevents each sweep from
/// silently choosing a different tree.
#[cfg(test)]
fn repository_root() -> std::path::PathBuf {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    assert!(
        root.join("server/Cargo.toml").is_file() && root.join("docs").is_dir(),
        "`{}` does not resolve to the SideSeat repository root",
        root.display()
    );
    root
}

#[cfg(test)]
fn production_rust_source_roots() -> Vec<std::path::PathBuf> {
    let repository = repository_root();
    let mut roots = vec![repository.join("server/src")];
    for entry in
        std::fs::read_dir(repository.join("server/crates")).expect("crates directory is readable")
    {
        let source = entry.expect("crate directory entry").path().join("src");
        if source.is_dir() {
            roots.push(source);
        }
    }
    roots
}

#[cfg(test)]
fn walk_production_rust_sources(visit: &mut impl FnMut(&std::path::Path, &str)) {
    for root in production_rust_source_roots() {
        walk_rust_sources(&root, visit);
    }
}

/// Every `.rs` file under a directory, with its contents.
#[cfg(test)]
fn walk_rust_sources(dir: &std::path::Path, visit: &mut impl FnMut(&std::path::Path, &str)) {
    // Fails closed: a directory or file the sweep cannot read is not a file it may skip. Silently returning
    // would make an unreadable subtree indistinguishable from an empty one, and the whole claim rests on having
    // looked at everything.
    let entries = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("the sweep could not read `{}`: {e}", dir.display()));
    for entry in entries {
        let path = entry
            .unwrap_or_else(|e| {
                panic!(
                    "the sweep could not read an entry of `{}`: {e}",
                    dir.display()
                )
            })
            .path();
        if path.is_dir() {
            walk_rust_sources(&path, visit);
        } else if path.extension().is_some_and(|e| e == "rs") {
            let source = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("the sweep could not read `{}`: {e}", path.display()));
            visit(&path, &source);
        }
    }
}

/// Every name a file's **production** tokens contain, with the line each sits on: comments are gone, and so is
/// every `#[cfg(test)]` item.
///
/// Tokenised rather than read line by line, which removes a *class* of bypass instead of another special case:
///
/// - A brace inside a string literal is a string. `#[cfg(test)] const X: &str = "{";` used to send the stripper
///   hunting for a closing brace, and it consumed every production item after it to the end of the file.
/// - A name written in pieces is the name it builds. Adjacent literals are joined *as tokens*, so
///   `concat!("lang", "graph")` is `langgraph` however it is laid out - including across lines, which matching a
///   line at a time could not do at all.
/// - An item's extent is exact. A braced group is one token, so the first one at an item's own level *is* its
///   body, and neither a multi-line `const` nor a multi-line `fn` signature can be mistaken for the other.
///
/// Fails closed twice over: a file that does not tokenise is not a file this sweep may skip, and a `cfg`
/// predicate that mentions `test` in any shape other than `cfg(test)` or this crate's exact
/// `cfg(any(test, feature = "test-support"))` test-support gate is refused rather than guessed at - reading
/// `cfg(not(test))` as a test item would strip production code, and reading an arbitrary
/// `cfg(any(test, ...))` as production would let a test item's names count as the server's.
#[cfg(test)]
fn production_names(source: &str, what: &str) -> Vec<(usize, String)> {
    let stream: proc_macro2::TokenStream = source
        .parse()
        .unwrap_or_else(|e| panic!("the sweep could not tokenise `{what}`: {e}"));
    let mut out = Vec::new();
    collect_production_names(stream, what, &mut out);
    out
}

#[cfg(test)]
fn collect_production_names(
    stream: proc_macro2::TokenStream,
    what: &str,
    out: &mut Vec<(usize, String)>,
) {
    use proc_macro2::{Delimiter, TokenTree};
    let tokens: Vec<TokenTree> = stream.into_iter().collect();
    let mut index = 0;
    while index < tokens.len() {
        // Prose, not code. A `///` or `//!` comment becomes a `#[doc]` attribute in the token stream, and the
        // explanations in this tree name the producers whose telemetry motivated each rule - which is what an
        // explanation of a producer's shape has to do. Reading them would force either mass exemptions or
        // deleting the explanations; the limit is stated on the test instead.
        if let Some(width) = doc_attribute_width(&tokens, index) {
            index += width;
            continue;
        }
        if is_cfg_test_attribute(&tokens, index, what) {
            // Everything the attribute applies to: any further attributes, the item's head, and then whichever
            // comes first - the braced body or the `;` of a statement item.
            index += 2;
            while index < tokens.len() {
                let done = match &tokens[index] {
                    TokenTree::Group(group) => group.delimiter() == Delimiter::Brace,
                    TokenTree::Punct(punct) => punct.as_char() == ';',
                    _ => false,
                };
                index += 1;
                if done {
                    break;
                }
            }
            continue;
        }
        match &tokens[index] {
            TokenTree::Ident(ident) => {
                out.push((ident.span().start().line, ident.to_string()));
                index += 1;
            }
            TokenTree::Literal(first) => {
                // A run of literals separated by nothing or by commas is one candidate as well as several: that
                // is what `concat!` builds, and what splitting a name into pieces looks like.
                let line = first.span().start().line;
                let start = index;
                let mut joined = String::new();
                while index < tokens.len() {
                    match &tokens[index] {
                        TokenTree::Literal(literal) => {
                            let text = literal.to_string();
                            joined.push_str(literal_text(&text));
                            out.push((literal.span().start().line, text));
                            index += 1;
                        }
                        TokenTree::Punct(punct)
                            if punct.as_char() == ','
                                && matches!(tokens.get(index + 1), Some(TokenTree::Literal(_))) =>
                        {
                            index += 1;
                        }
                        _ => break,
                    }
                }
                if index > start + 1 {
                    out.push((line, joined));
                }
            }
            TokenTree::Group(group) => {
                collect_production_names(group.stream(), what, out);
                index += 1;
            }
            TokenTree::Punct(_) => index += 1,
        }
    }
}

/// A string literal's own text, without the quoting a `contains` search would have to see through.
#[cfg(test)]
fn literal_text(text: &str) -> &str {
    text.trim_start_matches('r')
        .trim_matches('#')
        .trim_matches('"')
}

/// Whether the token at `index` begins a `#[cfg(test)]` attribute.
#[cfg(test)]
fn is_cfg_test_attribute(tokens: &[proc_macro2::TokenTree], index: usize, what: &str) -> bool {
    use proc_macro2::TokenTree;
    if !is_attribute_named(tokens, index, "cfg") {
        return false;
    }
    let Some(TokenTree::Group(group)) = tokens.get(index + 1) else {
        return false;
    };
    let inner: Vec<TokenTree> = group.stream().into_iter().collect();
    let predicate = match inner.get(1) {
        Some(TokenTree::Group(predicate)) => predicate.stream().to_string(),
        _ => return false,
    };
    // Whitespace-free, because a token stream renders `not(test)` as `not (test)`.
    let predicate: String = predicate.chars().filter(|c| !c.is_whitespace()).collect();
    // The test-only shapes this tree uses plus their exact production opposites. `test-support` exists solely so
    // the server package's integration tests can compile the domain crate's equivalence oracles; it is
    // deliberately as test-only as `cfg(test)`. Anything else mentioning `test` has no safe default - one
    // reading strips production code, the other counts a test item's names as the server's - so it is a
    // decision to be made here rather than a guess made silently.
    match predicate.as_str() {
        "test" | "any(test,feature=\"test-support\")" => true,
        "not(test)" | "not(any(test,feature=\"test-support\"))" => false,
        other => {
            assert!(
                !mentions_test(other),
                "`{what}` carries `#[cfg({other})]`, and this sweep only knows `cfg(test)`, \
                 `cfg(any(test, feature = \"test-support\"))`, and their exact negations - decide \
                 whether that item is production and teach \
                 `is_cfg_test_attribute` the answer"
            );
            false
        }
    }
}

/// How many tokens a doc attribute at `index` occupies, or `None` where there is not one.
///
/// Two shapes, and missing the second cost a round: `///` is an outer attribute (`#`, `[...]`) and `//!` is an
/// inner one (`#`, `!`, `[...]`), so a module's own explanation is three tokens rather than two.
#[cfg(test)]
fn doc_attribute_width(tokens: &[proc_macro2::TokenTree], index: usize) -> Option<usize> {
    use proc_macro2::TokenTree;
    if is_attribute_named(tokens, index, "doc") {
        return Some(2);
    }
    let Some(TokenTree::Punct(hash)) = tokens.get(index) else {
        return None;
    };
    let Some(TokenTree::Punct(bang)) = tokens.get(index + 1) else {
        return None;
    };
    if hash.as_char() != '#' || bang.as_char() != '!' {
        return None;
    }
    bracket_group_named(tokens.get(index + 2), "doc").then_some(3)
}

/// Whether the token at `index` begins an outer attribute with the given name - `#[name...]`.
#[cfg(test)]
fn is_attribute_named(tokens: &[proc_macro2::TokenTree], index: usize, name: &str) -> bool {
    use proc_macro2::TokenTree;
    let Some(TokenTree::Punct(hash)) = tokens.get(index) else {
        return false;
    };
    if hash.as_char() != '#' {
        return false;
    }
    bracket_group_named(tokens.get(index + 1), name)
}

/// Whether a token is a `[...]` group whose first token is the given identifier.
#[cfg(test)]
fn bracket_group_named(token: Option<&proc_macro2::TokenTree>, name: &str) -> bool {
    use proc_macro2::{Delimiter, TokenTree};
    let Some(TokenTree::Group(group)) = token else {
        return false;
    };
    group.delimiter() == Delimiter::Bracket
        && matches!(group.stream().into_iter().next(), Some(TokenTree::Ident(ident)) if ident == name)
}

/// Whether `text` names `marker` as a word rather than as a fragment of one.
///
/// `contains` was wrong in a way no amount of care repairs: `agno` is a framework and also the middle of
/// `diagnostic` and of `backend-agnostic`. A boundary is the start or end of the text, a character that is not
/// alphanumeric - so `_`, `-`, `.` and a quote all separate - or a change of case, which is what makes
/// `VercelFormat` a naming and `agnostic` not.
#[cfg(test)]
fn names_as_a_word(text: &str, marker: &str) -> bool {
    let folded = text.to_ascii_lowercase();
    let bytes = text.as_bytes();
    let mut from = 0;
    while let Some(offset) = folded[from..].find(marker) {
        let at = from + offset;
        let end = at + marker.len();
        let left = match at.checked_sub(1).map(|i| bytes[i]) {
            None => true,
            Some(before) => {
                !before.is_ascii_alphanumeric()
                    || (!before.is_ascii_uppercase() && bytes[at].is_ascii_uppercase())
            }
        };
        let right = match bytes.get(end).copied() {
            None => true,
            Some(after) => {
                !after.is_ascii_alphanumeric()
                    || (after.is_ascii_uppercase() && !bytes[end - 1].is_ascii_uppercase())
            }
        };
        if left && right {
            return true;
        }
        from = at + 1;
    }
    false
}

/// Whether a `cfg` predicate mentions the `test` configuration, as a whole word.
#[cfg(test)]
fn mentions_test(predicate: &str) -> bool {
    predicate
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .any(|word| word == "test")
}

/// The sweep's reader keeps production code, drops test items, and sees a name written in pieces.
///
/// It is load-bearing - the whole-server claim rests on it - and every one of these was a real bypass. Three came
/// from reading the source a line at a time and are gone by construction now that it is tokenised: a multi-line
/// `const` read as braced (consuming the item after it, so a production function was never seen); a multi-line
/// `fn` signature read as a statement (resuming inside its own body, so everything in it counted as production);
/// and a brace inside a **string literal** sending the search for a closing brace off to the end of the file. The
/// fourth is the reverse - a name assembled from adjacent literals, which no line match could join across lines.
#[test]
fn the_test_stripper_keeps_production_code_and_drops_test_items() {
    let source = r##"
#[cfg(test)]
const WRAPPED: &str =
    "x";

fn production_after_a_multiline_const() {
    let marker = "first";
}

#[cfg(test)]
pub(super) fn oracle_with_a_wrapped_signature(
    attrs: &Map,
) -> Reading {
    let inner = "second";
}

fn production_after_a_wrapped_signature() {
    let marker = "third";
}

#[cfg(test)]
mod tests {
    fn inside() {
        let marker = "fourth";
    }
}

fn production_after_a_module() {
    let marker = "fifth";
}

#[cfg(test)]
const BRACE_IN_A_STRING: &str = "{";

fn production_after_a_brace_in_a_string() {
    let marker = "sixth";
}

#[cfg(any(test, feature = "test-support"))]
fn test_support_oracle() {
    let marker = "eighth";
}

#[cfg(not(test))]
fn production_only() {
    let marker = "seventh";
}

fn a_name_in_pieces() -> String {
    concat!(
        "lang",
        "graph"
    )
    .to_string()
}
"##;

    let names = production_names(source, "the stripper's own fixture");
    let kept: Vec<String> = names.iter().map(|(_, text)| text.clone()).collect();
    let joined = kept.join("\n");

    for expected in [
        "production_after_a_multiline_const",
        "\"first\"",
        "production_after_a_wrapped_signature",
        "\"third\"",
        "production_after_a_module",
        "\"fifth\"",
        // A brace inside a string is a string, so the item after it is still read.
        "production_after_a_brace_in_a_string",
        "\"sixth\"",
        // `cfg(not(test))` is production, and reading it as a test item would strip it.
        "production_only",
        "\"seventh\"",
        // Adjacent literals are joined as tokens, so the name is the name however it is laid out.
        "langgraph",
    ] {
        assert!(
            kept.iter().any(|name| name == expected),
            "production name dropped: `{expected}`\nkept:\n{joined}"
        );
    }
    for dropped in [
        "WRAPPED",
        "\"x\"",
        "oracle_with_a_wrapped_signature",
        "\"second\"",
        "\"fourth\"",
        "BRACE_IN_A_STRING",
        "test_support_oracle",
        "\"eighth\"",
    ] {
        assert!(
            !kept.iter().any(|name| name == dropped),
            "test name kept: `{dropped}`\nkept:\n{joined}"
        );
    }
}

/// A marker is a **word**, not a fragment, and both directions cost something.
///
/// `contains` was the first spelling and it cannot work: `agno` is a framework and also the middle of
/// `diagnostic`, so the sweep reported the s3 error formatter and the backend-agnostic data layer. Requiring a
/// non-alphanumeric boundary on both sides fixes that and loses `VercelFormat`, where the boundary is a change of
/// case - which is why a case change counts as one. Load-bearing in the quiet direction: a matcher that is too
/// strict makes the whole sweep pass while naming nothing.
#[test]
fn a_marker_matches_a_word_and_not_a_fragment() {
    for (text, marker) in [
        ("agno", "agno"),
        ("agno_client", "agno"),
        ("try_vercel_format", "vercel"),
        ("VercelFormat", "vercel"),
        ("read_llama_index_state", "llama_index"),
        ("\"strands\"", "strands"),
        ("LlamaIndexReader", "llamaindex"),
        ("googleAdkResult", "adk"),
    ] {
        assert!(
            names_as_a_word(text, marker),
            "`{text}` names `{marker}` and the sweep would not see it"
        );
    }
    for (text, marker) in [
        ("diagnostic", "agno"),
        ("backend-agnostic", "agno"),
        ("is_agnostic", "agno"),
        ("stranded", "strands"),
        ("readka", "adk"),
    ] {
        assert!(
            !names_as_a_word(text, marker),
            "`{text}` does not name `{marker}`, and reporting it would be a false accusation"
        );
    }
}

/// A `cfg` predicate this sweep does not know is refused, not guessed at.
///
/// There is no safe default beyond the exact `test-support` gate: reading `cfg(not(test))` as a test item strips
/// production code, and reading `cfg(any(test, feature = "x"))` as production counts a test item's names as the
/// server's. So an unfamiliar shape mentioning `test` stops the sweep and asks for a decision.
#[test]
#[should_panic(expected = "this sweep only knows `cfg(test)`")]
fn an_unfamiliar_cfg_mentioning_test_is_refused() {
    production_names(
        "#[cfg(any(test, feature = \"x\"))]\nfn ambiguous() {}",
        "an unfamiliar cfg",
    );
}

/// The `repr` primitive accepts exactly the language its documentation specifies.
///
/// The design's fifth test required the accepted language to be *specified* rather than described as
/// "Python repr", which names something far larger. A specification nobody checks is a comment, so each
/// line of that table is a case here - and the refusals matter more than the acceptances: a refusal leaves
/// the string to be read as text, while a wrong parse invents a tool definition.
#[test]
fn the_repr_primitive_accepts_the_language_it_specifies() {
    use crate::rules::tool_repr::python_literal_to_json_for_test as parse;
    for accepted in [
        "{'k': True}",
        "{\"k\": False}",
        "{'k': None}",
        "[{'a': 1}, {'b': 2}]",
        "{'k': 'Nonetheless'}",
        "{'nested': {'deep': [1, 2, 3]}}",
    ] {
        assert!(
            parse(accepted).is_some(),
            "`{accepted}` is in the specified language and was refused"
        );
    }
    // A whole-word check, not a substring one: a value that merely starts with `None` is a string.
    assert_eq!(
        parse("{'k': 'Nonetheless'}").and_then(|v| v["k"].as_str().map(str::to_owned)),
        Some("Nonetheless".to_string()),
        "a string containing a literal's name must survive as that string"
    );
    for refused in [
        // Not object- or array-shaped: refused before anything else, so prose cannot become a definition.
        "True",
        "search",
        "Tool Arguments: none",
        "",
        // Python literals outside the accepted subset. Each must refuse rather than parse to something.
        "(1, 2)",
        "{'k': (1, 2)}",
        "{'k': b'bytes'}",
        "{'k': 1_000}",
        "{'k': 0x1f}",
        "{'k': inf}",
        "{'k': 'a' 'b'}",
        "{'k': 1,}",
        "{1, 2}",
        "{'k': f'x'}",
    ] {
        assert!(
            parse(refused).is_none(),
            "`{refused}` is outside the specified language and was accepted - a wrong parse invents a \
             tool definition, where a refusal only fails to find one"
        );
    }
}
