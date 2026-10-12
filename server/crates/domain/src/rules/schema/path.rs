//! A declared JSONPath, parsed and bounded where the asset loads.
//!
//! RFC 9535 evaluation materialises its node lists, and two of its features multiply them: a union offers
//! each current node once per selector, and a descendant segment visits every node below each current one.
//! `$[0,0][0,0]...` doubles the list per segment - 2^32 nodes from a four-byte payload at thirty-two
//! segments - and no ceiling after the call can bound work the evaluator has already done. So the bound is
//! on the declaration: the product of union arities and the count of descendant segments are capped as the
//! path parses, and a path past either cap is refused as a statement no asset may make.

use serde::Deserialize;
use serde_json::Value as JsonValue;

/// A parsed path whose worst-case fan-out is within the declared ceilings.
#[derive(Debug, Clone)]
pub struct JsonPath {
    parsed: serde_json_path::JsonPath,
    text: String,
}

impl JsonPath {
    /// Parse and bound a declared path.
    pub fn parse(text: &str) -> Result<Self, JsonPathDefect> {
        let parsed = serde_json_path::JsonPath::parse(text)
            .map_err(|error| JsonPathDefect::Parse(error.to_string()))?;
        let (fanout, descendants, root_in_filter) = fanout_of(text);
        if root_in_filter {
            return Err(JsonPathDefect::RootInFilter {
                path: text.to_string(),
            });
        }
        if fanout > sideseat_core::constants::RULE_PATH_MAX_UNION_FANOUT as u128 {
            return Err(JsonPathDefect::UnionFanout {
                path: text.to_string(),
                fanout,
            });
        }
        if descendants > sideseat_core::constants::RULE_PATH_MAX_DESCENDANTS {
            return Err(JsonPathDefect::Descendants {
                path: text.to_string(),
                descendants,
            });
        }
        Ok(Self {
            parsed,
            text: text.to_string(),
        })
    }

    /// Evaluate the path. Bounded because parsing bounded the declaration, not because of anything here.
    pub fn query<'b>(&self, value: &'b JsonValue) -> serde_json_path::NodeList<'b> {
        self.parsed.query(value)
    }
}

/// Two declared paths are the same statement exactly when they are spelled the same.
impl PartialEq for JsonPath {
    fn eq(&self, other: &Self) -> bool {
        self.text == other.text
    }
}

impl Eq for JsonPath {}

impl std::fmt::Display for JsonPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text)
    }
}

impl<'de> Deserialize<'de> for JsonPath {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).map_err(serde::de::Error::custom)
    }
}

/// In the declared schema a path is its text, as it is on the wire.
#[cfg(test)]
impl schemars::JsonSchema for JsonPath {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "JsonPath".into()
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        <String as schemars::JsonSchema>::json_schema(generator)
    }
}

/// Why a declared path was refused.
#[derive(Debug, thiserror::Error)]
pub enum JsonPathDefect {
    #[error("the path does not parse: {0}")]
    Parse(String),
    #[error(
        "path `{path}` multiplies one node into {fanout} through its unions; \
         a declaration past {max} per node is refused",
        max = sideseat_core::constants::RULE_PATH_MAX_UNION_FANOUT
    )]
    UnionFanout { path: String, fanout: u128 },
    #[error(
        "path `{path}` holds {descendants} descendant segments; \
         a declaration past {max} is refused",
        max = sideseat_core::constants::RULE_PATH_MAX_DESCENDANTS
    )]
    Descendants { path: String, descendants: usize },
    #[error(
        "path `{path}` selects from the document root inside a filter - anything but a plain member \
         chain off `$` selects from the whole document again for every node the filter visits; ask about \
         the current node (`@`) instead"
    )]
    RootInFilter { path: String },
}

/// The worst-case per-node fan-out a path's unions declare, and how many descendant segments it holds.
///
/// Counted on the text, conservatively, with a scope stack: every bracket at every depth multiplies by its
/// own arm count - a union nested inside a filter expression multiplies the work the filter does per node
/// exactly as a top-level one multiplies the node list - while a comma whose innermost grouping is a
/// parenthesis separates a function's arguments and is no union arm, and quoted names are names. No shipped
/// asset declares a union or a descendant at all, so what conservatism remains costs nothing real.
fn fanout_of(text: &str) -> (u128, usize, bool) {
    enum Scope {
        Bracket { arms: u128 },
        Paren,
    }
    let mut fanout: u128 = 1;
    let mut descendants = 0usize;
    // A `$` inside a bracket re-queries the whole document per current node. Only a plain member chain off
    // it - `$.a.b`, one lookup per node - is a statement; anything else after it (a bracket segment of any
    // kind, a wildcard, a descendant) selects from the document again for every node the filter visits,
    // which no later ceiling can see. Tracked while the chain lasts.
    let mut root_chain = false;
    let mut expanding_root = false;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    let mut scopes: Vec<Scope> = Vec::new();
    let mut previous = '\0';
    for c in text.chars() {
        if let Some(open) = quote {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == open {
                quote = None;
            }
            previous = c;
            continue;
        }
        if root_chain {
            // RFC 9535's own lexis: blank space may sit between segments (`$ [*]` is a bracket segment),
            // and a member-name shorthand is `_`, ASCII letters and digits, or any non-ASCII character - so
            // the chain lasts through all of those and ends at anything else.
            match c {
                '.' if previous == '.' => expanding_root = true,
                '[' | '*' => expanding_root = true,
                c if c == '.'
                    || c == '_'
                    || c.is_ascii_alphanumeric()
                    || !c.is_ascii()
                    || matches!(c, ' ' | '\t' | '\n' | '\r') => {}
                _ => root_chain = false,
            }
        }
        match c {
            '\'' | '"' => quote = Some(c),
            '[' => scopes.push(Scope::Bracket { arms: 1 }),
            ']' => {
                if let Some(Scope::Bracket { arms }) = scopes.pop() {
                    fanout = fanout.saturating_mul(arms);
                }
            }
            '(' => scopes.push(Scope::Paren),
            ')' => {
                if matches!(scopes.last(), Some(Scope::Paren)) {
                    scopes.pop();
                }
            }
            ',' => {
                if let Some(Scope::Bracket { arms }) = scopes.last_mut() {
                    *arms = arms.saturating_add(1);
                }
            }
            '.' if previous == '.' => descendants += 1,
            '$' if !scopes.is_empty() => root_chain = true,
            _ => {}
        }
        previous = c;
    }
    (fanout, descendants, expanding_root)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exponential shape: each `[0,0]` doubles the node list, which thirty-two segments turn into 2^32
    /// nodes from a four-byte payload. Refused at the declaration, where the only bound can be.
    #[test]
    fn a_path_whose_unions_multiply_past_the_ceiling_is_refused() {
        let doubling = format!("${}", "[0,0]".repeat(32));
        let error = JsonPath::parse(&doubling).expect_err("a 2^32 fan-out is refused");
        assert!(error.to_string().contains("multiplies"), "{error}");
        // Within the ceiling the same construct is a statement: four arms twice is sixteen per node.
        JsonPath::parse("$[0,1,2,3][0,1,2,3]").expect("a bounded union compiles");
    }

    /// Descendant chains revisit every node below each current one, so their count is capped too.
    #[test]
    fn a_path_with_too_many_descendant_segments_is_refused() {
        let error = JsonPath::parse("$..a..b..c").expect_err("three descendants are refused");
        assert!(error.to_string().contains("descendant"), "{error}");
        JsonPath::parse("$..a..b").expect("two descendants compile");
    }

    /// Commas inside quoted names are names, not union arms.
    #[test]
    fn a_quoted_comma_is_a_name_not_a_union() {
        JsonPath::parse("$['a,b']['c,d']['e,f']['g,h']['i,j']['k,l']['m,n']")
            .expect("quoted commas are not arms");
    }
}

#[cfg(test)]
mod scope_tests {
    use super::*;

    /// A union nested inside a filter multiplies the filter's work per node exactly as a top-level one
    /// multiplies the node list, so the accounting is per bracket at every depth.
    #[test]
    fn a_union_nested_in_a_filter_multiplies_like_a_top_level_one() {
        let nested = format!("$[?count(@{}) > 0]", "[0,0]".repeat(32));
        let error = JsonPath::parse(&nested).expect_err("2^32 inside a filter is refused");
        assert!(error.to_string().contains("multiplies"), "{error}");
    }

    /// A comma whose innermost grouping is a parenthesis separates a function's arguments: seven `match`
    /// filters hold seven commas and no union, and the path loads.
    #[test]
    fn a_function_argument_comma_is_not_a_union_arm() {
        let filters = "[?match(@.name, \"a\")].children".repeat(7);
        JsonPath::parse(&format!("${filters}")).expect("function arguments are not unions");
    }
}

#[cfg(test)]
mod filter_root_tests {
    use super::*;

    /// A filter that expands the document root materialises it again per node - `$[?count($[*]) > 0]` is
    /// quadratic before any reading ceiling applies - and is refused; a plain member chain off `$` is one
    /// lookup per node and stays, as the shipped `$[?$.isError == true]` does.
    #[test]
    fn an_expanding_root_subquery_in_a_filter_is_refused() {
        // Any selection off `$` beyond a member chain: a wildcard, a slice (no `*` and still the whole
        // array), an index or slice down a member chain, a descendant, and a root filter nested in a filter.
        for expanding in [
            "$[?count($[*]) > 0]",
            "$[?count($[:]) > 0]",
            "$[?count($.items[0:]) > 0]",
            "$[?count($..x) > 0]",
            "$[?count(@[?count($[?@.x]) > 0]) > 0]",
            "$[?count($ [*]) > 0]",
            "$[?count($.\u{1F600}[*]) > 0]",
            "$[?count($.é.items[*]) > 0]",
        ] {
            let error = JsonPath::parse(expanding).expect_err(expanding);
            assert!(
                error.to_string().contains("selects from the document root"),
                "{expanding}: {error}"
            );
        }
        JsonPath::parse("$[?$.isError == true]")
            .expect("a plain root member lookup is a statement");
        JsonPath::parse("$[?count(@.items[*]) > 0]")
            .expect("expanding the current node is bounded by it");
    }
}
