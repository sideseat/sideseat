//! Where an observation sat in the payload it came from.
//!
//! Reconstruction has to decide which observations are the same message. It did that from content
//! alone, because the structure was thrown away: a stored message array is expanded and every tool
//! call is split into a message of its own, so by the time anything compares them, "the third entry
//! of that array" is gone and two identical entries look like one message seen twice.
//!
//! A [`PositionPath`] is that structure, carried instead of re-derived. It records the route from the
//! stored payload to the observation - `["messages", 3, "content", 1]` - and every expansion step
//! *appends* to it rather than cloning the source. Two observations from one payload therefore differ
//! by construction, whether or not their content differs and whether or not the framework supplied
//! ids.
//!
//! What it is not: a comparison key across payloads. Two spans that each re-send the same
//! conversation have their own paths, and matching those is content's job. The path distinguishes
//! *within* one payload; content distinguishes *between* payloads.

use std::fmt;

use serde::{Deserialize, Serialize};

/// One step from a payload's root towards an observation.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum PathSegment {
    /// A JSON object member, e.g. `messages` in `{"messages": [...]}`.
    Key(String),
    /// A position in a JSON array, or in the stored observation list itself.
    Index(usize),
}

impl fmt::Display for PathSegment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Key(key) => write!(f, "{key}"),
            Self::Index(index) => write!(f, "{index}"),
        }
    }
}

/// The route from a stored payload to one observation.
///
/// **Deliberately not `Ord`.** It was, with the claim that "a set of paths sorts into document order", and that
/// claim is false: a JSON object's member order is not recoverable from a parsed value, so `Key` segments sort
/// lexically - `content` before `messages` whatever the payload said - and two paths from different spans
/// compare to a definite answer that means nothing. Nothing sorted paths, so the order was an unused, wrong
/// guarantee that the next caller would have believed. What *is* true is [`Self::document_order`]: siblings
/// under one parent, differing at an array index, are in the order the payload wrote them - and that is the
/// only comparison the ordering design ever asks for.
///
/// `Hash` and the serde derives went the same way, for the same reason: nothing keyed a map by a path and
/// nothing put one on the wire.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PositionPath(Vec<PathSegment>);

impl PositionPath {
    /// The path of the `index`-th observation of a span's stored list.
    pub fn root(index: usize) -> Self {
        Self(vec![PathSegment::Index(index)])
    }

    /// This path with an object member appended.
    pub fn child_key(&self, key: &str) -> Self {
        let mut segments = self.0.clone();
        segments.push(PathSegment::Key(key.to_string()));
        Self(segments)
    }

    /// This path with an array position appended.
    pub fn child_index(&self, index: usize) -> Self {
        let mut segments = self.0.clone();
        segments.push(PathSegment::Index(index));
        Self(segments)
    }

    /// True when nothing is recorded - a path from before provenance was carried, or a synthesised
    /// observation with no place in any payload.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The segments, for callers that need to compare prefixes.
    pub fn segments(&self) -> &[PathSegment] {
        &self.0
    }

    /// How many steps from the payload's root.
    pub fn depth(&self) -> usize {
        self.0.len()
    }

    /// The path of the container this observation sits in, or `None` at the root.
    pub fn parent(&self) -> Option<Self> {
        (!self.0.is_empty()).then(|| Self(self.0[..self.0.len() - 1].to_vec()))
    }

    /// True when `other` is inside this one - the same route, continued.
    ///
    /// Strict: a path is not its own ancestor. This is the question "did that block come out of this
    /// message", which expansion makes answerable and which content cannot answer at all.
    pub fn is_ancestor_of(&self, other: &Self) -> bool {
        other.0.len() > self.0.len() && other.0[..self.0.len()] == self.0[..]
    }

    /// The first step at which two paths differ, or `None` when one contains the other (or they are equal).
    pub fn divergence<'a>(&'a self, other: &'a Self) -> Option<(&'a PathSegment, &'a PathSegment)> {
        self.0
            .iter()
            .zip(other.0.iter())
            .find(|(mine, theirs)| mine != theirs)
    }

    /// Document order, **where that is a fact**: two observations under the same parent, differing at an array
    /// index, were written in that order by the producer.
    ///
    /// `None` everywhere else, and each case is a different kind of "no answer" rather than an oversight:
    /// different parents (nothing relates them - they may be in different spans), divergence at an object
    /// member (a JSON object has no order to recover), or one path inside the other (a container and its
    /// content are not siblings). A caller that needs a total order for determinism must say so with its own
    /// tie-break; it cannot borrow one from here, which is the point.
    pub fn document_order(&self, other: &Self) -> Option<std::cmp::Ordering> {
        if self.0.len() != other.0.len() {
            return None;
        }
        match self.divergence(other) {
            None => Some(std::cmp::Ordering::Equal),
            Some((PathSegment::Index(mine), PathSegment::Index(theirs))) => {
                // Siblings only: the divergence is the *last* step, so everything before it is the shared
                // parent. Measured as the length of the common prefix - I first wrote it as the shared
                // *suffix*, which is the same number only by coincidence and inverted the answer: siblings
                // were refused and cousins were ordered.
                let shared = self
                    .0
                    .iter()
                    .zip(other.0.iter())
                    .take_while(|(a, b)| a == b)
                    .count();
                (shared == self.0.len() - 1).then(|| mine.cmp(theirs))
            }
            Some(_) => None,
        }
    }
}

impl fmt::Display for PositionPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, segment) in self.0.iter().enumerate() {
            if i > 0 {
                write!(f, ".")?;
            }
            write!(f, "{segment}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_records_the_route_and_appends_without_mutating() {
        let root = PositionPath::root(2);
        let messages = root.child_key("messages");
        let third = messages.child_index(3);
        let first_block = third.child_key("content").child_index(1);

        assert_eq!(root.to_string(), "2");
        assert_eq!(third.to_string(), "2.messages.3");
        assert_eq!(first_block.to_string(), "2.messages.3.content.1");
        // Appending returns a new path: the parent is still usable, which is what lets one payload's
        // expansion fan out without the children sharing a mutable path.
        assert_eq!(messages.to_string(), "2.messages");
    }

    #[test]
    fn siblings_differ_and_order_by_index() {
        use std::cmp::Ordering;
        let parent = PositionPath::root(0).child_key("content");
        let first = parent.child_index(0);
        let second = parent.child_index(1);
        let tenth = parent.child_index(10);

        assert_ne!(
            first, second,
            "two entries of one array must never share a path - this is what tells identical \
             content apart without needing ids"
        );
        assert_eq!(first.document_order(&second), Some(Ordering::Less));
        assert_eq!(
            second.document_order(&tenth),
            Some(Ordering::Less),
            "indices order numerically, not as text: 10 comes after 2"
        );
        assert_eq!(first.document_order(&first), Some(Ordering::Equal));
    }

    #[test]
    fn document_order_is_absent_wherever_it_would_be_invented() {
        let one = PositionPath::root(0).child_key("messages").child_index(0);
        let other_span = PositionPath::root(1).child_key("messages").child_index(0);
        assert_eq!(
            one.document_order(&other_span),
            None,
            "two observations under different parents are unrelated - they can be in different spans, and a \
             definite answer here is the invented order this type used to derive"
        );

        let by_key = PositionPath::root(0).child_key("content");
        let other_key = PositionPath::root(0).child_key("messages");
        assert_eq!(
            by_key.document_order(&other_key),
            None,
            "a JSON object has no member order to recover, so `content` before `messages` would be a fact \
             about the alphabet rather than about the payload"
        );

        let container = PositionPath::root(0).child_key("content");
        let inside = container.child_index(0);
        assert_eq!(
            container.document_order(&inside),
            None,
            "a container and its content are not siblings"
        );

        let deep_a = PositionPath::root(0).child_index(1).child_index(0);
        let deep_b = PositionPath::root(0).child_index(2).child_index(0);
        assert_eq!(
            deep_a.document_order(&deep_b),
            None,
            "the divergence is not at the last step: these are cousins, and their order is their parents' \
             order, which the caller must ask for explicitly"
        );
    }

    #[test]
    fn containment_and_divergence_answer_where_a_block_came_from() {
        let message = PositionPath::root(0).child_key("messages").child_index(3);
        let block = message.child_key("content").child_index(1);

        assert!(message.is_ancestor_of(&block));
        assert!(!block.is_ancestor_of(&message));
        assert!(
            !message.is_ancestor_of(&message),
            "strict: a path is not its own ancestor, or `did this block come out of that message` \
             answers yes for the message itself"
        );
        assert_eq!(block.depth(), 5);
        assert_eq!(
            block.parent().map(|p| p.to_string()).as_deref(),
            Some("0.messages.3.content")
        );
        assert_eq!(PositionPath::default().parent(), None);

        let sibling = message.child_key("content").child_index(2);
        assert!(matches!(
            block.divergence(&sibling),
            Some((PathSegment::Index(1), PathSegment::Index(2)))
        ));
        assert_eq!(
            block.divergence(&block),
            None,
            "equal paths do not diverge, and neither does a path from one that contains it"
        );
    }

    #[test]
    fn an_absent_path_is_recognisable() {
        assert!(PositionPath::default().is_empty());
        assert!(!PositionPath::root(0).is_empty());
    }
}
