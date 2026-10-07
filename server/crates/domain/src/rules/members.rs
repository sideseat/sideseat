//! Producer member names and their meanings.
//!
//! Three questions about one vocabulary. Which member holds a message's **content** is ordered, because a value
//! carrying two of them has one answer and the order is what picks it. The other two are sets: whether a
//! member's presence means the value is *message-shaped* (so it is not bare structured output to be wrapped),
//! and whether it means the value is a content **block** (so one no case recognised is malformed rather than
//! plain data).
//!
//! One section rather than three lists, because a member usually answers more than one question and as three
//! lists in Rust they drifted: `contents` held content and said "message-shaped", while a dialect's own call-id
//! member said "content block" only.

use std::collections::BTreeSet;

#[derive(Debug, thiserror::Error)]
pub enum MemberCompileError {
    #[error("member rule `{rule}` in `{file}` names an empty member")]
    EmptyMember { file: String, rule: String },
    #[error("member rule `{rule}` in `{file}` names no member at all")]
    NoMembers { file: String, rule: String },
    #[error(
        "member rule `{rule}` in `{file}` holds a message's content without meaning the value is message-shaped - so one reader would take that member as the content while another read the object around it as bare data"
    )]
    ContentWithoutShape { file: String, rule: String },
    #[error("member rule `{rule}` in `{file}` says nothing about its member")]
    SaysNothing { file: String, rule: String },
    #[error(
        "member rule `{rule}` in `{file}` answers an ordered question and states no rank, so its position among the others is undeclared"
    )]
    ContentWithoutARank { file: String, rule: String },
    #[error(
        "member rule `{rule}` in `{file}` states a rank without answering an ordered question, which orders nothing"
    )]
    RankWithoutContent { file: String, rule: String },
    #[error(
        "member rule `{rule}` in `{file}` answers two ordered questions with one rank, so its position in one of them is a coincidence"
    )]
    TwoOrderedQuestions { file: String, rule: String },
    #[error(
        "member rules `{first}` and `{second}` share content rank {rank}, so which holds a value's content depends on load order"
    )]
    SharedRank {
        first: String,
        second: String,
        rank: i32,
    },
    #[error(
        "member rule `{rule}` in `{file}` spells `{target}`, which is no member of SideSeat's a spelling may stand in for"
    )]
    UnknownAliasTarget {
        file: String,
        rule: String,
        target: String,
    },
    #[error("member `{member}` is declared twice, by `{first}` and `{second}`")]
    DuplicateMember {
        member: String,
        first: String,
        second: String,
    },
}

/// The compiled vocabulary.
#[derive(Debug, Default)]
pub struct MemberPlan {
    content_in_order: Vec<String>,
    message_shaped: BTreeSet<String>,
    content_block: BTreeSet<String>,
    tool_call: BTreeSet<String>,
    tool_result: BTreeSet<String>,
    tool_calls: BTreeSet<String>,
    /// The ordered questions below each pick one member, so each is in rank order - never in the order the
    /// assets happen to load.
    bundled_tool_result: Vec<String>,
    result_call_id_in_order: Vec<String>,
    streamed_reply: Vec<String>,
    detached_system: Vec<String>,
    structured_value_wrapper: Vec<String>,
    control_block: BTreeSet<String>,
    aliases: std::collections::BTreeMap<String, Vec<String>>,
    tool_call_wrapper: Vec<String>,
    context: Vec<(String, ContextRead)>,
    media_bytes: BTreeSet<String>,
    prose: BTreeSet<String>,
    producer_shape: BTreeSet<String>,
}

/// How a context member is read.
#[derive(Debug, Clone)]
pub enum ContextRead {
    /// The member's value is one context of this kind.
    Whole(String),
    /// The member is an object: these of its members under their own kinds, the rest together.
    Parts {
        parts: std::collections::BTreeMap<String, String>,
        rest: String,
    },
}

/// The SideSeat members a producer's spelling may stand in for. A spelling naming any other member would
/// never be read, so it is refused rather than accepted as a statement about nothing.
const ALIAS_TARGETS: &[&str] = &["arguments", "finish_reason", "id", "stop", "tool_calls"];

/// The ordered questions a member rule may answer, each with its own ranking.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Ordered {
    Content,
    ResultCallId,
    BundledToolResult,
    StreamedReply,
    DetachedSystem,
    StructuredValueWrapper,
    ToolCallWrapper,
    Context,
    /// The spellings of one SideSeat member, each target its own ranking.
    Alias(String),
}

impl MemberPlan {
    /// The members that hold a message's content, in the order they are preferred.
    pub fn content_in_order(&self) -> impl Iterator<Item = &str> {
        self.content_in_order.iter().map(String::as_str)
    }

    /// Whether any member of this object means the value is message-shaped.
    pub fn any_means_message_shaped<'a>(&self, members: impl Iterator<Item = &'a String>) -> bool {
        members.into_iter().any(|m| self.message_shaped.contains(m))
    }

    /// Whether any member of this object means the value is a content block.
    pub fn any_means_content_block<'a>(&self, members: impl Iterator<Item = &'a String>) -> bool {
        members.into_iter().any(|m| self.content_block.contains(m))
    }

    /// Every member that means the value is message-shaped, for a test that checks the vocabulary as a set.
    pub fn message_shaped_members(&self) -> impl Iterator<Item = &str> {
        self.message_shaped.iter().map(String::as_str)
    }

    /// Every member that means the value is a content block, for a test that checks the vocabulary as a set.
    pub fn content_block_members(&self) -> impl Iterator<Item = &str> {
        self.content_block.iter().map(String::as_str)
    }

    /// Whether any member of this block means it is a tool call.
    pub fn any_means_tool_call<'a>(&self, members: impl Iterator<Item = &'a String>) -> bool {
        members.into_iter().any(|m| self.tool_call.contains(m))
    }

    /// Every member whose presence on a block means a tool call, in a fixed order.
    pub fn tool_call_members(&self) -> impl Iterator<Item = &str> {
        self.tool_call.iter().map(String::as_str)
    }

    /// Whether any member of this block means it is a tool result.
    pub fn any_means_tool_result<'a>(&self, members: impl Iterator<Item = &'a String>) -> bool {
        members.into_iter().any(|m| self.tool_result.contains(m))
    }

    /// Whether any member of this message holds its tool calls.
    pub fn any_holds_tool_calls<'a>(&self, members: impl Iterator<Item = &'a String>) -> bool {
        members.into_iter().any(|m| self.tool_calls.contains(m))
    }

    /// The members holding one result of a bundle, in the order they are preferred.
    pub fn bundled_tool_result(&self) -> impl Iterator<Item = &str> {
        self.bundled_tool_result.iter().map(String::as_str)
    }

    /// The members holding a bundled result's call id, in the order they are preferred.
    pub fn result_call_id_in_order(&self) -> impl Iterator<Item = &str> {
        self.result_call_id_in_order.iter().map(String::as_str)
    }

    /// The members holding a streamed response's combined text, in the order they are preferred.
    pub fn streamed_reply(&self) -> impl Iterator<Item = &str> {
        self.streamed_reply.iter().map(String::as_str)
    }

    /// Whether a structured-data block holding exactly this member is a provider control instruction.
    pub fn marks_control_block(&self, member: &str) -> bool {
        self.control_block.contains(member)
    }

    /// The other spellings of one of SideSeat's own members, in the order they are preferred.
    pub fn aliases_of(&self, member: &str) -> impl Iterator<Item = &str> {
        self.aliases
            .get(member)
            .into_iter()
            .flatten()
            .map(String::as_str)
    }

    /// The members an entry of a message's tool calls holds its call under, in the order they are preferred.
    pub fn tool_call_wrapper(&self) -> impl Iterator<Item = &str> {
        self.tool_call_wrapper.iter().map(String::as_str)
    }

    /// The members holding context beside a message's content, in the order the contexts are shown.
    pub fn context(&self) -> impl Iterator<Item = (&str, &ContextRead)> {
        self.context
            .iter()
            .map(|(member, read)| (member.as_str(), read))
    }

    /// Whether this member may hold inline media bytes.
    pub fn may_hold_media_bytes(&self, member: &str) -> bool {
        self.media_bytes.contains(member)
    }

    /// Whether any member of this block marks it as a producer's own shape.
    pub fn any_marks_producer_shape<'a>(&self, members: impl Iterator<Item = &'a String>) -> bool {
        members.into_iter().any(|m| self.producer_shape.contains(m))
    }

    /// Whether this member holds prose.
    pub fn holds_prose(&self, member: &str) -> bool {
        self.prose.contains(member)
    }

    /// Every member that may hold media bytes, for a test that checks the vocabulary as a set.
    pub fn media_byte_members(&self) -> impl Iterator<Item = &str> {
        self.media_bytes.iter().map(String::as_str)
    }

    /// Every member that holds prose, for a test that checks the vocabulary as a set.
    pub fn prose_members(&self) -> impl Iterator<Item = &str> {
        self.prose.iter().map(String::as_str)
    }

    /// The members whose object is a wrapper around the value under them, in the order they are preferred.
    pub fn structured_value_wrapper(&self) -> impl Iterator<Item = &str> {
        self.structured_value_wrapper.iter().map(String::as_str)
    }

    /// The members holding a system prompt beside a message array, in the order they are preferred.
    pub fn detached_system(&self) -> impl Iterator<Item = &str> {
        self.detached_system.iter().map(String::as_str)
    }

    pub fn rule_count(&self) -> usize {
        self.message_shaped.len()
            + self.content_block.len()
            + self.content_in_order.len()
            + self.tool_call.len()
            + self.tool_result.len()
            + self.tool_calls.len()
            + self.bundled_tool_result.len()
            + self.result_call_id_in_order.len()
            + self.streamed_reply.len()
            + self.detached_system.len()
            + self.structured_value_wrapper.len()
            + self.control_block.len()
            + self.aliases.values().map(Vec::len).sum::<usize>()
            + self.tool_call_wrapper.len()
            + self.context.len()
            + self.media_bytes.len()
            + self.prose.len()
            + self.producer_shape.len()
    }
}

pub fn compile(assets: &super::assets::ParsedAssets) -> Result<MemberPlan, MemberCompileError> {
    let mut plan = MemberPlan::default();
    let mut ranked: std::collections::BTreeMap<Ordered, Vec<(i32, usize, String, String)>> =
        Default::default();
    let mut context_rules: std::collections::HashMap<String, ContextRead> = Default::default();
    // One declaration per member name, across every asset: two would make "what does this member mean" a
    // question with two answers, resolved by load order.
    let mut by_member: std::collections::HashMap<String, String> = std::collections::HashMap::new();

    for (file_id, file) in assets.iter() {
        let file_id = &file_id.to_owned();
        for rule in &file.message_members {
            if rule.members.is_empty() {
                return Err(MemberCompileError::NoMembers {
                    file: file_id.clone(),
                    rule: rule.id.clone(),
                });
            }
            if rule.members.iter().any(String::is_empty) {
                return Err(MemberCompileError::EmptyMember {
                    file: file_id.clone(),
                    rule: rule.id.clone(),
                });
            }
            let says_something = rule.holds_content
                || rule.means_message_shaped
                || rule.means_content_block
                || rule.means_tool_call
                || rule.means_tool_result
                || rule.holds_tool_calls
                || rule.holds_bundled_tool_result
                || rule.holds_result_call_id
                || rule.holds_streamed_reply
                || rule.holds_detached_system
                || rule.marks_control_block
                || rule.wraps_structured_value
                || rule.alias_of.is_some()
                || rule.wraps_tool_call
                || rule.holds_context.is_some()
                || rule.holds_context_parts.is_some()
                || rule.may_hold_media_bytes
                || rule.holds_prose
                || rule.marks_producer_shape;
            if !says_something {
                return Err(MemberCompileError::SaysNothing {
                    file: file_id.clone(),
                    rule: rule.id.clone(),
                });
            }
            let questions: Vec<Ordered> = [
                (rule.holds_content, Ordered::Content),
                (rule.holds_result_call_id, Ordered::ResultCallId),
                (rule.holds_bundled_tool_result, Ordered::BundledToolResult),
                (rule.holds_streamed_reply, Ordered::StreamedReply),
                (rule.holds_detached_system, Ordered::DetachedSystem),
                (rule.wraps_structured_value, Ordered::StructuredValueWrapper),
                (rule.wraps_tool_call, Ordered::ToolCallWrapper),
                (
                    rule.holds_context.is_some() || rule.holds_context_parts.is_some(),
                    Ordered::Context,
                ),
            ]
            .into_iter()
            .filter_map(|(answers, question)| answers.then_some(question))
            .chain(rule.alias_of.clone().map(Ordered::Alias))
            .collect();
            if rule.holds_context.is_some() && rule.holds_context_parts.is_some() {
                return Err(MemberCompileError::TwoOrderedQuestions {
                    file: file_id.clone(),
                    rule: rule.id.clone(),
                });
            }
            if let Some(target) = &rule.alias_of
                && !ALIAS_TARGETS.contains(&target.as_str())
            {
                return Err(MemberCompileError::UnknownAliasTarget {
                    file: file_id.clone(),
                    rule: rule.id.clone(),
                    target: target.clone(),
                });
            }
            if let Some(kind) = &rule.holds_context {
                context_rules.insert(rule.id.clone(), ContextRead::Whole(kind.clone()));
            }
            if let Some(spec) = &rule.holds_context_parts {
                context_rules.insert(
                    rule.id.clone(),
                    ContextRead::Parts {
                        parts: spec.parts.clone(),
                        rest: spec.rest.clone(),
                    },
                );
            }
            if questions.len() > 1 {
                return Err(MemberCompileError::TwoOrderedQuestions {
                    file: file_id.clone(),
                    rule: rule.id.clone(),
                });
            }
            let ordered = !questions.is_empty();
            if ordered && rule.rank.is_none() {
                return Err(MemberCompileError::ContentWithoutARank {
                    file: file_id.clone(),
                    rule: rule.id.clone(),
                });
            }
            if !ordered && rule.rank.is_some() {
                return Err(MemberCompileError::RankWithoutContent {
                    file: file_id.clone(),
                    rule: rule.id.clone(),
                });
            }
            // Holding a message's content proves the object around it is a message: the member *is* the turn's
            // content, so a reader taking it while another reads the enclosing object as bare structured output
            // would be two answers about one value.
            if rule.holds_content && !rule.means_message_shaped {
                return Err(MemberCompileError::ContentWithoutShape {
                    file: file_id.clone(),
                    rule: rule.id.clone(),
                });
            }
            for member in &rule.members {
                if let Some(first) = by_member.get(member) {
                    return Err(MemberCompileError::DuplicateMember {
                        member: member.clone(),
                        first: first.clone(),
                        second: rule.id.clone(),
                    });
                }
                by_member.insert(member.clone(), rule.id.clone());

                if rule.means_message_shaped {
                    plan.message_shaped.insert(member.clone());
                }
                if rule.means_content_block {
                    plan.content_block.insert(member.clone());
                }
                if rule.means_tool_call {
                    plan.tool_call.insert(member.clone());
                }
                if rule.means_tool_result {
                    plan.tool_result.insert(member.clone());
                }
                if rule.holds_tool_calls {
                    plan.tool_calls.insert(member.clone());
                }
                if rule.marks_control_block {
                    plan.control_block.insert(member.clone());
                }
                if rule.may_hold_media_bytes {
                    plan.media_bytes.insert(member.clone());
                }
                if rule.holds_prose {
                    plan.prose.insert(member.clone());
                }
                if rule.marks_producer_shape {
                    plan.producer_shape.insert(member.clone());
                }
            }
            // One rank per declaration, so a family's spellings sit together in declaration order - deterministic
            // where two aliases somehow appear on one value, which is pathological but must still have an answer.
            if let (Some(rank), Some(question)) = (rule.rank, questions.first()) {
                let into = ranked.entry(question.clone()).or_default();
                for (offset, member) in rule.members.iter().enumerate() {
                    into.push((rank, offset, member.clone(), rule.id.clone()));
                }
            }
        }
    }

    for (question, mut list) in ranked {
        list.sort_by_key(|(rank, offset, _, _)| (*rank, *offset));
        for pair in list.windows(2) {
            // Two *declarations* sharing a rank is the refusal; a family's own spellings share one by
            // construction.
            if pair[0].0 == pair[1].0 && pair[0].3 != pair[1].3 {
                return Err(MemberCompileError::SharedRank {
                    first: pair[0].3.clone(),
                    second: pair[1].3.clone(),
                    rank: pair[0].0,
                });
            }
        }
        if question == Ordered::Context {
            for (_, _, member, rule_id) in list {
                let rule = context_rules
                    .get(&rule_id)
                    .expect("every context entry was recorded");
                plan.context.push((member, rule.clone()));
            }
            continue;
        }
        let members = list.into_iter().map(|(_, _, member, _)| member).collect();
        match question {
            Ordered::Content => plan.content_in_order = members,
            Ordered::ResultCallId => plan.result_call_id_in_order = members,
            Ordered::BundledToolResult => plan.bundled_tool_result = members,
            Ordered::StreamedReply => plan.streamed_reply = members,
            Ordered::DetachedSystem => plan.detached_system = members,
            Ordered::StructuredValueWrapper => plan.structured_value_wrapper = members,
            Ordered::ToolCallWrapper => plan.tool_call_wrapper = members,
            Ordered::Alias(target) => {
                plan.aliases.insert(target, members);
            }
            Ordered::Context => unreachable!("read above"),
        }
    }
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One asset holding exactly these member rules.
    fn probe(rules: &str) -> crate::rules::assets::ParsedAssets {
        let asset = format!(r#"{{"id":"probe","message_members":[{rules}]}}"#);
        let sources = std::collections::BTreeMap::from([("probe".to_string(), asset.into_bytes())]);
        crate::rules::assets::ParsedAssets::parse(&sources).expect("the probe assets parse")
    }

    fn compile_rules(rules: &str) -> Result<MemberPlan, MemberCompileError> {
        compile(&probe(rules))
    }

    /// A spelling family is one declaration with one flag vector, and every spelling in it answers alike.
    ///
    /// This is what the merged section could not express: as separate declarations the aliases drifted, and the
    /// compiler had no way to see it because two names carrying different flags is exactly what a vocabulary of
    /// distinct members looks like.
    /// One rank cannot order two questions: its position in one of them would be a coincidence.
    #[test]
    fn a_member_answering_two_ordered_questions_is_refused() {
        let refused = compile_rules(
            r#"{"id":"p.both","members":["x"],"rank":1,"holds_content":true,"means_message_shaped":true,"holds_result_call_id":true}"#,
        );
        assert!(
            matches!(refused, Err(MemberCompileError::TwoOrderedQuestions { .. })),
            "{refused:?}"
        );
        // Each question has its own ranking, so one rank in two of them is not a shared-rank collision.
        assert!(
            compile_rules(
                r#"{"id":"p.a","members":["a"],"rank":1,"holds_content":true,"means_message_shaped":true},
                   {"id":"p.b","members":["b"],"rank":1,"holds_result_call_id":true}"#,
            )
            .is_ok()
        );
    }

    /// A spelling of a member no reader asks for would never be read.
    #[test]
    fn a_spelling_of_no_member_a_reader_asks_for_is_refused() {
        let refused =
            compile_rules(r#"{"id":"p.alias","members":["x"],"rank":1,"alias_of":"nothing"}"#);
        assert!(
            matches!(refused, Err(MemberCompileError::UnknownAliasTarget { .. })),
            "{refused:?}"
        );
        assert!(
            compile_rules(r#"{"id":"p.alias","members":["x"],"rank":1,"alias_of":"stop"}"#).is_ok()
        );
    }

    #[test]
    fn a_spelling_family_shares_one_flag_vector() {
        let plan = compile_rules(
            r#"{"id":"p.pair","members":["function_call","functionCall"],"means_content_block":true}"#,
        )
        .expect("a family of two spellings compiles");

        for spelling in ["function_call", "functionCall"] {
            assert!(
                plan.any_means_content_block([spelling.to_string()].iter()),
                "{spelling} should be a content block, since its family says so"
            );
            assert!(
                !plan.any_means_message_shaped([spelling.to_string()].iter()),
                "{spelling} should not be message-shaped, since its family does not say so"
            );
        }
    }

    /// A family's spellings share its content rank, and that is not the shared-rank refusal.
    ///
    /// The refusal is about two *declarations* competing, where load order would decide; a family's own spellings
    /// are aliases and cannot compete. They still need a deterministic order for the pathological value carrying
    /// two of them at once, and declaration order is it.
    #[test]
    fn a_family_shares_its_rank_and_keeps_declaration_order() {
        let plan = compile_rules(
            r#"{"id":"p.a","members":["content","contents"],"rank":10,"holds_content":true,"means_message_shaped":true},
               {"id":"p.b","members":["parts"],"rank":20,"holds_content":true,"means_message_shaped":true}"#,
        )
        .expect("one rank per declaration, shared by its spellings");
        assert_eq!(
            plan.content_in_order().collect::<Vec<_>>(),
            vec!["content", "contents", "parts"]
        );
    }

    /// Holding a message's content proves the object around it is a message.
    ///
    /// Otherwise one reader takes that member as the turn's content while another reads the enclosing object as
    /// bare structured output to be wrapped - two answers about one value, from one declaration.
    #[test]
    fn content_without_message_shape_is_refused() {
        let error =
            compile_rules(r#"{"id":"p.c","members":["content"],"rank":10,"holds_content":true}"#)
                .expect_err("holding content without meaning message-shaped must be refused");
        assert!(
            matches!(error, MemberCompileError::ContentWithoutShape { .. }),
            "wrong refusal: {error}"
        );
    }

    /// Every other refusal, each fired, because none of them had ever been.
    ///
    /// A refusal nobody has exercised is a claim rather than a guard - and two of these are one edit away from
    /// being unreachable: `SaysNothing` needs all three flags absent, and `RankWithoutContent` is the mirror of
    /// the refusal above, so an implementation that dropped either would still compile every shipped asset.
    #[test]
    fn every_member_refusal_fires() {
        /// The declarations to compile, and which refusal they must produce.
        type Case = (&'static str, fn(&MemberCompileError) -> bool);
        let cases: Vec<Case> = vec![
            (r#"{"id":"p","members":[]}"#, |e| {
                matches!(e, MemberCompileError::NoMembers { .. })
            }),
            (
                r#"{"id":"p","members":[""],"means_content_block":true}"#,
                |e| matches!(e, MemberCompileError::EmptyMember { .. }),
            ),
            (r#"{"id":"p","members":["x"]}"#, |e| {
                matches!(e, MemberCompileError::SaysNothing { .. })
            }),
            (
                r#"{"id":"p","members":["x"],"holds_content":true,"means_message_shaped":true}"#,
                |e| matches!(e, MemberCompileError::ContentWithoutARank { .. }),
            ),
            (
                r#"{"id":"p","members":["x"],"rank":10,"means_content_block":true}"#,
                |e| matches!(e, MemberCompileError::RankWithoutContent { .. }),
            ),
            (
                r#"{"id":"a","members":["x"],"rank":10,"holds_content":true,"means_message_shaped":true},
                   {"id":"b","members":["y"],"rank":10,"holds_content":true,"means_message_shaped":true}"#,
                |e| matches!(e, MemberCompileError::SharedRank { .. }),
            ),
            (
                r#"{"id":"a","members":["x"],"means_content_block":true},
                   {"id":"b","members":["x"],"means_message_shaped":true}"#,
                |e| matches!(e, MemberCompileError::DuplicateMember { .. }),
            ),
            (
                // The same, *within* a family: two declarations is the obvious spelling of a duplicate, one
                // family repeating a spelling is the one a family makes newly possible.
                r#"{"id":"a","members":["x","x"],"means_content_block":true}"#,
                |e| matches!(e, MemberCompileError::DuplicateMember { .. }),
            ),
        ];
        for (rules, expected) in cases {
            let error = compile_rules(rules)
                .err()
                .unwrap_or_else(|| panic!("should have been refused: {rules}"));
            assert!(expected(&error), "wrong refusal for {rules}: {error}");
        }
    }

    /// The shipped vocabulary compiles, and every member that holds content is message-shaped.
    ///
    /// The refusal above says so at compile time; this says the shipped assets satisfy it, which is what makes
    /// adding the refusal a statement about them rather than only about future edits.
    #[test]
    fn the_shipped_vocabulary_satisfies_the_content_implies_shape_rule() {
        let plan = &super::super::ruleset().message_members;
        for member in plan.content_in_order() {
            assert!(
                plan.any_means_message_shaped([member.to_string()].iter()),
                "`{member}` holds content and is not message-shaped"
            );
        }
    }
}
