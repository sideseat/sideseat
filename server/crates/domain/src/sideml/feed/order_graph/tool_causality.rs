use super::*;

/// Exact call/result pairs recovered from every observation that dedup projected to each survivor.
///
/// A representative may be an id-less snapshot even though another copy carried a correlated id.
/// Reading the evidence makes the edge independent of that quality tie. Distinct ids or callers
/// remain ambiguous and deliberately produce no pair.
pub(super) fn exact_tool_pairs(
    evidence: &[OrderEvidence],
    survivors: &[BlockEntry],
    lineage: &[Option<usize>],
    repeat_ordinals: &[u32],
) -> HashSet<(usize, usize)> {
    type CallKey = (String, u32);

    let mut calls: HashMap<CallKey, HashSet<usize>> = HashMap::new();
    let mut result_ids: HashMap<usize, HashSet<String>> = HashMap::new();
    for (survivor, block) in survivors.iter().enumerate() {
        let Some(id) = block.tool_use_id.as_ref().filter(|id| !id.is_empty()) else {
            continue;
        };
        let ordinal = repeat_ordinals.get(survivor).copied().unwrap_or(0);
        if block.entry_type == "tool_use" {
            calls
                .entry((id.clone(), ordinal))
                .or_default()
                .insert(survivor);
        } else if block.entry_type == "tool_result" {
            result_ids.entry(survivor).or_default().insert(id.clone());
        }
    }
    for (observation, seen) in evidence.iter().enumerate() {
        let Some(survivor) = lineage.get(observation).copied().flatten() else {
            continue;
        };
        let ordinal = repeat_ordinals.get(survivor).copied().unwrap_or(0);
        match &seen.tool_reference {
            Some(ToolReference::Call(id)) => {
                calls
                    .entry((id.clone(), ordinal))
                    .or_default()
                    .insert(survivor);
            }
            Some(ToolReference::Result(id)) => {
                result_ids.entry(survivor).or_default().insert(id.clone());
            }
            None => {}
        }
    }

    let mut pairs = HashSet::new();
    #[expect(
        clippy::iter_over_hash_type,
        reason = "each result decides its own pair alone, into a set"
    )]
    for (result, ids) in result_ids {
        if survivors
            .get(result)
            .is_none_or(|block| block.entry_type != "tool_result")
            || ids.len() != 1
        {
            continue;
        }
        let ordinal = repeat_ordinals.get(result).copied().unwrap_or(0);
        let Some(id) = ids.into_iter().next() else {
            continue;
        };
        let Some(callers) = calls
            .get(&(id, ordinal))
            .filter(|callers| callers.len() == 1)
        else {
            continue;
        };
        let Some(&call) = callers.iter().next() else {
            continue;
        };
        pairs.insert((call, result));
    }
    pairs
}

/// Map each survivor in a multi-call emission to `(emission, branch call)`.
///
/// A provider may flatten parallel branches as `call A, call B, result A, result B` in one snapshot
/// and replay them as `call A, result A, call B, result B`. Both are the same partial order. Keeping
/// the source sequence between branches would turn one valid linearisation into a contradiction.
pub(super) fn parallel_tool_branches(
    evidence: &[OrderEvidence],
    lineage: &[Option<usize>],
    exact_pairs: &HashSet<(usize, usize)>,
) -> HashMap<usize, (usize, usize)> {
    let mut calls_by_emission: HashMap<usize, HashSet<usize>> = HashMap::new();
    for (observation, seen) in evidence.iter().enumerate() {
        let (Some(emission), Some(ToolReference::Call(_)), Some(call)) = (
            seen.emission,
            seen.tool_reference.as_ref(),
            lineage.get(observation).copied().flatten(),
        ) else {
            continue;
        };
        calls_by_emission.entry(emission).or_default().insert(call);
    }

    let mut results_by_call: HashMap<usize, Vec<usize>> = HashMap::new();
    #[expect(
        clippy::iter_over_hash_type,
        reason = "a result has one call, and each list is only read to give its results that call's branch"
    )]
    for &(call, result) in exact_pairs {
        results_by_call.entry(call).or_default().push(result);
    }
    // In emission order, and a survivor two multi-call emissions both hold keeps the first one's branch: one
    // call observed through two carriers is two emissions of it, and which of them claimed it was the map's
    // hash order, so the branch - and the edges `causal_sequence_edges` relaxes by it - changed between runs.
    let mut emissions: Vec<(usize, HashSet<usize>)> = calls_by_emission.into_iter().collect();
    emissions.sort_unstable_by_key(|(emission, _)| *emission);
    let mut branches = HashMap::new();
    for (emission, calls) in emissions {
        if calls.len() < 2 {
            continue;
        }
        let mut calls: Vec<usize> = calls.into_iter().collect();
        calls.sort_unstable();
        for call in calls {
            branches.entry(call).or_insert((emission, call));
            for &result in results_by_call.get(&call).into_iter().flatten() {
                branches.entry(result).or_insert((emission, call));
            }
        }
    }
    branches
}

/// Edges stated by one source sequence, relaxing only complete interleavings of parallel tool branches.
pub(super) fn causal_sequence_edges(
    sequence: &[usize],
    branches: &HashMap<usize, (usize, usize)>,
) -> HashSet<(usize, usize)> {
    type BranchBounds = HashMap<usize, (usize, usize)>;

    let mut by_emission: HashMap<usize, BranchBounds> = HashMap::new();
    for (position, survivor) in sequence.iter().copied().enumerate() {
        let Some(&(emission, branch)) = branches.get(&survivor) else {
            continue;
        };
        by_emission
            .entry(emission)
            .or_default()
            .entry(branch)
            .and_modify(|bounds| bounds.1 = position)
            .or_insert((position, position));
    }

    let mut parallel = HashSet::new();
    #[expect(
        clippy::iter_over_hash_type,
        reason = "each emission is judged alone, into a set"
    )]
    for (&emission, branch_bounds) in &by_emission {
        if branch_bounds.len() < 2 {
            continue;
        }
        let first = branch_bounds
            .values()
            .map(|bounds| bounds.0)
            .min()
            .unwrap_or(0);
        let last = branch_bounds
            .values()
            .map(|bounds| bounds.1)
            .max()
            .unwrap_or(0);
        let complete = sequence[first..=last].iter().all(|survivor| {
            branches
                .get(survivor)
                .is_some_and(|(group, _)| *group == emission)
        });
        if complete {
            parallel.insert(emission);
        }
    }

    let mut edges = HashSet::new();
    for pair in sequence.windows(2) {
        let crosses_parallel_branches = match (branches.get(&pair[0]), branches.get(&pair[1])) {
            (Some((left_group, left_branch)), Some((right_group, right_branch))) => {
                left_group == right_group
                    && left_branch != right_branch
                    && parallel.contains(left_group)
            }
            _ => false,
        };
        if !crosses_parallel_branches {
            edges.insert((pair[0], pair[1]));
        }
    }

    #[expect(
        clippy::iter_over_hash_type,
        reason = "every edge goes into a set, and the bounds' minimum and maximum have no order"
    )]
    for emission in parallel {
        let branch_bounds = &by_emission[&emission];
        let first = branch_bounds
            .values()
            .map(|bounds| bounds.0)
            .min()
            .unwrap_or(0);
        let last = branch_bounds
            .values()
            .map(|bounds| bounds.1)
            .max()
            .unwrap_or(0);
        if let Some(&before) = first.checked_sub(1).and_then(|index| sequence.get(index)) {
            #[expect(clippy::iter_over_hash_type, reason = "into a set")]
            for &(branch_first, _) in branch_bounds.values() {
                edges.insert((before, sequence[branch_first]));
            }
        }
        if let Some(&after) = sequence.get(last + 1) {
            #[expect(clippy::iter_over_hash_type, reason = "into a set")]
            for &(_, branch_last) in branch_bounds.values() {
                edges.insert((sequence[branch_last], after));
            }
        }
    }
    edges
}
