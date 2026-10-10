use super::*;

pub(in crate::sideml::feed) fn resolve(
    evidence: &[OrderEvidence],
    survivors: &[BlockEntry],
    lineage: &[Option<usize>],
    repeat_ordinals: &[u32],
    span_timestamps: &HashMap<String, SpanTimestamps>,
    constraints: Constraints,
) -> Vec<BlockEntry> {
    let n = survivors.len();
    if n <= 1 {
        return survivors.to_vec();
    }

    // Which survivor each observation became, as the pipeline recorded it - not recomputed here.
    //
    // Recomputing was wrong twice over. Survivors are not unique by `MessageIdentity`, because dedup
    // keys on `(identity, repeat ordinal)`: a response holding two identical tool calls with distinct
    // ids keeps both, and one map entry then took the other's evidence (`crewai/mcp_tools` is the
    // corpus trace that does this). And `withdraw_unbacked_ids` runs in between, clearing a
    // correlated result's id - which changes its identity outright, so its evidence stopped matching
    // anything at all.
    let survivor_of =
        |observation: usize| -> Option<usize> { lineage.get(observation).copied().flatten() };

    // Co-emission sets from the evidence: group credible-emission observations by instance, collect
    // the surviving identities in each, in source order. A block whose identity did not survive is
    // ignored - the unit is over survivors.
    let mut by_instance: HashMap<usize, Vec<EmissionMember>> = HashMap::new();
    for (observation, seen) in evidence.iter().enumerate() {
        let Some(instance) = seen.emission else {
            continue;
        };
        let Some(survivor) = survivor_of(observation) else {
            continue;
        };
        by_instance.entry(instance).or_default().push((
            seen.message_index,
            seen.entry_index,
            survivor,
        ));
    }

    let redundant = redundant_relistings(evidence, survivors, &survivor_of);

    // Contract each instance's survivors into one unit, and remember the source order within it.
    //
    // Iterated in a deterministic order: a HashMap's iteration order varies per run, and while
    // union-find's result does not depend on the order the unions arrive in, the intra-unit keys and
    // any later diagnostics do.
    let mut instances: Vec<(&usize, &Vec<EmissionMember>)> = by_instance.iter().collect();
    instances.sort_by_key(|(instance, _)| **instance);

    // A survivor is routinely claimed by *two* emission instances: an inner generation span emits a
    // message and its parent agent span re-emits the same one as its own output. That is the common
    // shape, not an anomaly, so instances are merged rather than rejected - and the consequence is
    // that a unit's members can carry position paths rooted in different payloads, whose coordinates
    // are not comparable. Taking a global minimum position across them can violate the source order
    // of both emissions.
    //
    // So each instance contributes *adjacency* rather than coordinates: consecutive members of one
    // emission become an edge, and a unit's members are ordered by resolving those edges. Two
    // emissions that agree are both honoured; if they disagree the unit falls back to legacy order,
    // which is the only answer that cannot claim to satisfy evidence it contradicts.
    let mut uf = UnionFind::new(n);
    let mut intra_edges: Vec<(usize, usize)> = Vec::new();
    for (&instance, members) in instances {
        // A redundant re-listing is not contracted and states no order: every message in it was produced
        // below, where the real emission's own sequence already says how it went.
        if redundant.contains(&instance) {
            continue;
        }
        let mut legacy: Vec<usize> = members.iter().map(|&(_, _, s)| s).collect();
        legacy.sort_unstable();
        legacy.dedup();

        if !constraints.contract_non_contiguous_emissions
            && !legacy.windows(2).all(|w| w[1] == w[0] + 1)
        {
            continue;
        }

        // This emission's own order, by source position, deduplicated: two blocks of one message can
        // map to one survivor.
        let mut ordered: Vec<EmissionMember> = members.clone();
        ordered.sort_by_key(|&(msg_idx, entry_idx, survivor)| (msg_idx, entry_idx, survivor));
        let mut sequence: Vec<usize> = Vec::with_capacity(ordered.len());
        for (_, _, survivor) in ordered {
            if sequence.last() != Some(&survivor) {
                sequence.push(survivor);
            }
        }
        for pair in sequence.windows(2) {
            if pair[0] != pair[1] {
                intra_edges.push((pair[0], pair[1]));
            }
        }
        // Union all survivors of this instance together.
        let first = sequence[0];
        for &survivor in &sequence[1..] {
            uf.union(first, survivor);
        }
    }

    // A part a later request's history is the first to show joins the response it was re-sent with.
    let (bindings, donors) =
        resent_part_bindings(evidence, &survivor_of, constraints.resent_part_binding);
    for sequence in bindings {
        for pair in sequence.windows(2) {
            intra_edges.push((pair[0], pair[1]));
            uf.union(pair[0], pair[1]);
        }
    }

    let unit_of: Vec<usize> = (0..n).map(|i| uf.find(i)).collect();

    // Priority per unit: the earliest time the *evidence* gives it. Time seeds the topological pop;
    // it never forces an order an edge does not.
    //
    // Taken from the pre-dedup occurrences, not from the survivors. A survivor's `timestamp` has
    // already been overwritten by `process_dedup` with its old batch anchor, so reading it here would
    // make the new order a function of the order it is replacing - and would carry the very
    // copy-survival dependence this redesign exists to remove: whichever copy won would decide the
    // anchor. Only a credible emission counts as evidence of a time (a re-listed snapshot's time is
    // when it was assembled), with the survivor's own effective time as the fallback where an
    // identity has no emission at all - a user message read from an attribute array, say.
    //
    // The fallback is the minimum effective time over **every observation** projected to the unit,
    // never the chosen survivor's alone. The survivor's copy is picked by quality and rescue rules
    // that legitimately change - the history rescue keeps the *earliest* candidate today, and a rule
    // change there must not move an unrelated block. With two identical user copies at t=10 and t=30
    // and an unrelated credible unit at t=20, reading the survivor's time put the user before or
    // after it depending on which copy was rescued; the minimum over both is the same fact whichever
    // copy wins.
    let mut unit_priority: HashMap<usize, DateTime<Utc>> = HashMap::new();
    let record = |unit: usize, time: DateTime<Utc>, map: &mut HashMap<usize, DateTime<Utc>>| {
        map.entry(unit)
            .and_modify(|t| {
                if time < *t {
                    *t = time;
                }
            })
            .or_insert(time);
    };
    let mut from_emission: HashMap<usize, DateTime<Utc>> = HashMap::new();
    let mut from_any_observation: HashMap<usize, DateTime<Utc>> = HashMap::new();
    let mut from_direct_observation: HashMap<usize, DateTime<Utc>> = HashMap::new();
    for (observation, seen) in evidence.iter().enumerate() {
        let Some(survivor) = survivor_of(observation) else {
            continue;
        };
        record(unit_of[survivor], seen.effective, &mut from_any_observation);
        if !seen.history {
            record(
                unit_of[survivor],
                seen.effective,
                &mut from_direct_observation,
            );
        }
        if !seen.credible {
            continue;
        }
        // A redundant re-listing's time is when it was assembled, not when anything happened - the
        // descendant that produced the message has the occurrence.
        if seen
            .emission
            .is_some_and(|instance| redundant.contains(&instance))
        {
            continue;
        }
        record(unit_of[survivor], seen.effective, &mut from_emission);
    }
    for (i, block) in survivors.iter().enumerate() {
        let unit = unit_of[i];
        let time = from_emission
            .get(&unit)
            .copied()
            // A history copy's time is when it was assembled - an agent re-listing its run carries the
            // run's start - so it speaks only for a unit nothing observed directly. Which observations are
            // history is settled before dedup, so this does not depend on the copy that survived.
            .or_else(|| from_direct_observation.get(&unit).copied())
            .or_else(|| from_any_observation.get(&unit).copied())
            .unwrap_or_else(|| effective_timestamp(block, span_timestamps));
        unit_priority
            .entry(unit)
            .and_modify(|t| {
                if time < *t {
                    *t = time;
                }
            })
            .or_insert(time);
    }
    // The smallest legacy index in each unit: the neutrality seed, and the tie-break of last resort.
    let mut unit_min_legacy: HashMap<usize, usize> = HashMap::new();
    for (i, &unit) in unit_of.iter().enumerate() {
        unit_min_legacy
            .entry(unit)
            .and_modify(|m| *m = (*m).min(i))
            .or_insert(i);
    }

    // Where each unit was *first observed*, over every observation that projects to it.
    //
    // This is the tie-break that survivor choice cannot move. The legacy index is the position of the
    // *surviving* block in the previous sort, so two units with nothing ordering them were separated by
    // which copy happened to win dedup - which is the dependence this redesign exists to remove, and
    // what makes `adk/tool_use` group its tool calls in two traces and interleave them in a third.
    // First observation is a property of the evidence: it considers every copy, so it does not change
    // when a different one survives.
    let mut unit_first_seen: HashMap<usize, usize> = HashMap::new();
    for (observation, _) in evidence.iter().enumerate() {
        let Some(survivor) = survivor_of(observation) else {
            continue;
        };
        unit_first_seen
            .entry(unit_of[survivor])
            .and_modify(|m| *m = (*m).min(observation))
            .or_insert(observation);
    }

    // Project exact pre-dedup call/result evidence onto the contracted ordering units.
    let exact_tool_edges: std::collections::HashSet<(usize, usize)> =
        exact_tool_pairs(evidence, survivors, lineage, repeat_ordinals)
            .into_iter()
            .filter_map(|(call, result)| {
                (unit_of[call] != unit_of[result]).then_some((unit_of[call], unit_of[result]))
            })
            .collect();
    let units: Vec<usize> = {
        let mut u: Vec<usize> = unit_priority.keys().copied().collect();
        u.sort_unstable();
        u
    };
    // Under the scaffold the seed is the legacy index alone (`None` sorts before any `Some`, so the
    // time term drops out entirely): that is what makes the resolve reproduce the legacy order rather
    // than merely agree with it on this corpus. Promoting time to the primary key is its own delta.
    let key = |u: usize,
               unit_priority: &HashMap<usize, DateTime<Utc>>,
               unit_min_legacy: &HashMap<usize, usize>| {
        let primary = if constraints.time_priority {
            Some(unit_priority[&u])
        } else {
            None
        };
        // Under `NEUTRAL` the legacy index alone decides, which is what makes the neutrality proof
        // hold. Otherwise first-observation breaks the tie and the legacy index is the last resort,
        // for a unit no observation projects to.
        let secondary = if constraints.time_priority {
            unit_first_seen
                .get(&u)
                .copied()
                .unwrap_or(unit_min_legacy[&u])
        } else {
            unit_min_legacy[&u]
        };
        (primary, secondary, u)
    };

    // Keys first, because the dataflow class needs one for the barrier node it introduces. Computed
    // once per unit rather than per edge: rebuilding a key on every indegree decrement made the resolve
    // cost scale with the edge count, which is what the barrier exists to bound.
    let mut keys_of_units: HashMap<usize, PopKey> = units
        .iter()
        .map(|&u| (u, key(u, &unit_priority, &unit_min_legacy)))
        .collect();
    let mut barriers: Vec<usize> = Vec::new();

    let mut successors: HashMap<usize, Vec<usize>> =
        units.iter().map(|&u| (u, Vec::new())).collect();
    let mut indegree: HashMap<usize, usize> = units.iter().map(|&u| (u, 0)).collect();
    let mut edges: std::collections::HashSet<(usize, usize)> = std::collections::HashSet::new();
    #[expect(clippy::iter_over_hash_type, reason = "ready set is ordered")]
    for &(call_unit, result_unit) in &exact_tool_edges {
        add_edge(
            call_unit,
            result_unit,
            constraints,
            &unit_min_legacy,
            &mut successors,
            &mut indegree,
            &mut edges,
        );
    }

    // Carrier sequence: the order a payload states between its own surviving observations.
    //
    // Adjacent pairs only; the transitive closure adds nothing to a topological order. Ordered by the
    // payload position, which is what `message_index`/`entry_index` carry for a carrier's blocks.
    if constraints.carrier_sequence_edges {
        let mut by_carrier: HashMap<usize, Vec<(i32, i32, usize)>> = HashMap::new();
        let own_outputs = own_outputs(evidence, &survivor_of);
        for (observation, seen) in evidence.iter().enumerate() {
            if !seen.carrier_ordered || own_outputs.contains(&(observation, seen.span)) {
                continue;
            }
            // Nor does it state a sequence: it is the *order* a re-listing gets wrong, so taking its
            // positions as edges is taking the one thing it has no authority over.
            if seen
                .emission
                .is_some_and(|instance| redundant.contains(&instance))
            {
                continue;
            }
            let Some(survivor) = survivor_of(observation) else {
                continue;
            };
            by_carrier.entry(seen.carrier).or_default().push((
                seen.message_index,
                seen.entry_index,
                survivor,
            ));
        }
        let mut carriers: Vec<&usize> = by_carrier.keys().collect();
        carriers.sort_unstable();
        let carriers: Vec<usize> = carriers.into_iter().copied().collect();
        for carrier in carriers {
            let mut members = by_carrier.remove(&carrier).unwrap_or_default();
            members.sort_unstable();
            let mut sequence: Vec<usize> = Vec::with_capacity(members.len());
            for (_, _, survivor) in members {
                let unit = unit_of[survivor];
                if sequence.last() != Some(&unit) {
                    sequence.push(unit);
                }
            }
            for pair in sequence.windows(2) {
                add_edge(
                    pair[0],
                    pair[1],
                    constraints,
                    &unit_min_legacy,
                    &mut successors,
                    &mut indegree,
                    &mut edges,
                );
            }
        }
    }

    // Generation dataflow: what a model call received precedes what it produced.
    //
    // Read from the evidence rather than from the survivors, because the copy on display often comes
    // from a different span - the answer a chain span re-lists is still the generation's output, and
    // the system prompt a generation received is still its input even when the surviving copy of it
    // was read somewhere else.
    //
    // Only edges between *different* units are added, and an edge already implied by contraction is
    // skipped. Under the scaffold an edge the legacy order has backwards is dropped, as everywhere.
    if constraints.generation_dataflow_edges {
        // Ordered sets, not vectors with a membership scan: a generation span re-sending a long
        // history has hundreds of input observations mapping to a handful of units, and checking each
        // against a growing `Vec` made collecting them quadratic in the observation count. `BTreeSet`
        // also fixes the iteration order, which a `HashSet` would leave to chance.
        let mut inputs_by_span: HashMap<usize, BTreeSet<usize>> = HashMap::new();
        let mut outputs_by_span: HashMap<usize, BTreeSet<usize>> = HashMap::new();
        for (observation, seen) in evidence.iter().enumerate() {
            if !seen.from_generation {
                continue;
            }
            // History is *kept* on the input side here, unlike carrier-sequence edges. The two ask
            // different questions of a re-send. "These messages precede what this generation produced"
            // is true of a replayed input and is exactly the evidence that pins a tool result before
            // the answer that cites it - no other class states that, because the consuming span is the
            // only place the two meet. "These messages are in this relative order globally" is *not*
            // true of a replay, which is what dragged ADK's second system prompt to the front of a
            // session, so that reading stays gated there.
            let Some(survivor) = survivor_of(observation) else {
                continue;
            };
            let side = if seen.is_output {
                &mut outputs_by_span
            } else {
                &mut inputs_by_span
            };
            side.entry(seen.span).or_default().insert(unit_of[survivor]);
        }
        let mut spans: Vec<&usize> = inputs_by_span.keys().collect();
        spans.sort_unstable();
        let spans: Vec<usize> = spans.into_iter().copied().collect();
        // A bound on the span indices that may get a barrier, so the two generations occupy disjoint ranges
        // above the survivor indices.
        let span_upper = inputs_by_span
            .keys()
            .chain(outputs_by_span.keys())
            .max()
            .map_or(0, |m| m + 1);
        for span in spans {
            let Some(outputs) = outputs_by_span.get(&span) else {
                continue;
            };
            let inputs = &inputs_by_span[&span];

            // **Overlapping sets get two barriers, not a product.** For inputs `{u, a}` and outputs
            // `{u, b}` the product contains `u -> b`, and one barrier cannot express that without also
            // asserting `u -> barrier -> u`. That is what kept the pairwise form here, and the pairwise
            // form has two problems the barrier construction does not.
            //
            // It is unbounded: a span re-sending a long history has hundreds of inputs, so the product is
            // quadratic in a span's message count - the exact growth the barrier was introduced to remove,
            // still reachable through this branch.
            //
            // And for two or more shared units it is **self-contradictory**. The relation it builds
            // contains `u -> v` *and* `v -> u` for every pair of shared units, so the resolver breaks a
            // cycle this code manufactured, and the order after that is a deterministic guess rather than
            // a derived one.
            //
            // Split the sets instead: `shared` is what the span both received and produced, `only_in` and
            // `only_out` the rest. The consistent part of the desired relation is
            // `only_in x only_out`, `only_in x shared` and `shared x only_out` - everything except the
            // shared-to-shared pairs, which are the contradictory ones. Two barriers express exactly that:
            //
            //   - `inbound`: every input (shared included) precedes it, and it precedes every pure output,
            //     giving `(only_in ∪ shared) x only_out`;
            //   - `outbound`: every *pure* input precedes it, and it precedes every shared unit, giving
            //     `only_in x shared`.
            //
            // That is `inputs + only_out + only_in + shared` edges - linear - and it omits only the pairs
            // that cannot all hold. Omitting them is the honest reading rather than a concession: a unit
            // this span both received and produced is a replay, and one span's dataflow says nothing about
            // where two such units sit relative to each other.
            //
            // With exactly one shared unit the omitted set is empty, so the two constructions agree
            // *exactly* - which is what `a_barrier_orders_exactly_as_pairwise_edges_do` can compare. Beyond
            // one they differ only where pairwise was already contradicting itself.
            let opposes_exact_tool_edge = exact_tool_edges
                .iter()
                .any(|(call, result)| outputs.contains(call) && inputs.contains(result));
            if constraints.pairwise_dataflow_edges || opposes_exact_tool_edge {
                for &input in inputs {
                    for &output in outputs {
                        if exact_tool_edges.contains(&(output, input)) {
                            continue;
                        }
                        add_edge(
                            input,
                            output,
                            constraints,
                            &unit_min_legacy,
                            &mut successors,
                            &mut indegree,
                            &mut edges,
                        );
                    }
                }
                continue;
            }

            // The consistent part of "everything received precedes everything produced", split so the
            // shared units are ordered against the rest without being ordered against each other.
            let shared: Vec<usize> = inputs
                .iter()
                .copied()
                .filter(|u| outputs.contains(u))
                .collect();
            let only_out: Vec<usize> = outputs
                .iter()
                .copied()
                .filter(|u| !inputs.contains(u))
                .collect();
            let only_in: Vec<usize> = inputs
                .iter()
                .copied()
                .filter(|u| !outputs.contains(u))
                .collect();

            // A barrier's key is the smallest key among the units it precedes, so it is popped exactly when
            // the earliest of them would have been: it emits nothing and the resulting order is unchanged.
            let install = |generation: usize,
                           before: &[usize],
                           after: &[usize],
                           keys_of_units: &mut HashMap<usize, PopKey>,
                           unit_min_legacy: &mut HashMap<usize, usize>,
                           successors: &mut HashMap<usize, Vec<usize>>,
                           indegree: &mut HashMap<usize, usize>,
                           edges: &mut std::collections::HashSet<(usize, usize)>,
                           barriers: &mut Vec<usize>| {
                if before.is_empty() || after.is_empty() {
                    return;
                }
                let barrier = barrier_unit(span, survivors.len(), span_upper, generation);
                let key = after
                    .iter()
                    .filter_map(|u| keys_of_units.get(u).copied())
                    .min();
                let legacy = after
                    .iter()
                    .filter_map(|u| unit_min_legacy.get(u).copied())
                    .min();
                let (Some(key), Some(legacy)) = (key, legacy) else {
                    return;
                };
                keys_of_units.insert(barrier, key);
                unit_min_legacy.insert(barrier, legacy);
                successors.entry(barrier).or_default();
                indegree.entry(barrier).or_insert(0);
                barriers.push(barrier);
                let snapshot = unit_min_legacy.clone();
                for &from in before {
                    add_edge(
                        from,
                        barrier,
                        constraints,
                        &snapshot,
                        successors,
                        indegree,
                        edges,
                    );
                }
                for &to in after {
                    add_edge(
                        barrier,
                        to,
                        constraints,
                        &snapshot,
                        successors,
                        indegree,
                        edges,
                    );
                }
            };

            let all_inputs: Vec<usize> = inputs.iter().copied().collect();
            // `(only_in ∪ shared) -> only_out`.
            install(
                0,
                &all_inputs,
                &only_out,
                &mut keys_of_units,
                &mut unit_min_legacy,
                &mut successors,
                &mut indegree,
                &mut edges,
                &mut barriers,
            );
            // `only_in -> shared`. Absent where nothing is shared, which is every corpus span.
            install(
                1,
                &only_in,
                &shared,
                &mut keys_of_units,
                &mut unit_min_legacy,
                &mut successors,
                &mut indegree,
                &mut edges,
                &mut barriers,
            );
        }
    }

    // Request framing: a detached frame precedes the inputs *first seen* in its own request.
    //
    // A framework reports the system instruction on the span that *sent* it - the generation span -
    // while the question arrived on an orchestration span that started earlier, so ordering by evidence
    // time put the frame after the question. 27 trace views across 22 fixtures, every one displaced by
    // exactly one position (`a_system_instruction_precedes_the_first_user_turn`).
    //
    // The request is the generation span that carried the frame; the fact is the carrier's
    // (`carrier_is_detached_request_frame`), never the role's, since `developer` normalises to System
    // and an in-band system message is a turn in its array. Both sides derive from *all* pre-dedup
    // observations projected through lineage, like the dataflow class above, so the surviving copy of
    // the question inherits the edge wherever it sits - reading the survivor's own span instead would
    // let a quality tie decide which request gets the edge, which
    // `which_copy_survives_does_not_change_the_order` forbids.
    //
    // Two exclusions on the target side, and the first was learned from a regression rather than
    // deduced. "The frame precedes this input" is true *within one request's payload* and false as a
    // global statement about a replay: `agent-framework/swarm` hands the conversation through four
    // agents, each re-sending it under a new instruction, and framing every input dragged all four
    // instructions to the front of the trace. So a frame precedes only what its request saw **first** -
    // an input already listed by an earlier request belongs to that turn, and an earlier generation's
    // *output* precedes this frame by conversation order however this request re-consumed it.
    if constraints.request_framing_edges {
        let mut frames_by_span: HashMap<usize, BTreeSet<usize>> = HashMap::new();
        let mut inputs_by_span: HashMap<usize, BTreeSet<usize>> = HashMap::new();
        let mut generation_outputs: BTreeSet<usize> = BTreeSet::new();
        let mut span_first_effective: HashMap<usize, DateTime<Utc>> = HashMap::new();
        // The ordered members of each input array, per (span, family): `(message, entry, unit)`.
        type ArrayMembers = HashMap<usize, Vec<EmissionMember>>;
        let mut arrays_by_span: HashMap<usize, ArrayMembers> = HashMap::new();
        for (observation, seen) in evidence.iter().enumerate() {
            if !seen.from_generation {
                continue;
            }
            let Some(survivor) = survivor_of(observation) else {
                continue;
            };
            let unit = unit_of[survivor];
            if seen.is_output {
                generation_outputs.insert(unit);
                continue;
            }
            span_first_effective
                .entry(seen.span)
                .and_modify(|t| {
                    if seen.effective < *t {
                        *t = seen.effective;
                    }
                })
                .or_insert(seen.effective);
            if let Some(family) = seen.input_family {
                arrays_by_span
                    .entry(seen.span)
                    .or_default()
                    .entry(family)
                    .or_default()
                    .push((seen.message_index, seen.entry_index, unit));
            }
            let side = if seen.detached_frame {
                &mut frames_by_span
            } else {
                &mut inputs_by_span
            };
            side.entry(seen.span).or_default().insert(unit);
        }
        // Requests in the order they happened, so "first seen" is well defined; the span id breaks a
        // timestamp tie deterministically.
        let mut requests: Vec<usize> = frames_by_span
            .keys()
            .chain(inputs_by_span.keys())
            .copied()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        requests.sort_by_key(|span| (span_first_effective.get(span).copied(), *span));

        let mut already_seen: BTreeSet<usize> = BTreeSet::new();
        // Requests that only carry arrays still take part in first-seen accounting.
        #[expect(clippy::iter_over_hash_type, reason = "sorted right after")]
        for span in arrays_by_span.keys() {
            if !requests.contains(span) {
                requests.push(*span);
            }
        }
        requests.sort_by_key(|span| (span_first_effective.get(span).copied(), *span));
        for span in requests {
            let inputs = inputs_by_span.get(&span).cloned().unwrap_or_default();
            // An ordered input array orders the request's **first-seen** members. The array's own
            // positions are the evidence, history included - the framing block projects everything
            // through lineage - and first-seen is what neutralises a replay: a member already listed by
            // an earlier request belongs to that turn, and an earlier generation's output keeps its
            // conversation position, so a changed instruction at position 0 of a replaying array
            // (`adk/image_gen`'s critic) frames only its own request's new turns. This is what lets the
            // array order a frame that lives *inside* it - langgraph's `llm.input_messages` carries
            // `system@0, user@1`, and extraction fragments that array into per-index carriers, so the
            // ordinary carrier-sequence class never sees it whole.
            if let Some(families) = arrays_by_span.get(&span) {
                let mut family_ids: Vec<&usize> = families.keys().collect();
                family_ids.sort_unstable();
                for family in family_ids {
                    let mut members = families[family].clone();
                    members.sort_unstable();
                    let mut sequence: Vec<usize> = Vec::new();
                    // A set beside the vector, because `Vec::contains` made this quadratic in the
                    // array's unique members and a replaying array re-lists the whole conversation.
                    let mut in_sequence: BTreeSet<usize> = BTreeSet::new();
                    for (_, _, unit) in members {
                        if already_seen.contains(&unit)
                            || generation_outputs.contains(&unit)
                            || !in_sequence.insert(unit)
                        {
                            continue;
                        }
                        sequence.push(unit);
                    }
                    for pair in sequence.windows(2) {
                        add_edge(
                            pair[0],
                            pair[1],
                            constraints,
                            &unit_min_legacy,
                            &mut successors,
                            &mut indegree,
                            &mut edges,
                        );
                    }
                }
            }
            if let Some(frames) = frames_by_span.get(&span) {
                for &frame in frames {
                    for &input in &inputs {
                        if frames.contains(&input)
                            || generation_outputs.contains(&input)
                            || already_seen.contains(&input)
                        {
                            continue;
                        }
                        add_edge(
                            frame,
                            input,
                            constraints,
                            &unit_min_legacy,
                            &mut successors,
                            &mut indegree,
                            &mut edges,
                        );
                    }
                }
            }
            already_seen.extend(inputs);
        }
    }

    // Sibling tool turns: recover causal order when a runtime quantises the tool execution and its
    // terminal generation to the same start timestamp.
    //
    // This is intentionally a shape proof, not a role heuristic. The relation is added only for one
    // direct-sibling turn with exactly two generation outputs: one generation ends in ToolUse and
    // another ends in Stop. Exact call/result ids must pair uniquely, live on the same tool span and
    // have the same start as the terminal generation. The preamble may share that millisecond or be
    // in an earlier one.
    // That states:
    //
    //     tool-use preamble -> each call -> its result -> terminal answer
    //
    // Multiple tool spans are fine, but an extra generation output, duplicate id, split call/result
    // pair, retry or parallel branch makes the wave ambiguous and therefore leaves it unconstrained.
    if constraints.sibling_tool_turn_edges {
        #[derive(Default)]
        struct SiblingTurn<'a> {
            generation_output_spans: BTreeSet<&'a str>,
            preamble_spans: BTreeSet<&'a str>,
            preamble_units: BTreeSet<usize>,
            terminal_spans: BTreeSet<&'a str>,
            terminal_units: BTreeSet<usize>,
            calls_by_id: HashMap<&'a str, Vec<(&'a str, usize)>>,
            results_by_id: HashMap<&'a str, Vec<(&'a str, usize)>>,
        }

        let mut turns: HashMap<&str, SiblingTurn<'_>> = HashMap::new();
        for (i, block) in survivors.iter().enumerate() {
            let Some(parent) = block.parent_span_id.as_deref() else {
                continue;
            };
            let turn = turns.entry(parent).or_default();
            let unit = unit_of[i];

            if block.is_generation_span() && block.is_output_source() {
                turn.generation_output_spans.insert(&block.span_id);
                if block.role == ChatRole::Assistant
                    && block.entry_type == "text"
                    && block.finish_reason == Some(FinishReason::ToolUse)
                {
                    turn.preamble_spans.insert(&block.span_id);
                    turn.preamble_units.insert(unit);
                }
                if block.role == ChatRole::Assistant
                    && block.entry_type == "text"
                    && block.finish_reason == Some(FinishReason::Stop)
                {
                    turn.terminal_spans.insert(&block.span_id);
                    turn.terminal_units.insert(unit);
                }
            }

            if !block.is_tool_span() {
                continue;
            }
            let Some(id) = block.tool_use_id.as_deref().filter(|id| !id.is_empty()) else {
                continue;
            };
            if block.entry_type == "tool_use" {
                turn.calls_by_id
                    .entry(id)
                    .or_default()
                    .push((&block.span_id, unit));
            } else if block.entry_type == "tool_result" {
                turn.results_by_id
                    .entry(id)
                    .or_default()
                    .push((&block.span_id, unit));
            }
        }

        let mut parents: Vec<&str> = turns.keys().copied().collect();
        parents.sort();
        for parent in parents {
            let turn = turns.remove(parent).expect("parent key came from map");
            if turn.generation_output_spans.len() != 2
                || turn.preamble_spans.len() != 1
                || turn.terminal_spans.len() != 1
                || turn.preamble_spans == turn.terminal_spans
                || turn.preamble_units.is_empty()
                || turn.terminal_units.is_empty()
            {
                continue;
            }

            let preamble_span = *turn
                .preamble_spans
                .first()
                .expect("one preamble span was required");
            let terminal_span = *turn
                .terminal_spans
                .first()
                .expect("one terminal span was required");
            let Some(preamble_start) = span_timestamps
                .get(preamble_span)
                .map(|timestamps| timestamps.span_start)
            else {
                continue;
            };
            let Some(terminal_start) = span_timestamps
                .get(terminal_span)
                .map(|timestamps| timestamps.span_start)
            else {
                continue;
            };
            if preamble_start > terminal_start {
                continue;
            }

            if turn.calls_by_id.is_empty()
                || turn.calls_by_id.len() != turn.results_by_id.len()
                || turn
                    .results_by_id
                    .keys()
                    .any(|id| !turn.calls_by_id.contains_key(id))
            {
                continue;
            }

            let mut pairs: Vec<(usize, usize)> = Vec::new();
            let mut ambiguous = false;
            let mut ids: Vec<&str> = turn.calls_by_id.keys().copied().collect();
            ids.sort();
            for id in ids {
                let calls = &turn.calls_by_id[id];
                let Some(results) = turn.results_by_id.get(id) else {
                    ambiguous = true;
                    break;
                };
                if calls.len() != 1 || results.len() != 1 || calls[0].0 != results[0].0 {
                    ambiguous = true;
                    break;
                }
                let Some(tool_start) = span_timestamps
                    .get(calls[0].0)
                    .map(|timestamps| timestamps.span_start)
                else {
                    ambiguous = true;
                    break;
                };
                if tool_start != terminal_start {
                    ambiguous = true;
                    break;
                }
                pairs.push((calls[0].1, results[0].1));
            }
            if ambiguous || pairs.is_empty() {
                continue;
            }

            for &(call, result) in &pairs {
                for &preamble in &turn.preamble_units {
                    add_edge(
                        preamble,
                        call,
                        constraints,
                        &unit_min_legacy,
                        &mut successors,
                        &mut indegree,
                        &mut edges,
                    );
                }
                for &terminal in &turn.terminal_units {
                    add_edge(
                        result,
                        terminal,
                        constraints,
                        &unit_min_legacy,
                        &mut successors,
                        &mut indegree,
                        &mut edges,
                    );
                }
            }
        }
    }

    // Kahn's algorithm, popping the ready unit with the smallest (priority, min-legacy, unit-id).
    // On a stall - a cycle - break it deterministically by the same key over the remaining units,
    // so the resolver is total rather than panicking. (Full SCC condensation is a later increment.
    // Two corpus fixtures *do* cycle - `ordering_contradictions_are_pinned` holds the set - and
    // both land on the correct order under this release rule.)
    let mut order: Vec<usize> = Vec::with_capacity(units.len());
    // Two ordered sets rather than a rescan. The loop used to look at every unit on every iteration
    // to find the ready ones, which is quadratic in the number of units - and a session view can hold
    // thousands. `ready` yields the next unit in O(log n); `remaining` exists only for the cycle case,
    // where nothing is ready and the smallest key has to be released to keep the resolve total.
    // Barriers are nodes too: they carry no blocks, but they have to be popped for their outputs to
    // become ready.
    let nodes: Vec<usize> = units.iter().copied().chain(barriers).collect();
    let keys = keys_of_units;

    let mut ready: BTreeSet<(PopKey, usize)> = BTreeSet::new();
    let mut remaining: BTreeSet<(PopKey, usize)> = BTreeSet::new();
    for &u in &nodes {
        let entry = (keys[&u], u);
        remaining.insert(entry);
        if indegree[&u] == 0 {
            ready.insert(entry);
        }
    }

    // Cycles released to keep the resolve total. A cycle means the *evidence* contradicts itself, which
    // is a fact about the telemetry or about a constraint class, and it used to be resolved in silence -
    // so nothing distinguished "the order is derived" from "the order is a deterministic guess after a
    // contradiction". Counted here and reported once below, rather than per release, because one
    // contradiction commonly strands several units.
    let mut cycles_broken = 0usize;

    while order.len() < nodes.len() {
        let next_entry = match ready.iter().next().copied() {
            Some(entry) => entry,
            // A cycle: the evidence contradicts itself. Release the smallest-key remaining unit so the
            // result is still a total order.
            None => {
                cycles_broken += 1;
                #[cfg(any(test, feature = "test-support"))]
                CYCLES_BROKEN_IN_TESTS.with(|c| *c.borrow_mut() += 1);
                remaining
                    .iter()
                    .next()
                    .copied()
                    .expect("a node remains while the order is incomplete")
            }
        };
        ready.remove(&next_entry);
        remaining.remove(&next_entry);
        let next = next_entry.1;
        order.push(next);
        for &s in &successors[&next] {
            if let Some(d) = indegree.get_mut(&s) {
                *d = d.saturating_sub(1);
                if *d == 0 {
                    let entry = (keys[&s], s);
                    if remaining.contains(&entry) {
                        ready.insert(entry);
                    }
                }
            }
        }
    }

    if cycles_broken > 0 {
        tracing::warn!(
            cycles_broken,
            units = units.len(),
            blocks = n,
            trace_id = survivors.first().map(|b| b.trace_id.as_str()),
            "ordering evidence contradicts itself; released units deterministically to keep the order \
             total - the result is a consistent guess rather than a derived order"
        );
    }

    // Emit each unit's members, ordered by the emissions' own adjacency.
    let mut members_of: HashMap<usize, Vec<usize>> =
        units.iter().map(|&u| (u, Vec::new())).collect();
    for (i, &unit) in unit_of.iter().enumerate() {
        members_of.get_mut(&unit).expect("unit present").push(i);
    }
    // The edges bucketed by unit, once, instead of the whole list rescanned per unit.
    //
    // `order_within_unit` filtered `intra_edges` itself, so the scan was O(units x edges) - a session with
    // thousands of units and thousands of edges rescanned everything for each. Bucketing reproduces exactly
    // the set each unit's filter kept: an edge whose endpoints sit in different units was skipped by that
    // filter and is not bucketed under either.
    let mut edges_of: HashMap<usize, Vec<(usize, usize)>> = HashMap::new();
    if constraints.source_position_member_order {
        for &(from, to) in &intra_edges {
            match (unit_of.get(from), unit_of.get(to)) {
                (Some(from_unit), Some(to_unit)) if from_unit == to_unit => {
                    edges_of.entry(*from_unit).or_default().push((from, to));
                }
                _ => {}
            }
        }
    }

    let mut out = Vec::with_capacity(n);
    for unit in order {
        let mut members = members_of.remove(&unit).unwrap_or_default();
        members.sort_unstable();
        if constraints.source_position_member_order {
            let unit_edges = edges_of.remove(&unit).unwrap_or_default();
            members = order_within_unit(&members, &unit_edges);
        }
        for i in members {
            out.push(at_home(
                &survivors[i],
                donors.get(&i).map(|&d| &survivors[d]),
            ));
        }
    }
    out
}
