//! Why a declaration produced no usable carrier value.
//!
//! **One vocabulary, because there were three.** `Outcome<T>` carried `Presence` and a `Vec<Defect>`;
//! `span_fields::Refusal` carried a `Reading`; and an answer's absence was a bare `Option<Verdict<T>>` with no
//! evidence at all. Three shapes for one question - "a source was consulted and did not answer" - which meant a
//! caller could not handle them uniformly and, in practice, two of them reached nobody: `Outcome` and `Presence`
//! were constructed nowhere outside their own tests, and `Defect` survived only as a builder for one log line.
//!
//! The reasoning behind `Outcome`'s shape was sound and is kept, because it is what this module's *structure*
//! obeys: defects are orthogonal to the optional result. A wrapper member that is present and not a list is
//! *simultaneously* a malformed reading of that member and a valid reading of the
//! element around it, so a sum type over (value | failure) has to choose and either choice is a false statement.
//! The live resolver already satisfies that - `Resolved` carries its answer *and* a `refused: Vec<Refusal>`
//! beside it, so the four states are expressible:
//!
//! | Shape | answer | `refused` |
//! | --- | --- | --- |
//! | nobody wrote it | none | empty |
//! | written, holding nothing | none | one `Empty` |
//! | **malformed** | none | one `Malformed` |
//! | **recovered** | some | one `WrongMember` |
//!
//! Orthogonality by *composition* rather than by a field on a struct. `Outcome<T>` was a second implementation
//! of the same ruling that nothing adopted, so it is gone and this is what remains.
//!
//! What a refusal is **not**: an absent carrier. Nobody writing a key is the ordinary case and carries no
//! diagnosis - the previous type admitted `Reading::Absent` while its own doc comment said "present and could
//! not be read", and the push sites guarded that by hand. [`Unusable`] cannot express it.

use super::expr::ClausePath;

/// Why a carrier that *was* written could not be used.
///
/// Typed rather than prose, so a caller can act on the cause without parsing a message; the detail is for
/// whoever reads the diagnostic and is never parsed. The causes are the ones the extraction paths actually
/// produce - a new one is a code change, deliberately, because a cause nobody named is a cause nobody can
/// decide about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unusable {
    /// The key exists and holds nothing: an empty string, an empty list, a JSON `null`.
    ///
    /// Reported rather than silent, because "no producer wrote this" and "a producer wrote it empty" are
    /// different statements about the same empty column.
    Empty,
    /// Present and not this field's type at all - a string where a number belongs, an object where a list does.
    Malformed { detail: String },
    /// A **member** of an otherwise usable value was wrong, and the rest was kept. The recovered row above:
    /// this is the cause that has to coexist with an answer.
    WrongMember { detail: String },
    /// A number the field cannot hold: past a counter's range, or non-finite.
    OutOfRange { detail: String },
    /// A gate could not be asked, because the carrier it reads is present and unreadable. Distinct from the
    /// gate answering no, which is not a refusal - it is the declaration doing its job.
    UnanswerableGate { detail: String },
}

impl Unusable {
    /// The specifics, for a human. `Empty` has none: the cause is the whole statement.
    pub fn detail(&self) -> Option<&str> {
        match self {
            Self::Empty => None,
            Self::Malformed { detail }
            | Self::WrongMember { detail }
            | Self::OutOfRange { detail }
            | Self::UnanswerableGate { detail } => Some(detail),
        }
    }
}

impl std::fmt::Display for Unusable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            Self::Empty => "empty",
            Self::Malformed { .. } => "malformed",
            Self::WrongMember { .. } => "a wrong member",
            Self::OutOfRange { .. } => "out of range",
            Self::UnanswerableGate { .. } => "an unanswerable gate",
        };
        match self.detail() {
            Some(detail) => write!(f, "{name}: {detail}"),
            None => write!(f, "{name}"),
        }
    }
}

/// One declaration that read one carrier and could not use what it found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    /// The declaration: `rule/source`, as [`ClausePath`] renders it. Which of a chain's five spellings failed is
    /// what a reader needs; that *something* failed is not.
    pub clause: ClausePath,
    /// What it read, in the vocabulary a reader recognises - an attribute name, a path, the span's own name.
    pub carrier: String,
    pub cause: Unusable,
}

impl Refusal {
    pub fn new(clause: ClausePath, carrier: impl Into<String>, cause: Unusable) -> Self {
        Self {
            clause,
            carrier: carrier.into(),
            cause,
        }
    }
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} read `{}`: {}", self.clause, self.carrier, self.cause)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refusal_names_the_declaration_the_carrier_and_the_cause() {
        let refusal = Refusal::new(
            ClausePath::root("span-fields.session_id".to_string()),
            "session.id",
            Unusable::Malformed {
                detail: "expected a string, found an object".to_string(),
            },
        );
        assert_eq!(
            refusal.to_string(),
            "span-fields.session_id read `session.id`: malformed: expected a string, found an object",
            "all three facts have to survive into the diagnostic: which declaration, which carrier, and why - \
             a message naming only the column says nothing a reader can act on"
        );
    }

    #[test]
    fn empty_is_a_cause_with_no_detail_and_the_rest_carry_one() {
        assert_eq!(Unusable::Empty.detail(), None);
        assert_eq!(Unusable::Empty.to_string(), "empty");
        for cause in [
            Unusable::Malformed {
                detail: "d".to_string(),
            },
            Unusable::WrongMember {
                detail: "d".to_string(),
            },
            Unusable::OutOfRange {
                detail: "d".to_string(),
            },
            Unusable::UnanswerableGate {
                detail: "d".to_string(),
            },
        ] {
            assert_eq!(
                cause.detail(),
                Some("d"),
                "every cause but `Empty` says something specific, or the taxonomy is decoration"
            );
        }
    }
}
