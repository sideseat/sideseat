use super::*;
use proptest::prelude::*;

/// The implementation `order_within_unit` replaced, kept verbatim as the oracle.
///
/// O(members^2) in its selection loop and O(degree) per edge in its deduplication, which is why it was
/// replaced; its *answers* are the specification.
fn order_within_unit_reference(members: &[usize], intra_edges: &[(usize, usize)]) -> Vec<usize> {
    if members.len() < 2 {
        return members.to_vec();
    }
    let inside: HashMap<usize, ()> = members.iter().map(|&m| (m, ())).collect();
    let mut successors: HashMap<usize, Vec<usize>> =
        members.iter().map(|&m| (m, Vec::new())).collect();
    let mut indegree: HashMap<usize, usize> = members.iter().map(|&m| (m, 0)).collect();
    for &(from, to) in intra_edges {
        if !inside.contains_key(&from) || !inside.contains_key(&to) {
            continue;
        }
        let succ = successors.get_mut(&from).expect("member present");
        if !succ.contains(&to) {
            succ.push(to);
            *indegree.get_mut(&to).expect("member present") += 1;
        }
    }

    let mut out: Vec<usize> = Vec::with_capacity(members.len());
    let mut remaining: Vec<usize> = members.to_vec();
    while !remaining.is_empty() {
        let Some(&next) = remaining
            .iter()
            .filter(|m| indegree[m] == 0)
            .min_by_key(|&&m| m)
        else {
            return members.to_vec();
        };
        out.push(next);
        remaining.retain(|&m| m != next);
        for &s in &successors[&next] {
            if let Some(d) = indegree.get_mut(&s) {
                *d = d.saturating_sub(1);
            }
        }
    }
    out
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 512,
        failure_persistence: Some(Box::new(
            proptest::test_runner::FileFailurePersistence::Direct(
                "src/sideml/feed/order_graph.seeds",
            ),
        )),
        ..ProptestConfig::default()
    })]

    /// The two agree on every generated graph.
    ///
    /// Members are drawn as a sorted distinct set because that is what the caller passes
    /// (`members.sort_unstable()` immediately before). Edges are drawn from a range *wider* than the
    /// member set, so endpoints outside the unit are generated - the case the `inside` guard exists for
    /// and the one the new bucketing had to reproduce exactly.
    #[test]
    fn the_indexed_order_equals_the_retired_one(
        members in prop::collection::btree_set(0usize..24, 0..12),
        edges in prop::collection::vec((0usize..32, 0usize..32), 0..40),
    ) {
        let members: Vec<usize> = members.into_iter().collect();
        prop_assert_eq!(
            order_within_unit(&members, &edges),
            order_within_unit_reference(&members, &edges)
        );
    }

    /// And on graphs whose edges are all inside the member set, which is where a real ordering happens.
    ///
    /// The wide generator above produces mostly-skipped edges, so most of its cases exercise the guard
    /// rather than the sort. This one keeps every edge in range, which is what makes cycles, chains and
    /// multi-edges common enough to matter.
    #[test]
    fn the_indexed_order_equals_the_retired_one_on_dense_graphs(
        size in 2usize..10,
        pairs in prop::collection::vec((0usize..10, 0usize..10), 0..30),
    ) {
        let members: Vec<usize> = (0..size).collect();
        let edges: Vec<(usize, usize)> = pairs
            .into_iter()
            .map(|(a, b)| (a % size, b % size))
            .collect();
        prop_assert_eq!(
            order_within_unit(&members, &edges),
            order_within_unit_reference(&members, &edges)
        );
    }
}

/// A cycle falls back to source order in both, rather than to a partial order.
///
/// Called out as its own case because it is the one place the two implementations detect the condition
/// differently - the old one when no zero-indegree member remained, the new one when the heap drained
/// early - and "returns something plausible" would pass an equality test against a matching bug.
#[test]
fn a_cycle_keeps_source_order() {
    let members = vec![3usize, 1, 2];
    let edges = vec![(1, 2), (2, 3), (3, 1)];
    assert_eq!(order_within_unit(&members, &edges), members);
    assert_eq!(order_within_unit_reference(&members, &edges), members);
}

/// A repeated edge does not double the indegree, in either implementation.
///
/// Without the deduplication the target's indegree never reaches zero, so it is dropped from the output
/// and the length check reads it as a cycle - source order, silently, for a graph that has a valid one.
#[test]
fn a_repeated_edge_does_not_strand_its_target() {
    let members = vec![0usize, 1];
    let edges = vec![(0, 1), (0, 1), (0, 1)];
    assert_eq!(order_within_unit(&members, &edges), vec![0, 1]);
    assert_eq!(order_within_unit_reference(&members, &edges), vec![0, 1]);
}
