//! Read-time message projection filters compiled from producer rule assets.
//!
//! Telemetry extraction is intentionally untouched. A matching rule can hide producer bookkeeping
//! from the reconstructed conversation while the raw span and extracted messages remain stored.

use std::collections::{BTreeSet, HashMap};

use crate::observations::{MessageSource, RawMessage};

use super::schema::{MessageProjectionAction, RuleFile};
use super::span_conditions::{self, Readable, SpanAtom, SpanExpr, SpanSubject};

/// Facts available while projecting one stored span's messages.
pub struct MessageProjectionContext<'a> {
    pub scope_name: Option<&'a str>,
    pub scope_version: Option<&'a str>,
    pub span_name: Option<&'a str>,
    pub successful: bool,
    pub messages: &'a [RawMessage],
}

/// Typed projection rules, compiled once with the rest of the embedded ruleset.
#[derive(Debug, Default)]
pub struct MessageProjectionPlan {
    rules: Vec<CompiledProjection>,
}

#[derive(Debug)]
struct CompiledProjection {
    condition: SpanExpr,
    only_attribute_source: String,
    successful_only: bool,
    action: MessageProjectionAction,
}

impl MessageProjectionPlan {
    pub fn compile(files: &[RuleFile]) -> Result<Self, String> {
        let mut ids = BTreeSet::new();
        let mut rules = Vec::new();

        for file in files {
            for rule in &file.message_projections {
                if !ids.insert(rule.id.as_str()) {
                    return Err(format!(
                        "message projection clause id `{}` is declared more than once",
                        rule.id
                    ));
                }
                if rule.id.is_empty() || rule.only_attribute_source.is_empty() {
                    return Err(format!(
                        "message projection clause `{}` contains an empty identifier or attribute source",
                        rule.id
                    ));
                }
                let condition = span_conditions::lower(&rule.condition, Readable::PROJECTION)
                    .map_err(|defect| {
                        format!("message projection clause `{}`: {defect}", rule.id)
                    })?;
                if !requires_a_scope(&condition) {
                    return Err(format!(
                        "message projection clause `{}` does not require one instrumentation scope, so it could \
                         suppress ordinary rows of every producer",
                        rule.id
                    ));
                }
                rules.push(CompiledProjection {
                    condition,
                    only_attribute_source: rule.only_attribute_source.clone(),
                    successful_only: rule.successful_only,
                    action: rule.action,
                });
            }
        }

        Ok(Self { rules })
    }

    /// Whether the matching producer declaration withdraws this row from the SideML conversation.
    pub fn suppresses_messages(&self, context: &MessageProjectionContext<'_>) -> bool {
        // A row's span name may be missing; a projection asks only for a name it has, so a missing one is empty
        // text, which no `starts_with` the compiler accepts holds for.
        let no_attributes = HashMap::new();
        let subject = SpanSubject {
            span_name: context.span_name.unwrap_or(""),
            attrs: &no_attributes,
            scope_name: context.scope_name,
            scope_version: context.scope_version,
            resource: None,
        };
        self.rules.iter().any(|rule| {
            let success_matches = !rule.successful_only || context.successful;
            let source_matches = !context.messages.is_empty()
                && context.messages.iter().all(|message| {
                    matches!(
                        &message.source,
                        MessageSource::Attribute { key, .. } if key == &rule.only_attribute_source
                    )
                });
            span_conditions::holds(&rule.condition, &subject)
                && success_matches
                && source_matches
                && rule.action == MessageProjectionAction::SuppressMessages
        })
    }
}

/// Whether the condition cannot hold without naming one instrumentation scope: the scope test itself, or a
/// conjunction with one among its members.
fn requires_a_scope(condition: &SpanExpr) -> bool {
    use super::expr::Expr;
    let is_scope = |expr: &SpanExpr| matches!(expr, Expr::Atom(SpanAtom::ScopeNameEquals { .. }));
    match condition {
        Expr::All(group) => group.children().iter().any(is_scope),
        other => is_scope(other),
    }
}

/// The retired dimension `scope_version_major_at_least`: the leading digits of the scope's version, as a
/// number. Kept as the oracle `scope_version` replaces it against.
#[cfg(test)]
fn version_major(version: &str) -> Option<u64> {
    let digits = version.bytes().take_while(u8::is_ascii_digit).count();
    (digits > 0)
        .then(|| version[..digits].parse().ok())
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `scope.version` against the major-number dimension it replaced, over the versions a scope reports: equal
    /// on every release, and different, deliberately, where the old reading was not a version comparison - text
    /// after the digits, a leading `v`, an epoch, and the pre-releases of the bound.
    #[test]
    fn a_version_range_answers_as_the_major_number_did_on_every_release() {
        let condition = span_conditions::lower(
            &serde_json::from_value(serde_json::json!({
                "source": "scope.version",
                "version": {"scheme": "pep440", "at_least": "6.dev0", "because": "probe"}
            }))
            .expect("the condition parses"),
            Readable::PROJECTION,
        )
        .expect("the condition lowers");
        let no_attributes = HashMap::new();
        let new = |version: Option<&str>| {
            span_conditions::holds(
                &condition,
                &SpanSubject {
                    span_name: "",
                    attrs: &no_attributes,
                    scope_name: None,
                    scope_version: version,
                    resource: None,
                },
            )
        };
        let old = |version: Option<&str>| {
            version
                .and_then(version_major)
                .is_some_and(|major| major >= 6)
        };
        // `6.dev0` is the first release of the 6 series, so its pre-releases are in it as they were.
        for release in [
            "5.9.1",
            "6",
            "6.0",
            "6.0.0",
            "6.0.0b7",
            "6.0rc1",
            "6.12.3",
            "7.1",
            "10.0.post1",
            "4.28.0",
        ] {
            assert_eq!(new(Some(release)), old(Some(release)), "{release}");
        }
        assert!(!new(None) && !old(None), "no version answers neither");
        for (version, was, is) in [
            ("6garbage", true, false),
            ("v6.0", false, true),
            ("1!5.0", false, true),
        ] {
            assert_eq!(
                (old(Some(version)), new(Some(version))),
                (was, is),
                "{version}"
            );
        }
    }
}
