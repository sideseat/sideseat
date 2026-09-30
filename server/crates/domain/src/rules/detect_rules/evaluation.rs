use super::*;

impl CompiledDetect {
    /// Any satisfied signal matches. Ordered cheapest-first: an equality probe is a hash lookup, while
    /// the prefix dimensions scan the span's keys.
    pub(super) fn matches(&self, ctx: &DetectContext<'_>) -> bool {
        let spec = &self.match_spec;

        if spec
            .attr_equals
            .iter()
            .any(|KeyValue { key, value }| ctx.span_attrs.get(key).is_some_and(|v| v == value))
        {
            return true;
        }
        if spec
            .attr_equals_ignore_case
            .iter()
            .any(|KeyValue { key, value }| {
                ctx.span_attrs
                    .get(key)
                    .is_some_and(|v| v.eq_ignore_ascii_case(value))
            })
        {
            return true;
        }
        if spec
            .attr_exists
            .iter()
            .any(|key| ctx.span_attrs.contains_key(key))
        {
            return true;
        }
        if spec
            .span_name_exact
            .iter()
            .any(|name| ctx.span_name == name)
        {
            return true;
        }
        if let Some(scope_name) = ctx.scope_name
            && spec.scope_name.iter().any(|name| scope_name == name)
        {
            return true;
        }
        // Prefix only: `starts_with` subsumes its own equality, so the equality arm here could never be the
        // reason a rule matched, and it made a separator-suffixed literal dead beside the bare one.
        if spec
            .span_name
            .iter()
            .any(|prefix| ctx.span_name.starts_with(prefix))
        {
            return true;
        }
        if !spec.service_name.is_empty()
            && let Some(service) = ctx.resource_attrs.get(super::super::SERVICE_NAME_KEY)
            // Substring, and the equality arm this used to have beside it was dead: `contains` subsumes its own
            // equality. The breadth is deliberate - `my-app-openai-agents-v1` is a service a user names themselves
            // and it identifies the SDK.
            && spec
                .service_name
                .iter()
                .any(|declared| service.contains(declared.as_str()))
        {
            return true;
        }
        if spec
            .resource_attr_contains
            .iter()
            .any(|KeyValue { key, value }| {
                ctx.resource_attrs
                    .get(key)
                    .is_some_and(|v| v.contains(value.as_str()))
            })
        {
            return true;
        }
        if spec
            .span_attr_contains
            .iter()
            .any(|KeyValue { key, value }| {
                ctx.span_attrs
                    .get(key)
                    .is_some_and(|v| v.contains(value.as_str()))
            })
        {
            return true;
        }
        if !spec.attr_prefix.is_empty()
            && ctx
                .span_attrs
                .keys()
                .any(|k| spec.attr_prefix.iter().any(|p| k.starts_with(p.as_str())))
        {
            return true;
        }
        if !self.text_needles_lowered.is_empty() {
            let hit = |text: &str| {
                let lowered = text.to_lowercase();
                self.text_needles_lowered
                    .iter()
                    .any(|n| lowered.contains(n.as_str()))
            };
            // The **first source with a value** answers, where the declaration says so: two attributes may
            // hold two answers to one question, and searching both asks "does either say so" where the
            // question was "does the one that applies say so". The span name counts as present always, since a
            // span has one - which is why it is only ever declared first where this flag is set.
            if self.text_first_present_source {
                if self.span_name_is_a_text_source {
                    return hit(ctx.span_name);
                }
                return self
                    .text_attribute_keys
                    .iter()
                    .find_map(|k| ctx.span_attrs.get(k))
                    .is_some_and(|v| hit(v));
            }
            if self.span_name_is_a_text_source && hit(ctx.span_name) {
                return true;
            }
            if self
                .text_attribute_keys
                .iter()
                .filter_map(|k| ctx.span_attrs.get(k))
                .any(|v| hit(v))
            {
                return true;
            }
        }
        false
    }
}

impl DetectPlan {
    /// The rule that claims this span.
    ///
    /// Takes the first by `legacy_rank`, which is what reproduces the table this replaced. Where more
    /// than one rule matches, that choice is a *migration bridge* and the alternatives are recoverable
    /// through [`Self::overlapping_candidates`] - the design's target is that no span has two, and the
    /// only way to get there is to be able to see which spans do.
    pub fn resolve(&self, ctx: &DetectContext<'_>) -> Option<&CompiledDetect> {
        let first = self.rules.iter().find(|rule| rule.matches(ctx))?;
        // `supersedes` **orders**, which is what `legacy_rank`'s own doc says the accepted design is: "a declared
        // `supersedes` resolves a known overlap". It used to waive only the overlap *report* while rank decided
        // the winner regardless - so the field documented an ordering it took no part in, and could be deleted
        // from an asset without changing a single attribution.
        //
        // The fast path is the common one: nothing supersedes this rule, so no later rule can displace it and the
        // scan stops where it always did. Only a rule some other rule claims to beat pays for the second look.
        if !self.superseded.contains(first.rule_id.as_str()) {
            return Some(first);
        }
        // The winner is the matching rule that **no** other matching rule beats, lowest rank among those. Not
        // "the first matching rule that beats the rank-winner": in rank order that test is satisfied by the
        // rank-winner itself, so the edge ordered nothing. Domination is transitive, since `supersedes` is a DAG
        // and a rule that beats a rule which beats this one beats it too; compilation refuses a cycle.
        let matching: Vec<&CompiledDetect> =
            self.rules.iter().filter(|rule| rule.matches(ctx)).collect();
        let beaten: std::collections::BTreeSet<&str> = matching
            .iter()
            .flat_map(|rule| self.dominated_by(rule.rule_id.as_str()))
            .collect();
        matching
            .iter()
            .find(|rule| !beaten.contains(rule.rule_id.as_str()))
            .copied()
            // Every candidate beaten by another is only possible in a cycle, which compilation refuses - so this
            // is unreachable, and falling back to the rank-winner is what it would have answered anyway.
            .or(Some(first))
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
            let spec = &rule.match_spec;
            let mut disagreements = |carrier: &str, wanted: &str, found: Option<&String>| {
                if let Some(found) = found
                    && !found.eq_ignore_ascii_case(wanted)
                {
                    out.push(NearMiss {
                        rule_id: rule.rule_id.clone(),
                        label: rule.label.clone(),
                        carrier: carrier.to_string(),
                        expected: wanted.to_string(),
                        found: found.clone(),
                    });
                }
            };
            for pair in spec.attr_equals.iter().chain(&spec.attr_equals_ignore_case) {
                disagreements(&pair.key, &pair.value, ctx.span_attrs.get(&pair.key));
            }
            for pair in &spec.span_attr_contains {
                if let Some(found) = ctx.span_attrs.get(&pair.key)
                    && !found.contains(pair.value.as_str())
                {
                    out.push(NearMiss {
                        rule_id: rule.rule_id.clone(),
                        label: rule.label.clone(),
                        carrier: pair.key.clone(),
                        expected: format!("containing `{}`", pair.value),
                        found: found.clone(),
                    });
                }
            }
            if let Some(service) = ctx.resource_attrs.get(super::super::SERVICE_NAME_KEY) {
                for declared in &spec.service_name {
                    if !service.contains(declared.as_str()) {
                        out.push(NearMiss {
                            rule_id: rule.rule_id.clone(),
                            label: rule.label.clone(),
                            carrier: super::super::SERVICE_NAME_KEY.to_string(),
                            expected: format!("containing `{declared}`"),
                            found: service.clone(),
                        });
                    }
                }
            }
        }
        out
    }

    /// Every rule that matches, in rank order.
    ///
    /// The instrument for retiring `legacy_rank`: a span with one candidate needs no ordering, and one
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
        // **Transitively** dominated, not only directly. `supersedes` is a DAG, so a rule that supersedes a
        // rule which supersedes a third owns that overlap too - reading direct edges only reported an overlap
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
    /// The transitive closure, because precedence is a DAG: a rule that supersedes one which supersedes a third
    /// has settled its ordering against all of them. Compilation refuses a cycle, so this terminates.
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
