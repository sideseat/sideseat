//! Member names a producer uses, and what each one's presence means.
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
    #[error("member rules in `{path}` are malformed: {message}")]
    Parse { path: String, message: String },
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
        "member rule `{rule}` in `{file}` holds content and states no rank, so its position among the others is undeclared"
    )]
    ContentWithoutARank { file: String, rule: String },
    #[error(
        "member rule `{rule}` in `{file}` states a rank without holding content, which orders nothing"
    )]
    RankWithoutContent { file: String, rule: String },
    #[error(
        "member rules `{first}` and `{second}` share content rank {rank}, so which holds a value's content depends on load order"
    )]
    SharedRank {
        first: String,
        second: String,
        rank: i32,
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

    pub fn rule_count(&self) -> usize {
        self.message_shaped.len() + self.content_block.len() + self.content_in_order.len()
    }
}

pub fn compile(
    sources: &std::collections::BTreeMap<String, Vec<u8>>,
) -> Result<MemberPlan, MemberCompileError> {
    let mut plan = MemberPlan::default();
    let mut ranked: Vec<(i32, usize, String, String)> = Vec::new();
    // One declaration per member name, across every asset: two would make "what does this member mean" a
    // question with two answers, resolved by load order.
    let mut by_member: std::collections::HashMap<String, String> = std::collections::HashMap::new();

    for (file_id, bytes) in sources {
        let file: super::schema::RuleFile =
            serde_json::from_slice(bytes).map_err(|error| MemberCompileError::Parse {
                path: file_id.clone(),
                message: error.to_string(),
            })?;
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
            if !rule.holds_content && !rule.means_message_shaped && !rule.means_content_block {
                return Err(MemberCompileError::SaysNothing {
                    file: file_id.clone(),
                    rule: rule.id.clone(),
                });
            }
            if rule.holds_content && rule.rank.is_none() {
                return Err(MemberCompileError::ContentWithoutARank {
                    file: file_id.clone(),
                    rule: rule.id.clone(),
                });
            }
            if !rule.holds_content && rule.rank.is_some() {
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
            }
            // One rank per declaration, so a family's spellings sit together in declaration order - deterministic
            // where two aliases somehow appear on one value, which is pathological but must still have an answer.
            if let Some(rank) = rule.rank {
                for (offset, member) in rule.members.iter().enumerate() {
                    ranked.push((rank, offset, member.clone(), rule.id.clone()));
                }
            }
        }
    }

    ranked.sort_by_key(|(rank, offset, _, _)| (*rank, *offset));
    for pair in ranked.windows(2) {
        // Two *declarations* sharing a rank is the refusal; a family's own spellings share one by construction.
        if pair[0].0 == pair[1].0 && pair[0].3 != pair[1].3 {
            return Err(MemberCompileError::SharedRank {
                first: pair[0].3.clone(),
                second: pair[1].3.clone(),
                rank: pair[0].0,
            });
        }
    }
    plan.content_in_order = ranked.into_iter().map(|(_, _, member, _)| member).collect();
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One asset holding exactly these member rules.
    fn compile_rules(rules: &str) -> Result<MemberPlan, MemberCompileError> {
        let asset = format!(r#"{{"id":"probe","message_members":[{rules}]}}"#);
        let mut sources = std::collections::BTreeMap::new();
        sources.insert("probe".to_string(), asset.into_bytes());
        compile(&sources)
    }

    /// A spelling family is one declaration with one flag vector, and every spelling in it answers alike.
    ///
    /// This is what the merged section could not express: as separate declarations the aliases drifted, and the
    /// compiler had no way to see it because two names carrying different flags is exactly what a vocabulary of
    /// distinct members looks like.
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
