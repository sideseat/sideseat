//! Read-time message projection filters compiled from producer rule assets.
//!
//! Telemetry extraction is intentionally untouched. A matching rule can hide producer bookkeeping
//! from the reconstructed conversation while the raw span and extracted messages remain stored.

use std::collections::BTreeSet;

use crate::observations::{MessageSource, RawMessage};

use super::schema::{MessageProjectionAction, MessageProjectionRule, RuleFile};

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
    rules: Vec<MessageProjectionRule>,
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
                let spec = &rule.match_spec;
                if rule.id.is_empty()
                    || spec.scope_name.is_empty()
                    || spec.span_name_prefix.is_empty()
                    || spec.only_attribute_source.is_empty()
                {
                    return Err(format!(
                        "message projection clause `{}` contains an empty identifier or matcher",
                        rule.id
                    ));
                }
                rules.push(rule.clone());
            }
        }

        Ok(Self { rules })
    }

    /// Whether the matching producer declaration withdraws this row from the SideML conversation.
    pub fn suppresses_messages(&self, context: &MessageProjectionContext<'_>) -> bool {
        self.rules.iter().any(|rule| {
            let spec = &rule.match_spec;
            let scope_matches = context.scope_name == Some(spec.scope_name.as_str());
            let version_matches = context
                .scope_version
                .and_then(version_major)
                .is_some_and(|major| major >= spec.scope_version_major_at_least);
            let name_matches = context
                .span_name
                .is_some_and(|name| name.starts_with(&spec.span_name_prefix));
            let success_matches = !spec.successful_only || context.successful;
            let source_matches = !context.messages.is_empty()
                && context.messages.iter().all(|message| {
                    matches!(
                        &message.source,
                        MessageSource::Attribute { key, .. }
                            if key == &spec.only_attribute_source
                    )
                });

            scope_matches
                && version_matches
                && name_matches
                && success_matches
                && source_matches
                && rule.action == MessageProjectionAction::SuppressMessages
        })
    }
}

fn version_major(version: &str) -> Option<u64> {
    let digits = version.bytes().take_while(u8::is_ascii_digit).count();
    (digits > 0)
        .then(|| version[..digits].parse().ok())
        .flatten()
}
