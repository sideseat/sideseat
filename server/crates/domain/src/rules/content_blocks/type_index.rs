//! Which cases of one chain position can answer a block, chosen by the block's `type`.
//!
//! Most content-block cases are discriminated by a top-level `$.type` `one_of`: such a case holds only for a
//! block whose `type` is one of those strings, so a block stating another need not ask it. The index reads a
//! block's `type` once and hands back the cases that could hold for it - the cases discriminated by that type
//! merged with every case that is not discriminated - **in declaration order**, so the first case that answers
//! is the one the linear walk finds. A case is indexed only where its condition provably requires a `type`
//! among its strings; anything the analysis cannot prove stays among the undiscriminated cases, which every
//! block is asked about.

use std::collections::{BTreeSet, HashMap};

use serde_json::Value as JsonValue;

use crate::rules::expr::{Expr, JsonAtom, JsonExpr, JsonSubjectAtom};
use crate::rules::schema::ContentBlockRule;

/// The cases of one position, by the `type` a block must state for each to hold.
#[derive(Debug, Default)]
pub(super) struct TypeIndex {
    /// For each `type` some case requires: that type's cases and the undiscriminated ones, in declaration order.
    by_type: HashMap<String, Vec<usize>>,
    /// The cases that hold whatever the block's `type`, in declaration order: what a block stating no string
    /// `type`, or one no case requires, is asked.
    untyped: Vec<usize>,
}

impl TypeIndex {
    pub(super) fn build(cases: &[ContentBlockRule]) -> Self {
        let required: Vec<Option<BTreeSet<String>>> = cases
            .iter()
            .map(|case| case.require.expression().and_then(required_types))
            .collect();
        let untyped: Vec<usize> = required
            .iter()
            .enumerate()
            .filter(|(_, types)| types.is_none())
            .map(|(index, _)| index)
            .collect();
        let mut by_type: HashMap<String, Vec<usize>> = HashMap::new();
        for (index, types) in required.iter().enumerate() {
            for name in types.iter().flatten() {
                by_type.entry(name.clone()).or_default().push(index);
            }
        }
        for list in by_type.values_mut() {
            list.extend(&untyped);
            list.sort_unstable();
        }
        Self { by_type, untyped }
    }

    /// The positions, in declaration order, of the cases that can hold for this block.
    pub(super) fn candidates(&self, block: &JsonValue) -> &[usize] {
        block
            .get("type")
            .and_then(JsonValue::as_str)
            .and_then(|name| self.by_type.get(name))
            .map_or(self.untyped.as_slice(), Vec::as_slice)
    }
}

/// The strings a block's top-level `type` must be among for the condition to hold, where the condition says
/// so; `None` where it does not, which leaves the case asked of every block.
///
/// Sound by construction over the three-valued logic, where a condition holds only when it is true: a
/// conjunction holds only if every member does, so any member's requirement is the conjunction's, and two
/// are intersected; a disjunction holds only if some member does, so it requires the union where every member
/// requires something, and nothing otherwise; a negation proves nothing.
fn required_types(condition: &JsonExpr) -> Option<BTreeSet<String>> {
    match condition {
        Expr::Atom(JsonAtom::Some { path, satisfies }) if names_the_type(path) => {
            required_strings(satisfies)
        }
        Expr::Atom(_) | Expr::Not(_) => None,
        Expr::All(group) => group
            .children()
            .iter()
            .filter_map(required_types)
            .reduce(|left, right| &left & &right),
        Expr::Any(group) => group
            .children()
            .iter()
            .map(required_types)
            .collect::<Option<Vec<_>>>()
            .map(|sets| sets.into_iter().flatten().collect()),
    }
}

/// The strings a selected value must be one of for the condition on it to hold, where the condition says so.
fn required_strings(condition: &Expr<JsonSubjectAtom>) -> Option<BTreeSet<String>> {
    match condition {
        Expr::Atom(JsonSubjectAtom::OneOf { values }) => Some(values.iter().cloned().collect()),
        Expr::Atom(JsonSubjectAtom::Equals {
            value: JsonValue::String(value),
        }) => Some(BTreeSet::from([value.clone()])),
        Expr::Atom(_) | Expr::Not(_) => None,
        Expr::All(group) => group
            .children()
            .iter()
            .filter_map(required_strings)
            .reduce(|left, right| &left & &right),
        Expr::Any(group) => group
            .children()
            .iter()
            .map(required_strings)
            .collect::<Option<Vec<_>>>()
            .map(|sets| sets.into_iter().flatten().collect()),
    }
}

/// Whether a path selects exactly the block's own `type` member: the one member the index reads.
fn names_the_type(path: &crate::rules::schema::JsonPath) -> bool {
    matches!(
        path.to_string().as_str(),
        "$.type" | "$['type']" | "$[\"type\"]"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn case(require: serde_json::Value) -> ContentBlockRule {
        let mut case = serde_json::json!({
            "id": "probe",
            "priority": 1,
            "at": "provider_formats",
            "thinking": {"text": "$.text"}
        });
        if !require.is_null() {
            case["where"] = require;
        }
        serde_json::from_value(case).expect("the probe case parses")
    }

    fn required(require: serde_json::Value) -> Option<BTreeSet<String>> {
        case(require).require.expression().and_then(required_types)
    }

    fn set(names: &[&str]) -> Option<BTreeSet<String>> {
        Some(names.iter().map(|name| name.to_string()).collect())
    }

    /// A case is indexed by the `type` strings its condition proves, and by nothing it does not prove.
    #[test]
    fn a_case_is_indexed_only_by_the_types_its_condition_requires() {
        assert_eq!(
            required(serde_json::json!({"path": "$.type", "one_of": ["text", "input_text"]})),
            set(&["input_text", "text"])
        );
        assert_eq!(
            required(serde_json::json!({"path": "$.type", "equals": "image"})),
            set(&["image"])
        );
        assert_eq!(
            required(serde_json::json!({"all": [
                {"path": "$.type", "one_of": ["a", "b"]},
                {"path": "$.text", "kind": "string"}
            ]})),
            set(&["a", "b"]),
            "a conjunction requires what its member does"
        );
        assert_eq!(
            required(serde_json::json!({"any": [
                {"path": "$.type", "one_of": ["a"]},
                {"path": "$.type", "equals": "b"}
            ]})),
            set(&["a", "b"]),
            "a disjunction every branch of which requires a type requires one of them"
        );
        for (why, require) in [
            ("no condition", serde_json::json!(null)),
            (
                "another member",
                serde_json::json!({"path": "$.kind", "one_of": ["a"]}),
            ),
            (
                "a nested type",
                serde_json::json!({"path": "$.part.type", "one_of": ["a"]}),
            ),
            (
                "a negation",
                serde_json::json!({"path": "$.type", "none_of": ["a"]}),
            ),
            (
                "a prefix",
                serde_json::json!({"path": "$.type", "starts_with": "input_"}),
            ),
            (
                "presence alone",
                serde_json::json!({"path": "$.type", "exists": true}),
            ),
            (
                "a number",
                serde_json::json!({"path": "$.type", "equals": 1}),
            ),
            (
                "a disjunction with a branch that requires nothing",
                serde_json::json!({"any": [
                    {"path": "$.type", "one_of": ["a"]},
                    {"path": "$.text", "kind": "string"}
                ]}),
            ),
        ] {
            assert_eq!(required(require), None, "{why}");
        }
    }

    /// A block is asked the cases its `type` can satisfy and every undiscriminated case, in declaration order;
    /// a block with no string `type`, or one no case requires, is asked only the undiscriminated ones.
    #[test]
    fn a_block_is_asked_its_types_cases_and_the_rest_in_declaration_order() {
        let cases = [
            case(serde_json::json!({"path": "$.type", "one_of": ["text"]})),
            case(serde_json::json!({"path": "$.text", "kind": "string"})),
            case(serde_json::json!({"path": "$.type", "one_of": ["image", "text"]})),
            case(serde_json::json!({"path": "$.type", "one_of": ["image"]})),
            case(serde_json::json!(null)),
        ];
        let index = TypeIndex::build(&cases);
        let asked = |block: serde_json::Value| index.candidates(&block).to_vec();
        assert_eq!(asked(serde_json::json!({"type": "text"})), [0, 1, 2, 4]);
        assert_eq!(asked(serde_json::json!({"type": "image"})), [1, 2, 3, 4]);
        assert_eq!(asked(serde_json::json!({"type": "audio"})), [1, 4]);
        assert_eq!(asked(serde_json::json!({"type": 7})), [1, 4]);
        assert_eq!(asked(serde_json::json!({"text": "no type"})), [1, 4]);
        assert_eq!(asked(serde_json::json!("a string block")), [1, 4]);
    }
}
