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
/// Ordered so that a set of paths sorts into document order: `Key` before `Index` is arbitrary but
/// consistent, and sibling indices sort numerically, which is what a stable output order needs.
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
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
    fn siblings_differ_and_sort_in_document_order() {
        let parent = PositionPath::root(0).child_key("content");
        let first = parent.child_index(0);
        let second = parent.child_index(1);
        let tenth = parent.child_index(10);

        assert_ne!(
            first, second,
            "two entries of one array must never share a path - this is what tells identical \
             content apart without needing ids"
        );
        let mut sorted = vec![tenth.clone(), second.clone(), first.clone()];
        sorted.sort();
        assert_eq!(
            sorted,
            vec![first, second, tenth],
            "indices must order numerically, not as text: 10 sorts after 2"
        );
    }

    #[test]
    fn an_absent_path_is_recognisable() {
        assert!(PositionPath::default().is_empty());
        assert!(!PositionPath::root(0).is_empty());
    }
}
