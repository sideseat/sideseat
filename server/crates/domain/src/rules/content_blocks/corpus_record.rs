//! Every question the content-block chain is asked while a closure runs, so a corpus test can ask each one
//! again of the full walk and compare (`ContentBlockPlan::normalize_with_every_case`).

use std::cell::RefCell;

use serde_json::Value as JsonValue;

use super::{ContentBlockPlan, Expansion, predicates_hold};
use crate::rules::schema::ChainPosition;

/// One question the chain was asked: a block, the position it was asked at (`None` for an envelope expansion),
/// and whether the block was a message's own.
pub type Asked = (JsonValue, Option<ChainPosition>, bool);

thread_local! {
    static ASKED: RefCell<Option<Vec<Asked>>> = const { RefCell::new(None) };
}

pub(super) fn record(block: &JsonValue, at: Option<ChainPosition>, consult_envelopes: bool) {
    ASKED.with(|asked| {
        if let Some(asked) = asked.borrow_mut().as_mut() {
            asked.push((block.clone(), at, consult_envelopes));
        }
    });
}

/// Run `work`, and return what it returned with every question the chain was asked on this thread meanwhile.
pub fn questions_asked<T>(work: impl FnOnce() -> T) -> (T, Vec<Asked>) {
    ASKED.with(|asked| *asked.borrow_mut() = Some(Vec::new()));
    let out = work();
    let asked = ASKED
        .with(|asked| asked.borrow_mut().take())
        .unwrap_or_default();
    (out, asked)
}

impl ContentBlockPlan {
    /// [`Self::normalize_with`] walking **every** case of the position, as it did before the index: the oracle
    /// the corpus equivalence test holds the indexed walk to. Nested normalisation inside a case is the indexed
    /// one, so a comparison answers for one level, and the corpus record holds every level.
    pub fn normalize_with_every_case(
        &self,
        block: &JsonValue,
        at: ChainPosition,
        consult_envelopes: bool,
    ) -> Option<JsonValue> {
        let _depth = super::NormalisationDepth::enter()?;
        let (cases, _) = self.position(at);
        self.first_answer(block, cases.iter(), consult_envelopes)
    }

    /// [`Self::expand`] asking every envelope case: its oracle, as above.
    pub fn expand_with_every_case<'b>(&self, block: &'b JsonValue) -> Option<Expansion<'b>> {
        let rule = self
            .envelopes
            .iter()
            .find(|rule| predicates_hold(block, &rule.require))?;
        Self::expansion(block, rule)
    }
}
