//! One precedence mechanism for every ordered arena.
//!
//! An **arena** is a set of clauses tried in order, where the first that holds answers: detection rules, the
//! two classifications, message rules that contend for a carrier, the cases at one content-chain position,
//! tool shapes, event categories, and each ordered question of `message_members`. Every one of them orders its
//! clauses the same way:
//!
//! - **`priority`**, an integer, lower first. Two clauses of one arena sharing a priority are refused, because
//!   whatever broke the tie - load order, an id comparison - would be a precedence nobody stated.
//! - **`supersedes`**, where an arena accepts it, names the clauses this one is meant to beat where both hold.
//!   It is documentation of an intended overlap and is *validated*, never executed: each target must exist,
//!   differ from the source, be named once, and come **after** the source by priority. So the priority alone
//!   decides every answer, and deleting an edge changes none (`server/specs/CheckedPrecedence.tla`).
//!
//! What an arena *is* stays the caller's to say, because it is not always an equivalence: two message rules
//! compete only where their stage, output axis and input domain overlap, and that relation is pairwise.

/// The first two clauses that compete and share a priority, in priority order.
///
/// `same_arena` says which pairs compete; a section whose clauses all compete passes `|_, _| true`. Clauses that
/// share a priority and do not compete are legitimate and pass.
pub fn shared_priority<T>(
    clauses: &[T],
    priority: impl Fn(&T) -> i32,
    same_arena: impl Fn(&T, &T) -> bool,
) -> Option<(&T, &T)> {
    let mut order: Vec<&T> = clauses.iter().collect();
    order.sort_by_key(|clause| priority(clause));
    for (index, first) in order.iter().enumerate() {
        for second in &order[index + 1..] {
            if priority(second) != priority(first) {
                break;
            }
            if same_arena(first, second) {
                return Some((first, second));
            }
        }
    }
    None
}

/// Why one `supersedes` edge cannot mean what it says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeDefect {
    /// The source names the same target twice.
    Duplicate,
    /// The source names itself.
    ToItself,
    /// No clause of the arena has this id.
    UnknownTarget,
    /// The target is tried first, so the edge states an order the priorities contradict.
    DisagreesWithPriority { source: i32, target: i32 },
}

impl EdgeDefect {
    /// The defect as the tail of a sentence naming the edge.
    pub fn describe(self) -> String {
        match self {
            Self::Duplicate => "is named twice by this clause".to_string(),
            Self::ToItself => "is the clause itself".to_string(),
            Self::UnknownTarget => "no clause of this arena declares".to_string(),
            Self::DisagreesWithPriority { source, target } => format!(
                "is tried first (priority {target}, against this clause's {source}): `supersedes` documents an \
                 overlap the priorities already resolve, so the source must come first"
            ),
        }
    }
}

/// The first defective edge of one clause, and the target it names.
pub fn edge_defect<'t>(
    source_id: &str,
    source_priority: i32,
    targets: &'t [String],
    priority_of: impl Fn(&str) -> Option<i32>,
) -> Option<(&'t str, EdgeDefect)> {
    let mut seen: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    for target in targets {
        let defect = if !seen.insert(target.as_str()) {
            Some(EdgeDefect::Duplicate)
        } else if target == source_id {
            Some(EdgeDefect::ToItself)
        } else {
            match priority_of(target) {
                None => Some(EdgeDefect::UnknownTarget),
                Some(target_priority) if target_priority <= source_priority => {
                    Some(EdgeDefect::DisagreesWithPriority {
                        source: source_priority,
                        target: target_priority,
                    })
                }
                Some(_) => None,
            }
        };
        if let Some(defect) = defect {
            return Some((target.as_str(), defect));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shared_priority_is_found_only_between_competing_clauses() {
        let clauses = [(1, "a", 'x'), (2, "b", 'x'), (2, "c", 'y'), (3, "d", 'x')];
        let competing = |a: &(i32, &str, char), b: &(i32, &str, char)| a.2 == b.2;
        assert_eq!(shared_priority(&clauses, |c| c.0, competing), None);
        assert_eq!(
            shared_priority(&clauses, |c| c.0, |_, _| true).map(|(a, b)| (a.1, b.1)),
            Some(("b", "c"))
        );
    }

    #[test]
    fn every_kind_of_defective_edge_is_named() {
        let priority_of = |id: &str| match id {
            "early" => Some(1),
            "late" => Some(9),
            _ => None,
        };
        let edges = |targets: &[&str]| targets.iter().map(|t| t.to_string()).collect::<Vec<_>>();
        assert_eq!(edge_defect("mid", 5, &edges(&["late"]), priority_of), None);
        assert_eq!(
            edge_defect("mid", 5, &edges(&["late", "late"]), priority_of),
            Some(("late", EdgeDefect::Duplicate))
        );
        assert_eq!(
            edge_defect("mid", 5, &edges(&["mid"]), priority_of),
            Some(("mid", EdgeDefect::ToItself))
        );
        assert_eq!(
            edge_defect("mid", 5, &edges(&["absent"]), priority_of),
            Some(("absent", EdgeDefect::UnknownTarget))
        );
        assert_eq!(
            edge_defect("mid", 5, &edges(&["early"]), priority_of),
            Some((
                "early",
                EdgeDefect::DisagreesWithPriority {
                    source: 5,
                    target: 1
                }
            ))
        );
        // An equal priority is not "after", whatever else refuses the tie.
        assert!(matches!(
            edge_defect("mid", 9, &edges(&["late"]), priority_of),
            Some((_, EdgeDefect::DisagreesWithPriority { .. }))
        ));
    }
}
