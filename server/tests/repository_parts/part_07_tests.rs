// Serialised bytes in an order of their own: no production type that is serialised into stored or answered bytes
// holds a hash-ordered collection. The check reads the production Rust as tokens, so a string or a comment never
// passes for code.

/// The production Rust sources: every crate's `src` and the server's own, without the test files beside them.
fn production_rust_sources() -> Vec<(String, String)> {
    let repo = repo_root();
    let mut roots = vec![repo.join("server/src")];
    for entry in std::fs::read_dir(repo.join("server/crates")).expect("the crates directory") {
        roots.push(entry.expect("a crate").path().join("src"));
    }
    let mut sources = Vec::new();
    while let Some(dir) = roots.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();
            if path.is_dir() {
                if !name.ends_with("tests_parts") && name != "tests" {
                    roots.push(path);
                }
                continue;
            }
            if !name.ends_with(".rs") || name.ends_with("tests.rs") || name.contains("_tests") {
                continue;
            }
            let text = std::fs::read_to_string(&path).expect("a source file");
            let relative = path
                .strip_prefix(repo)
                .unwrap_or(&path)
                .display()
                .to_string();
            sources.push((relative, text));
        }
    }
    sources.sort();
    sources
}

/// The collections that iterate - and so serialise - in an order of their own.
const HASH_ORDERED: [&str; 4] = ["HashMap", "HashSet", "DashMap", "DashSet"];

type Tokens = [proc_macro2::TokenTree];

/// An item's attributes, each as the tokens inside its brackets.
type Attributes = [Vec<proc_macro2::TokenTree>];

/// The token trees of `stream`. The lexer has matched every bracket and dropped every comment, so neither a
/// string literal nor a comment can read as code.
fn trees(stream: proc_macro2::TokenStream) -> Vec<proc_macro2::TokenTree> {
    stream.into_iter().collect()
}

/// Every identifier in `tokens`, inside groups too.
fn identifiers(tokens: &Tokens, into: &mut Vec<String>) {
    for token in tokens {
        match token {
            proc_macro2::TokenTree::Ident(ident) => into.push(ident.to_string()),
            proc_macro2::TokenTree::Group(group) => identifiers(&trees(group.stream()), into),
            _ => {}
        }
    }
}

fn is_punct(token: Option<&proc_macro2::TokenTree>, c: char) -> bool {
    matches!(token, Some(proc_macro2::TokenTree::Punct(punct)) if punct.as_char() == c)
}

fn is_group(token: Option<&proc_macro2::TokenTree>, delimiter: proc_macro2::Delimiter) -> bool {
    matches!(token, Some(proc_macro2::TokenTree::Group(group)) if group.delimiter() == delimiter)
}

fn is_ident(token: Option<&proc_macro2::TokenTree>, word: &str) -> bool {
    matches!(token, Some(proc_macro2::TokenTree::Ident(ident)) if ident == word)
}

/// The angle-bracket depth after `tokens[index]`, from `depth` before it. Angle brackets are punctuation to the
/// lexer, so they are counted here, past each `->`.
fn angle_depth(tokens: &Tokens, index: usize, depth: i32) -> i32 {
    let arrow = index > 0 && is_punct(tokens.get(index - 1), '-');
    if is_punct(tokens.get(index), '<') {
        depth + 1
    } else if is_punct(tokens.get(index), '>') && !arrow {
        depth - 1
    } else {
        depth
    }
}

/// `tokens` split at their top-level commas, those between generic arguments not among them.
fn top_level_parts(tokens: &Tokens) -> Vec<&Tokens> {
    let (mut parts, mut depth, mut start) = (Vec::new(), 0i32, 0usize);
    for index in 0..tokens.len() {
        depth = angle_depth(tokens, index, depth);
        if depth == 0 && is_punct(tokens.get(index), ',') {
            parts.push(&tokens[start..index]);
            start = index + 1;
        }
    }
    parts.push(&tokens[start..]);
    parts.retain(|part| !part.is_empty());
    parts
}

/// The index after the generic parameters or arguments opening at `open`.
fn after_generics(tokens: &Tokens, open: usize) -> usize {
    let mut depth = 0;
    for index in open..tokens.len() {
        depth = angle_depth(tokens, index, depth);
        if depth == 0 {
            return index + 1;
        }
    }
    tokens.len()
}

/// The attributes at the front of `tokens`, inner ones too, and the index after them.
fn leading_attributes(tokens: &Tokens) -> (Vec<Vec<proc_macro2::TokenTree>>, usize) {
    let (mut found, mut at) = (Vec::new(), 0);
    while is_punct(tokens.get(at), '#') {
        let bang = usize::from(is_punct(tokens.get(at + 1), '!'));
        let Some(proc_macro2::TokenTree::Group(group)) = tokens.get(at + 1 + bang) else {
            break;
        };
        if group.delimiter() != proc_macro2::Delimiter::Bracket {
            break;
        }
        found.push(trees(group.stream()));
        at += 2 + bang;
    }
    (found, at)
}

/// Whether an attribute is `name(...)` with `word` among the identifiers in its parentheses - at any depth when
/// `nested`, so that `cfg_attr(..., derive(Serialize))` derives too.
fn attribute_names(tokens: &Tokens, name: &str, word: &str, nested: bool) -> bool {
    tokens.iter().enumerate().any(|(index, token)| match token {
        proc_macro2::TokenTree::Ident(ident) if ident == name => match tokens.get(index + 1) {
            Some(proc_macro2::TokenTree::Group(group))
                if group.delimiter() == proc_macro2::Delimiter::Parenthesis =>
            {
                let inner = trees(group.stream());
                if nested {
                    let mut names = Vec::new();
                    identifiers(&inner, &mut names);
                    names.iter().any(|found| found == word)
                } else {
                    inner.iter().any(|token| is_ident(Some(token), word))
                }
            }
            _ => false,
        },
        proc_macro2::TokenTree::Group(group) if nested => {
            attribute_names(&trees(group.stream()), name, word, nested)
        }
        _ => false,
    })
}

/// Whether an attribute compiles its item only for tests: `cfg(test)`, or an `all(...)` that requires it.
fn is_cfg_test(attribute: &Tokens) -> bool {
    fn requires_test(predicate: &Tokens) -> bool {
        match predicate {
            [word] => is_ident(Some(word), "test"),
            [all, proc_macro2::TokenTree::Group(inner)] if is_ident(Some(all), "all") => {
                top_level_parts(&trees(inner.stream()))
                    .into_iter()
                    .any(requires_test)
            }
            _ => false,
        }
    }
    matches!(attribute, [cfg, proc_macro2::TokenTree::Group(predicate)]
        if is_ident(Some(cfg), "cfg") && requires_test(&trees(predicate.stream())))
}

/// Whether a member's attributes leave it out of the serialised form.
fn is_skipped(attributes: &Attributes) -> bool {
    attributes.iter().any(|attribute| {
        attribute_names(attribute, "serde", "skip", false)
            || attribute_names(attribute, "serde", "skip_serializing", false)
    })
}

/// The index after a visibility at `at`, if there is one.
fn after_visibility(tokens: &Tokens, at: usize) -> usize {
    if !is_ident(tokens.get(at), "pub") {
        return at;
    }
    at + 1
        + usize::from(is_group(
            tokens.get(at + 1),
            proc_macro2::Delimiter::Parenthesis,
        ))
}

const ITEM_KEYWORDS: [&str; 9] = [
    "struct", "enum", "union", "type", "use", "mod", "impl", "trait", "fn",
];

/// Each item in `tokens`, and in every block inside one, as `visit(container, attributes, item)` with the item's
/// tokens from after its visibility; each `#[cfg(test)]` item is left out with everything inside it. `container`
/// is the keyword of the item whose block holds this one, so an associated type is not taken for an alias.
fn visit_items(
    tokens: &Tokens,
    container: &str,
    visit: &mut dyn FnMut(&str, &Attributes, &Tokens),
) {
    let mut at = 0;
    while at < tokens.len() {
        let (found, after) = leading_attributes(&tokens[at..]);
        let start = after_visibility(tokens, at + after);
        let keyword = tokens[start..]
            .iter()
            .map_while(|token| match token {
                proc_macro2::TokenTree::Ident(ident) => Some(ident.to_string()),
                _ => None,
            })
            .find(|word| ITEM_KEYWORDS.contains(&word.as_str()))
            .unwrap_or_default();
        // An item ends at its `;` or its block, but not at a block inside generic arguments or parameters - a
        // const argument or default - and a `use` only at its `;`, its braces being a list. In an item's own head
        // angles are counted; in a statement or a field, where `<` may be a comparison, a block ends it unless a
        // `<` or a `,` comes just before it, the places of a const argument. A test-only field ends at its `,`
        // too, so that it leaves out only itself.
        let test_only = found.iter().any(|attribute| is_cfg_test(attribute));
        let mut end = start;
        let mut depth = 0;
        while let Some(token) = tokens.get(end) {
            depth = angle_depth(tokens, end, depth).max(0);
            let after_argument_start = end > start
                && (is_punct(tokens.get(end - 1), '<') || is_punct(tokens.get(end - 1), ','));
            end += 1;
            let block = is_group(Some(token), proc_macro2::Delimiter::Brace);
            let ends = if keyword.is_empty() {
                (block && !after_argument_start)
                    || (test_only && depth == 0 && is_punct(Some(token), ','))
            } else {
                keyword != "use" && block && depth == 0
            };
            if is_punct(Some(token), ';') || ends {
                break;
            }
        }
        at = end.max(at + 1);
        if test_only {
            continue;
        }
        let item = &tokens[start..end];
        visit(container, &found, item);
        visit_blocks(item, &keyword, visit);
    }
}

/// The items in each block in `tokens`, however deep in other groups it is: a closure's body inside a call's
/// arguments is a block too.
fn visit_blocks(
    tokens: &Tokens,
    container: &str,
    visit: &mut dyn FnMut(&str, &Attributes, &Tokens),
) {
    for token in tokens {
        if let proc_macro2::TokenTree::Group(group) = token {
            if group.delimiter() == proc_macro2::Delimiter::Brace {
                visit_items(&trees(group.stream()), container, visit);
            } else {
                visit_blocks(&trees(group.stream()), container, visit);
            }
        }
    }
}

/// Every alias in `tokens` with each name its declaration mentions, its generic defaults included, and each
/// renamed import or re-export (`Original as Alias`) as an alias mentioning `Original`.
fn declared_aliases(tokens: &Tokens, into: &mut Vec<(String, Vec<String>)>) {
    visit_items(tokens, "mod", &mut |container, _, item| {
        if is_ident(item.first(), "type") && !matches!(container, "impl" | "trait") {
            if let Some(proc_macro2::TokenTree::Ident(name)) = item.get(1) {
                let mut mentioned = Vec::new();
                identifiers(&item[2..], &mut mentioned);
                into.push((name.to_string(), mentioned));
            }
        } else if is_ident(item.first(), "use") {
            renamed_imports(item, into);
        }
    });
}

/// `seeds` and every alias that resolves to one of them, directly or through another, to a fixed point. Names are
/// not scoped: an alias reaches every file, which may import it under its own name, and a name one place gives
/// another meaning is still taken for the collection - a loud failure, never a silent pass.
fn resolved(seeds: &[&str], aliases: &[(String, Vec<String>)], whole: bool) -> BTreeSet<String> {
    let mut names: BTreeSet<String> = seeds.iter().map(|name| name.to_string()).collect();
    loop {
        let before = names.len();
        for (alias, mentioned) in aliases {
            // A derive is renamed only as a whole: `use serde::Serialize as Ser`.
            let reaches = if whole {
                mentioned.iter().any(|name| names.contains(name))
            } else {
                mentioned.len() == 1 && names.contains(&mentioned[0])
            };
            if reaches {
                names.insert(alias.clone());
            }
        }
        if names.len() == before {
            return names;
        }
    }
}

/// Each `Original as Alias` in a `use` item, at any depth of its lists, as an alias mentioning `Original`.
fn renamed_imports(tokens: &Tokens, into: &mut Vec<(String, Vec<String>)>) {
    for (index, token) in tokens.iter().enumerate() {
        match token {
            proc_macro2::TokenTree::Ident(original) if is_ident(tokens.get(index + 1), "as") => {
                if let Some(proc_macro2::TokenTree::Ident(alias)) = tokens.get(index + 2) {
                    into.push((alias.to_string(), vec![original.to_string()]));
                }
            }
            proc_macro2::TokenTree::Group(group) => renamed_imports(&trees(group.stream()), into),
            _ => {}
        }
    }
}

/// The first name in `tokens`, inside groups too, that is a hash-ordered collection.
fn held(tokens: &Tokens, names: &BTreeSet<String>) -> Option<String> {
    let mut mentioned = Vec::new();
    identifiers(tokens, &mut mentioned);
    mentioned.into_iter().find(|name| names.contains(name))
}

/// The members of a body - a struct's fields, a tuple's positions, an enum's variants - that are serialised, each
/// with its label and its tokens from after its attributes and visibility.
fn serialised_members(body: &Tokens, tuple: bool) -> Vec<(String, &Tokens)> {
    top_level_parts(body)
        .into_iter()
        .enumerate()
        .filter_map(|(position, member)| {
            let (attributes, after) = leading_attributes(member);
            // Skipped by serde, or compiled only for tests.
            if is_skipped(&attributes) || attributes.iter().any(|attribute| is_cfg_test(attribute))
            {
                return None;
            }
            let member = &member[after_visibility(member, after)..];
            let label = if tuple {
                position.to_string()
            } else {
                member.first()?.to_string()
            };
            Some((label, member))
        })
        .collect()
}

/// The hash-ordered collection a struct's member or an enum's variant holds where it is serialised.
fn held_by(
    member: &Tokens,
    tuple: bool,
    variant: bool,
    names: &BTreeSet<String>,
) -> Option<String> {
    if tuple {
        return held(member, names);
    }
    let rest = &member[1..];
    // A variant's own fields, each skipped or not by its own attributes. Anything else after a variant's name -
    // a discriminant, or what a shift in one ran into - is read whole.
    if variant && let Some(proc_macro2::TokenTree::Group(fields)) = rest.first() {
        let positional = fields.delimiter() == proc_macro2::Delimiter::Parenthesis;
        return serialised_members(&trees(fields.stream()), positional)
            .into_iter()
            .find_map(|(_, field)| held_by(field, positional, false, names))
            .or_else(|| held(&rest[1..], names));
    }
    held(rest, names)
}

/// A type serialised into stored or answered bytes keeps its members in an order of its own. A `HashMap` or
/// `HashSet` serialises in its own iteration order, which differs from one process to the next, so the same
/// answer was different bytes each time: the secret vault's file and the filter options' response were. Any
/// `#[derive(Serialize)]` type in a production crate that holds one - directly, inside `Option` or `Vec`, as a
/// generic default, or through a type alias or a renamed import - is named here with its member, and so is any
/// place a generic serialised type, or axum's `Json`, is given one as an argument.
///
/// The check reads declarations, not types: a map serialised through `serde_json::to_vec`, `json!` or a generic
/// function is beyond it. Within that, it errs towards naming too much, which fails loudly, over too little,
/// which does not: names are not scoped, so an alias that is a hash collection in one file is taken for one in
/// every file, and an alias that mentions one anywhere, a default its users may replace included, is one.
///
/// Known limits, where it reads Rust's syntax by its own rules rather than the compiler's: what a macro
/// generates, or declares inside its own invocation, is not seen; a `#[cfg(test)]` is honoured on items and
/// members, not on statements or expressions; an item is found where it starts a statement, so one that a
/// statement's comparison runs into is read as part of that statement; and a generic serialised type given a
/// collection in a test-only field that follows another field is named all the same.
#[test]
fn no_serialised_type_holds_a_hash_map() {
    let sources = production_rust_sources();
    assert!(
        sources.len() > 200,
        "only {} production sources",
        sources.len()
    );
    let found = serialised_hash_members(&sources);
    assert!(
        found.is_empty(),
        "serialised types holding a hash-ordered collection, which serialise differently in each process - \
         use a BTreeMap or BTreeSet:\n  {}",
        found.join("\n  ")
    );
}

/// The check finds each shape it claims: a member held directly, inside `Option` or `Vec`, through an alias, a
/// chain of them, an alias's default, a renamed import or a renamed re-export from another file, past a local
/// alias of the same name, in an enum, in a tuple struct, in a closure's block, as a generic default, given to a
/// generic serialised type, to `Json` or to an alias or rename of either - after a const argument or a
/// test-only field too - behind generic parameters with a const default, a `where` clause with a const argument
/// and field attributes, under a derive renamed in this file or another, and after a test module with a brace
/// in a string. It leaves alone an unserialised type, a skipped or test-only member or variant field, a comment,
/// a string, and a test module, compound guards included.
#[test]
fn the_serialised_hash_map_check_finds_every_shape() {
    let shapes = r##"
        use std::collections::{BTreeMap, HashSet as Set};
        use std::collections::HashMap as Renamed;
        use serde::Serialize as Ser;
        type Index = std::collections::HashMap<String, u32>;
        type Chained = Index;
        type Spread =
            std::collections::HashSet<
                u32,
            >;
        type Attrs<V = [u8; 16]> = HashMap<String, V>;
        type Pair<A = HashMap<String, u32>, B = HashMap<String, u32>> = (A, B);
        fn local() { type Index = BTreeMap<String, u32>; }
        #[derive(Debug, Serialize)]
        pub struct Direct { pub members: HashMap<String, u32>, pub kept: Vec<u32> }
        #[derive(serde::Serialize, Clone)]
        struct Wrapped { optional: Option<HashSet<String>>, listed: Vec<HashMap<String, u32>> }
        #[derive(Serialize)]
        pub(crate) struct Aliased { index: Index, attrs: Attrs, renamed: Renamed<String, u32>, set: Set<u32>, pair: Pair<u32> }
        #[derive(Serialize)]
        enum Shape { Map(HashMap<String, u32>), Nothing, Kept { #[serde(skip)] cache: HashMap<String, u32>, kept: u32 }, Held { members: HashSet<u32> } }
        #[derive(Serialize)]
        struct Tuple(u32, HashMap<String, u32>, HashSet<u32>);
        #[derive(Serialize)]
        struct Generic<T = (), F: Fn() -> u32 = fn() -> u32> { #[serde(skip)] marker: PhantomData<(T, F)>, #[serde(with = "x")] options: HashMap<String, u32> }
        #[derive(Serialize)]
        struct Defaulted<T = HashMap<String, u32>> { options: T }
        #[derive(Serialize)]
        struct Response<T> { data: T }
        type Reply = Response<HashMap<String, u32>>;
        type Enveloped<T> = Response<T>;
        use axum::Json as Wire;
        async fn handler() -> Json<Vec<HashSet<u32>>> { Json(Vec::new()) }
        fn enveloped() -> Enveloped<HashMap<String, String>> { todo!() }
        fn wire() -> Wire<HashSet<u32>> { todo!() }
        fn routes() { get(|| async { #[derive(Serialize)] struct Inline { attrs: HashMap<String, String> } }); }
        #[derive(Serialize)]
        struct Frame<const N: usize, T> { data: T }
        fn frames() {
            let frame: Frame<{ HEADER + 4 }, HashMap<String, String>> = todo!();
            let empty = count > 0 && Frame::<4, HashSet<u32>>::default().data.is_empty();
        }
        struct PendingReply { #[cfg(test)] counter: usize, body: Response<HashMap<String, String>> }
        #[derive(Serialize)]
        struct TestOnlyField { #[cfg(test)] probe: HashMap<String, u32>, kept: u32 }
        #[derive(Serialize)]
        struct Packet<const N: usize = { 16 }> { values: HashMap<String, u32> }
        #[derive(Serialize)]
        struct Bounded<T> where T: Into<(u32, u32)> + Codec<{ BLOCK + 1 }> { members: HashMap<String, T> }
        #[derive(Ser)]
        struct ViaRenamedDerive { members: HashMap<String, u32> }
        #[derive(Serialize)]
        struct ThroughAliases { chained: Chained, spread: Option<Spread> }
        #[cfg(test)]
        mod early { fn brace() { let _ = "{"; } #[derive(Serialize)] struct Early { members: HashMap<String, u32> } }
        #[derive(Serialize)]
        struct AfterTests { members: Vec<HashMap<String, u32>>, #[serde(skip)] cache: HashMap<String, u32> }
        #[derive(Debug)]
        struct Unserialised { members: HashMap<String, u32> }
        // #[derive(Serialize)] struct Commented { members: HashMap<String, u32> }
        /// #[derive(Serialize)] struct Documented { members: HashMap<String, u32> }
        const QUOTED: &str = "#[derive(Serialize)] struct Quoted { members: HashMap<String, u32> }";
        #[cfg(test)]
        mod tests { #[derive(Serialize)] struct InTests { members: HashMap<String, u32> } }
        #[cfg(all(test, feature = "integration"))]
        mod integration { #[derive(Serialize)] struct Guarded { members: HashMap<String, u32> } }
    "##;
    let reexports = r##"
        pub use std::collections::HashMap as Metadata;
        pub use serde::Serialize as WireSerialize;
    "##;
    let other = r##"
        use crate::reexports::{Metadata, WireSerialize};
        #[derive(Serialize)]
        struct Elsewhere { index: Index, metadata: Metadata<String, String>, kept: BTreeMap<String, u32> }
        #[derive(WireSerialize)]
        struct Wire { options: HashMap<String, u32> }
    "##;
    let found = serialised_hash_members(&[
        ("src/shapes.rs".to_string(), shapes.to_string()),
        ("src/reexports.rs".to_string(), reexports.to_string()),
        ("src/other.rs".to_string(), other.to_string()),
    ]);
    let names: Vec<&str> = found
        .iter()
        .map(|hit| hit.split(" holds").next().unwrap_or_default())
        .collect();
    assert_eq!(
        names,
        vec![
            "src/shapes.rs: Direct.members",
            "src/shapes.rs: Wrapped.optional",
            "src/shapes.rs: Wrapped.listed",
            "src/shapes.rs: Aliased.index",
            "src/shapes.rs: Aliased.attrs",
            "src/shapes.rs: Aliased.renamed",
            "src/shapes.rs: Aliased.set",
            "src/shapes.rs: Aliased.pair",
            "src/shapes.rs: Shape.Map",
            "src/shapes.rs: Shape.Held",
            "src/shapes.rs: Tuple.1",
            "src/shapes.rs: Tuple.2",
            "src/shapes.rs: Generic.options",
            "src/shapes.rs: Defaulted.<generics>",
            "src/shapes.rs: Response<..>",
            "src/shapes.rs: Json<..>",
            "src/shapes.rs: Enveloped<..>",
            "src/shapes.rs: Wire<..>",
            "src/shapes.rs: Inline.attrs",
            "src/shapes.rs: Frame<..>",
            "src/shapes.rs: Frame<..>",
            "src/shapes.rs: Response<..>",
            "src/shapes.rs: Packet.values",
            "src/shapes.rs: Bounded.members",
            "src/shapes.rs: ViaRenamedDerive.members",
            "src/shapes.rs: ThroughAliases.chained",
            "src/shapes.rs: ThroughAliases.spread",
            "src/shapes.rs: AfterTests.members",
            "src/other.rs: Elsewhere.index",
            "src/other.rs: Elsewhere.metadata",
            "src/other.rs: Wire.options",
        ],
        "{found:?}"
    );
}

/// Each place in `tokens`, and in their groups - but not in a block, whose items [`visit_items`] walks itself, so
/// nothing is seen twice - where one of `generic` is given a hash-ordered collection as an argument.
fn instantiations(
    tokens: &Tokens,
    generic: &BTreeSet<String>,
    names: &BTreeSet<String>,
    into: &mut Vec<String>,
) {
    for (index, token) in tokens.iter().enumerate() {
        match token {
            proc_macro2::TokenTree::Ident(ident) if generic.contains(&ident.to_string()) => {
                let mut open = index + 1;
                if is_punct(tokens.get(open), ':') && is_punct(tokens.get(open + 1), ':') {
                    open += 2;
                }
                if is_punct(tokens.get(open), '<') {
                    let end = after_generics(tokens, open);
                    let arguments = &tokens[open + 1..end.saturating_sub(1).max(open + 1)];
                    if let Some(collection) = held(arguments, names) {
                        into.push(format!("{ident}<..> holds a {collection}"));
                    }
                }
            }
            proc_macro2::TokenTree::Group(group)
                if group.delimiter() != proc_macro2::Delimiter::Brace =>
            {
                instantiations(&trees(group.stream()), generic, names, into);
            }
            _ => {}
        }
    }
}

/// Every member of a `#[derive(Serialize)]` type in `sources` that holds a hash map or set - directly, inside
/// another type, as a generic default, or through a type alias or a renamed import - as `path: Type.member holds
/// a Collection`, and every place a generic serialised type, or axum's `Json`, is given one, as `path: Type<..>
/// holds a Collection`.
fn serialised_hash_members(sources: &[(String, String)]) -> Vec<String> {
    let files: Vec<(&str, Vec<proc_macro2::TokenTree>)> = sources
        .iter()
        .map(|(path, text)| {
            let stream: proc_macro2::TokenStream = text
                .parse()
                .unwrap_or_else(|error| panic!("{path} does not lex: {error:?}"));
            (path.as_str(), trees(stream))
        })
        .collect();
    let mut aliases = Vec::new();
    for (_, tokens) in &files {
        declared_aliases(tokens, &mut aliases);
    }
    let names = resolved(&HASH_ORDERED, &aliases, true);
    let derives = resolved(&["Serialize"], &aliases, false);
    let serialised = |attributes: &Attributes| {
        attributes.iter().any(|attribute| {
            derives
                .iter()
                .any(|derive| attribute_names(attribute, "derive", derive, true))
        })
    };
    // The serialised types with generic parameters, whose arguments are serialised wherever they are given, and
    // every alias and rename of one.
    let mut generic = vec!["Json".to_string()];
    for (_, tokens) in &files {
        visit_items(tokens, "mod", &mut |_, attributes, item| {
            if serialised(attributes)
                && (is_ident(item.first(), "struct") || is_ident(item.first(), "enum"))
                && is_punct(item.get(2), '<')
                && let Some(name) = item.get(1)
            {
                generic.push(name.to_string());
            }
        });
    }
    let generic: Vec<&str> = generic.iter().map(String::as_str).collect();
    let generic = resolved(&generic, &aliases, true);
    let mut found = Vec::new();
    for (path, tokens) in &files {
        visit_items(tokens, "mod", &mut |_, attributes, item| {
            // A declaration's own name is not an instantiation of it.
            let declares = ["struct", "enum", "union", "type", "trait", "fn"]
                .iter()
                .any(|keyword| is_ident(item.first(), keyword));
            let from = if declares { 2.min(item.len()) } else { 0 };
            let mut given = Vec::new();
            instantiations(&item[from..], &generic, &names, &mut given);
            found.extend(given.into_iter().map(|hit| format!("{path}: {hit}")));
            let is_struct = is_ident(item.first(), "struct");
            let is_enum = is_ident(item.first(), "enum");
            if !serialised(attributes) || !(is_struct || is_enum) {
                return;
            }
            let Some(proc_macro2::TokenTree::Ident(name)) = item.get(1) else {
                return;
            };
            // The body: a tuple struct's parentheses follow its generics; any other body is the item's last
            // token, its block, past generics and a `where` clause that may hold groups of their own.
            let mut at = 2;
            if is_punct(item.get(at), '<') {
                at = after_generics(item, at);
            }
            let tuple = is_struct && is_group(item.get(at), proc_macro2::Delimiter::Parenthesis);
            let (body_at, delimiter) = if tuple {
                (at, proc_macro2::Delimiter::Parenthesis)
            } else {
                (item.len().saturating_sub(1), proc_macro2::Delimiter::Brace)
            };
            let Some(proc_macro2::TokenTree::Group(body)) = item.get(body_at) else {
                return;
            };
            if body.delimiter() != delimiter {
                return;
            }
            // Generics and a `where` clause: a default or a bound that names a collection.
            let head: Vec<proc_macro2::TokenTree> = item[2..body_at]
                .iter()
                .chain(&item[(body_at + 1).min(item.len())..])
                .cloned()
                .collect();
            if let Some(collection) = held(&head, &names) {
                found.push(format!("{path}: {name}.<generics> holds a {collection}"));
            }
            let body = trees(body.stream());
            for (label, member) in serialised_members(&body, tuple) {
                if let Some(collection) = held_by(member, tuple, is_enum, &names) {
                    found.push(format!("{path}: {name}.{label} holds a {collection}"));
                }
            }
        });
    }
    found
}
