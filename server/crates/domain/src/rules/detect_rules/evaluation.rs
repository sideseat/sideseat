use super::*;

impl CompiledDetect {
    /// Whether the rule's condition holds of this span.
    pub(super) fn matches(&self, ctx: &DetectContext<'_>) -> bool {
        span_conditions::holds(
            &self.condition,
            &span_conditions::SpanSubject {
                span_name: ctx.span_name,
                attrs: ctx.span_attrs,
                scope_name: ctx.scope_name,
                resource: Some(ctx.resource_attrs),
            },
        )
    }
}

impl DetectPlan {
    /// The rule that claims this span: the first that matches, by priority.
    ///
    /// Where more than one rule matches, the priority is what decides, and the alternatives are recoverable
    /// through [`Self::overlapping_candidates`] - the design's target is that no span has two, and the only way
    /// to get there is to be able to see which spans do. `supersedes` takes no part: compilation has checked
    /// that every edge agrees with the priorities.
    pub fn resolve(&self, ctx: &DetectContext<'_>) -> Option<&CompiledDetect> {
        self.rules.iter().find(|rule| rule.matches(ctx))
    }

    /// The retired resolution, for the oracle that states what the precedence migration changed: the first
    /// matching clause, by the frozen rank, that no other matching clause beats through the frozen and
    /// transitively closed `supersedes` edges. `retired` is `(clause id, rank, edges)` as the assets stated them;
    /// a current clause split out of a retired one matches as that one did (`origin`: split id, retired id), so
    /// the retired plan sees the predicates it had. The table lives with the test that freezes it, since it
    /// names producers.
    #[cfg(any(test, feature = "test-support"))]
    pub fn retired_resolve(
        &self,
        ctx: &DetectContext<'_>,
        retired: &[(&str, i32, &[&str])],
        origin: &[(&str, &str)],
    ) -> Option<&str> {
        let origin_of = |id: &str| -> String {
            origin
                .iter()
                .find(|(split, _)| *split == id)
                .map_or(id, |(_, from)| from)
                .to_string()
        };
        let matching: std::collections::BTreeSet<String> = self
            .rules
            .iter()
            .filter(|rule| rule.matches(ctx))
            .map(|rule| origin_of(&rule.rule_id))
            .collect();
        let edges = |id: &str| -> Vec<&str> {
            retired
                .iter()
                .find(|(clause, ..)| *clause == id)
                .map_or_else(Vec::new, |(_, _, edges)| edges.to_vec())
        };
        let mut beaten: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
        for id in &matching {
            let mut queue: Vec<&str> = edges(id);
            while let Some(target) = queue.pop() {
                if beaten.insert(target) {
                    queue.extend(edges(target));
                }
            }
        }
        let mut ordered: Vec<(i32, &str)> = retired
            .iter()
            .filter(|(id, ..)| matching.contains(*id))
            .map(|(id, rank, _)| (*rank, *id))
            .collect();
        ordered.sort();
        let winner = ordered
            .iter()
            .find(|(_, id)| !beaten.contains(id))
            .or(ordered.first())?
            .1;
        self.rules
            .iter()
            .find(|rule| rule.rule_id == winner)
            .map(|rule| rule.label.as_str())
    }

    /// Why nothing was attributed: the rules that read a key this span **has**, and disagreed about its value.
    ///
    /// "No rule answered" is a bare `None`, which is the answer an operator most often needs evidence for - and
    /// there was none anywhere. A full explain trace over every rule would bury it; what identifies the common
    /// failure is much narrower: a producer writes the attribute a rule reads, with a value the rule does not know.
    /// That is exactly an unrecognised producer conforming to a convention, and the operator's next step is to
    /// declare the value.
    ///
    /// Sound and cheap by construction: only conditions naming a *key* participate, and only where the span
    /// carries that key. A rule asking about a namespace this span has nothing in is not a near miss and is not
    /// reported - reporting every rule is what makes an explanation useless.
    pub fn near_misses(&self, ctx: &DetectContext<'_>) -> Vec<NearMiss> {
        // Nothing to explain where something answered - and asked here rather than left to the caller, because a
        // function that reports six disagreements about a span it attributed correctly is a function whose
        // usefulness depends on where it is called from.
        if self.resolve(ctx).is_some() {
            return Vec::new();
        }
        let mut out: Vec<NearMiss> = Vec::new();
        for rule in &self.rules {
            for atom in span_conditions::positive_atoms(&rule.condition) {
                let mut push = |carrier: &str, expected: String, found: &String| {
                    out.push(NearMiss {
                        rule_id: rule.rule_id.clone(),
                        label: rule.label.clone(),
                        carrier: carrier.to_string(),
                        expected,
                        found: found.clone(),
                    });
                };
                match atom {
                    span_conditions::SpanAtom::SpanAttrEquals { key, value }
                    | span_conditions::SpanAtom::SpanAttrEqualsIgnoreAsciiCase { key, value } => {
                        if let Some(found) = ctx.span_attrs.get(key)
                            && !found.eq_ignore_ascii_case(value)
                        {
                            push(key, value.clone(), found);
                        }
                    }
                    span_conditions::SpanAtom::SpanAttrContains { key, value } => {
                        if let Some(found) = ctx.span_attrs.get(key)
                            && !found.contains(value.as_str())
                        {
                            push(key, format!("containing `{value}`"), found);
                        }
                    }
                    span_conditions::SpanAtom::ResourceAttrContains { key, value } => {
                        if let Some(found) = ctx.resource_attrs.get(key)
                            && !found.contains(value.as_str())
                        {
                            push(key, format!("containing `{value}`"), found);
                        }
                    }
                    _ => {}
                }
            }
        }
        out
    }

    /// Every rule that matches, in priority order.
    ///
    /// The instrument for narrowing the predicates: a span with one candidate needs no ordering, and one
    /// with several names exactly which predicates are not yet sufficient. A rule that `supersedes`
    /// another is not reported against it, because that overlap is already owned.
    pub fn overlapping_candidates<'p>(
        &'p self,
        ctx: &DetectContext<'_>,
    ) -> Vec<&'p CompiledDetect> {
        let matching: Vec<&CompiledDetect> =
            self.rules.iter().filter(|rule| rule.matches(ctx)).collect();
        if matching.len() < 2 {
            return Vec::new();
        }
        // **Transitively** dominated, not only directly. Every edge agrees with priority, so the edges form a
        // DAG, and a rule that supersedes a rule which supersedes a third owns that overlap too - reading direct edges only reported an overlap
        // whose ordering is in fact declared, which is noise in the one instrument meant to say where ordering
        // is *not* yet declared.
        let dominated = self.dominated_by(matching[0].rule_id.as_str());
        let contested: Vec<&CompiledDetect> = matching
            .iter()
            .skip(1)
            .filter(|other| !dominated.contains(other.rule_id.as_str()))
            .copied()
            .collect();
        if contested.is_empty() {
            return Vec::new();
        }
        matching
    }

    /// Every rule the named one supersedes, directly or through another.
    ///
    /// The transitive closure: a rule that supersedes one which supersedes a third has documented its overlap with
    /// all of them. Every edge points to a later priority, so this terminates.
    fn dominated_by(&self, rule_id: &str) -> std::collections::BTreeSet<&str> {
        let mut out: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
        let mut queue: Vec<&str> = vec![rule_id];
        while let Some(current) = queue.pop() {
            let Some(rule) = self
                .rules
                .iter()
                .find(|candidate| candidate.rule_id == current)
            else {
                continue;
            };
            for target in &rule.supersedes {
                if out.insert(target.as_str()) {
                    queue.push(target.as_str());
                }
            }
        }
        out
    }

    /// The label a declaration resolves to, when it names exactly one framework this server knows.
    ///
    /// A list is accepted because the SDKs accept one (`framework=[Strands, Bedrock]`), and it resolves
    /// only when a single *framework* is named: a provider slug is claimed by no asset and so
    /// contributes nothing, which is how `[Strands, Bedrock]` still resolves to Strands while two
    /// genuine frameworks resolve to nothing. Two answers is not an answer, and picking one would label
    /// a span with no evidence for it.
    pub fn label_from_declaration(&self, declared: &str) -> Option<&str> {
        let mut labels: Vec<&str> = declared
            .split(',')
            .filter_map(|slug| self.sdk_slugs.get(slug.trim()))
            .map(String::as_str)
            .collect();
        labels.dedup();
        match labels.as_slice() {
            [one] => Some(one),
            _ => None,
        }
    }

    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }

    pub fn rules(&self) -> impl Iterator<Item = &CompiledDetect> {
        self.rules.iter()
    }

    pub fn slug_count(&self) -> usize {
        self.sdk_slugs.len()
    }
}
