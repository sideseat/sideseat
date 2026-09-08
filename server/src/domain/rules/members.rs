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
    let mut ranked: Vec<(i32, String, String)> = Vec::new();
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
            if rule.member.is_empty() {
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
            if let Some(first) = by_member.get(&rule.member) {
                return Err(MemberCompileError::DuplicateMember {
                    member: rule.member.clone(),
                    first: first.clone(),
                    second: rule.id.clone(),
                });
            }
            by_member.insert(rule.member.clone(), rule.id.clone());

            if rule.means_message_shaped {
                plan.message_shaped.insert(rule.member.clone());
            }
            if rule.means_content_block {
                plan.content_block.insert(rule.member.clone());
            }
            if let Some(rank) = rule.rank {
                ranked.push((rank, rule.member.clone(), rule.id.clone()));
            }
        }
    }

    ranked.sort_by_key(|(rank, _, _)| *rank);
    for pair in ranked.windows(2) {
        if pair[0].0 == pair[1].0 {
            return Err(MemberCompileError::SharedRank {
                first: pair[0].2.clone(),
                second: pair[1].2.clone(),
                rank: pair[0].0,
            });
        }
    }
    plan.content_in_order = ranked.into_iter().map(|(_, member, _)| member).collect();
    Ok(plan)
}
