//! What a reading produced, and what was wrong with it - as two independent facts.
//!
//! Every failure the review series found in the extraction paths was one of two shapes: a value that was
//! *there* and unusable reported as though nobody had written it, or a value that was usable *and* defective
//! with only the value surviving. Both come from the same modelling mistake - treating "what came out" and
//! "what went wrong" as one answer.
//!
//! So they are separate here. `presence` says whether the carrier held anything, `value` whether anything could
//! be made of it, and `defects` what was wrong on the way. The four states that matters are:
//!
//! | Shape | `presence` | `value` | `defects` |
//! | --- | --- | --- | --- |
//! | nobody wrote it | `Absent` | `None` | empty |
//! | written, holding nothing | `Empty` | `None` | empty |
//! | **malformed** | `Present` | `None` | at least one |
//! | **recovered** | `Present` | `Some` | at least one |
//!
//! The last row is why this is not a four-variant enum, and it took a ruling to see: a wrapper member that is
//! present and not a list is *simultaneously* a malformed reading of that member and a valid reading of the
//! element around it, so a sum type has to choose and either choice is a false statement. Codex's words:
//! "defects need to be orthogonal to the optional result".
//!
//! `Presence` is separate from `value` for the mirror reason - collapsed into it, malformed-but-present has to
//! masquerade as `Absent`, which is exactly the distinction the whole thing exists to keep.

use super::expr::ClausePath;

/// Whether the carrier held anything, independent of whether anything could be made of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Presence {
    /// No key in this position carried anything. Nobody wrote it.
    Absent,
    /// The key exists and holds nothing: an empty string, an empty list, a JSON `null`.
    Empty,
    /// The key exists and holds something, whether or not it was usable.
    Present,
}

/// Why a reading did not produce what its declaration asked for.
///
/// Typed rather than a message, so a caller can act on the kind without parsing prose - `on_malformed` decides
/// on the kind, while the detail is for whoever reads the diagnostic. The kinds are the ones the extraction
/// paths actually produce; a new one is a code change, which is the point: a defect nobody named is a defect
/// nobody can decide about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Defect {
    /// The declaration that read it - `rule/source`, as `ClausePath` renders it. Which of a chain's five
    /// spellings failed is what a reader needs; that *something* failed is not.
    pub clause: ClausePath,
    /// What it read, in the vocabulary a reader recognises: an attribute name, a path, the span's own name.
    pub carrier: String,
    pub kind: DefectKind,
    /// The specifics, for a human. Never parsed.
    pub detail: String,
}

/// What kind of thing went wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefectKind {
    /// The carrier is present and does not parse at all.
    Unparseable,
    /// It parsed and holds the wrong kind of thing for what was asked - a string where a number belongs, an
    /// object where a list belongs.
    WrongType,
    /// A *member* of an otherwise usable value was wrong, and the rest was kept. The recovered row above.
    WrongMember,
    /// The value is a number the field cannot hold: past a counter's range, or non-finite.
    OutOfRange,
    /// A gate could not be asked, because the carrier it reads is present and unreadable. Distinct from the
    /// gate answering no, which is not a defect.
    UnanswerableGate,
}

impl Defect {
    pub fn new(
        clause: ClausePath,
        carrier: impl Into<String>,
        kind: DefectKind,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            clause,
            carrier: carrier.into(),
            kind,
            detail: detail.into(),
        }
    }
}

/// What one reading produced, and what was wrong with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome<T> {
    pub presence: Presence,
    pub value: Option<T>,
    pub defects: Vec<Defect>,
}

impl<T> Outcome<T> {
    /// Nobody wrote it.
    pub fn absent() -> Self {
        Self {
            presence: Presence::Absent,
            value: None,
            defects: Vec::new(),
        }
    }

    /// Written, holding nothing. Not a defect: an empty value is a statement a producer can make, and a
    /// declaration decides whether it is an answer (`accept_empty`).
    pub fn empty() -> Self {
        Self {
            presence: Presence::Empty,
            value: None,
            defects: Vec::new(),
        }
    }

    /// A clean value.
    pub fn present(value: T) -> Self {
        Self {
            presence: Presence::Present,
            value: Some(value),
            defects: Vec::new(),
        }
    }

    /// Present, and nothing could be made of it.
    pub fn malformed(defect: Defect) -> Self {
        Self {
            presence: Presence::Present,
            value: None,
            defects: vec![defect],
        }
    }

    /// A value **and** a defect: what was read is usable, and something about it was wrong.
    ///
    /// The row a sum type cannot express. A caller that only wants the value may ignore the defects; a caller
    /// deciding whether to fall through must not, which is why they travel together rather than being reported
    /// through a side channel.
    pub fn recovered(value: T, defect: Defect) -> Self {
        Self {
            presence: Presence::Present,
            value: Some(value),
            defects: vec![defect],
        }
    }

    /// Whether anything could be made of it.
    pub fn yielded(&self) -> bool {
        self.value.is_some()
    }

    /// Whether it is present and nothing could be made of it - what `on_malformed` decides about.
    ///
    /// A *recovered* reading is deliberately **not** malformed by this test: it has a value, and a chain that
    /// stopped on it would discard a usable answer over a member it already recovered from.
    pub fn is_malformed(&self) -> bool {
        self.presence == Presence::Present && self.value.is_none() && !self.defects.is_empty()
    }

    /// The same outcome with its value mapped, keeping presence and defects.
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> Outcome<U> {
        Outcome {
            presence: self.presence,
            value: self.value.map(f),
            defects: self.defects,
        }
    }

    /// Add a defect to a reading that already has a value: the recovered case, built incrementally.
    pub fn with_defect(mut self, defect: Defect) -> Self {
        self.defects.push(defect);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a_defect() -> Defect {
        Defect::new(
            ClausePath::root("probe.rule").then("probe.source"),
            "acme.payload",
            DefectKind::WrongType,
            "expected a number, found a string",
        )
    }

    /// The four states, and the two that a sum type cannot tell apart.
    ///
    /// `malformed` and `absent` both have no value, and collapsing them is the defect this review found six
    /// times: a value that was *there* and unusable reported as though nobody wrote it. `recovered` has a value
    /// *and* a defect, which is the row that forced defects to be orthogonal rather than a variant.
    #[test]
    fn presence_and_value_and_defects_are_three_independent_facts() {
        let absent: Outcome<i64> = Outcome::absent();
        assert_eq!(absent.presence, Presence::Absent);
        assert!(!absent.yielded());
        assert!(
            !absent.is_malformed(),
            "nobody wrote it, so nothing is wrong"
        );

        let empty: Outcome<i64> = Outcome::empty();
        assert_eq!(empty.presence, Presence::Empty);
        assert!(!empty.yielded());
        assert!(
            !empty.is_malformed(),
            "an empty value is a statement a producer can make, and `accept_empty` decides about it"
        );

        let malformed: Outcome<i64> = Outcome::malformed(a_defect());
        assert_eq!(
            malformed.presence,
            Presence::Present,
            "it was there - which is what distinguishes it from `absent`, and what a sum type loses"
        );
        assert!(!malformed.yielded());
        assert!(malformed.is_malformed());

        let recovered = Outcome::recovered(7_i64, a_defect());
        assert_eq!(recovered.presence, Presence::Present);
        assert_eq!(recovered.value, Some(7));
        assert_eq!(recovered.defects.len(), 1);
        assert!(
            !recovered.is_malformed(),
            "a recovered reading has a value, so a chain stopping on it would discard a usable answer over a \
             member it already recovered from"
        );

        // A clean value has no defects, or the distinction would be decorative.
        let clean = Outcome::present(7_i64);
        assert!(clean.defects.is_empty());
        assert!(clean.yielded() && !clean.is_malformed());
    }

    /// Mapping keeps both other facts, and a defect can be added to a value that already exists.
    #[test]
    fn mapping_a_value_keeps_its_presence_and_its_defects() {
        let recovered = Outcome::recovered(7_i64, a_defect()).map(|n| n.to_string());
        assert_eq!(recovered.value, Some("7".to_string()));
        assert_eq!(recovered.presence, Presence::Present);
        assert_eq!(
            recovered.defects.len(),
            1,
            "a map is not a way to lose them"
        );

        let built = Outcome::present(1_i64).with_defect(a_defect());
        assert!(built.yielded() && !built.is_malformed() && built.defects.len() == 1);

        // And an absent reading maps to an absent one: there is nothing to map.
        let absent: Outcome<i64> = Outcome::absent();
        let mapped = absent.map(|n| n.to_string());
        assert_eq!(mapped.presence, Presence::Absent);
        assert_eq!(mapped.value, None);
    }

    /// A defect names the declaration that produced it, not just the carrier.
    ///
    /// Which of a chain's five spellings failed is what a reader needs; that *something* failed is not. This is
    /// the same reason an emission carries an `EvidenceSet`.
    #[test]
    fn a_defect_names_the_declaration_that_produced_it() {
        let defect = a_defect();
        assert_eq!(defect.clause.to_string(), "probe.rule → probe.source");
        assert_eq!(defect.carrier, "acme.payload");
        assert_eq!(defect.kind, DefectKind::WrongType);
    }
}
