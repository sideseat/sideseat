use super::*;

/// Compile every asset's field rules into one plan.
pub fn compile(
    assets: &super::super::assets::ParsedAssets,
) -> Result<SpanFieldPlan, FieldCompileError> {
    let mut rules: Vec<CompiledRule> = Vec::new();
    // Both indices exist to refuse a *silent* mistake: a duplicate id makes an explain trace ambiguous, and
    // a duplicate target means one field has two resolvers and which one fills it depends on load order.
    let mut by_id: HashMap<String, String> = HashMap::new();
    let mut by_target: HashMap<FieldTarget, String> = HashMap::new();

    // Declaration defects (two clauses sharing an id among them) are refused by `ParsedAssets::parse`, so no
    // file reaching this loop has one.
    for (file_id, file) in assets.iter() {
        let file_id = &file_id.to_owned();
        for rule in &file.span_fields {
            if let Some(first) = by_id.get(&rule.id) {
                return Err(FieldCompileError::DuplicateId {
                    rule: rule.id.clone(),
                    first: first.clone(),
                    second: file_id.clone(),
                });
            }
            if let Some(first) = by_target.get(&rule.target) {
                return Err(FieldCompileError::DuplicateTarget {
                    target: rule.target,
                    first: first.clone(),
                    second: rule.id.clone(),
                });
            }
            rules.push(compile_rule(file_id, rule)?);
            by_id.insert(rule.id.clone(), file_id.clone());
            by_target.insert(rule.target, rule.id.clone());
        }
    }
    Ok(SpanFieldPlan { rules })
}

fn compile_rule(file_id: &str, rule: &SpanFieldRule) -> Result<CompiledRule, FieldCompileError> {
    if rule.sources.is_empty() {
        return Err(FieldCompileError::NoSources {
            file: file_id.to_string(),
            rule: rule.id.clone(),
        });
    }
    if rule.combine == FieldCombine::MergeAll && rule.target.field_type() != FieldType::StringList {
        return Err(FieldCompileError::MergeIntoScalar {
            file: file_id.to_string(),
            rule: rule.id.clone(),
        });
    }
    let mut sources = Vec::with_capacity(rule.sources.len());
    for spec in &rule.sources {
        // A declaration that could not take effect reads as one that does.
        if spec.lowercase
            && !matches!(
                rule.target.field_type(),
                FieldType::Text | FieldType::StringList
            )
        {
            return Err(FieldCompileError::FoldWithoutText {
                file: file_id.to_string(),
                rule: rule.id.clone(),
            });
        }
        // Exactly one form. Two would make the read ambiguous and none makes the source dead, and both used to
        // be expressible - so this counts rather than pattern-matching a pair, which is what stopped covering
        // the forms as they were added.
        let forms = usize::from(spec.attribute.is_some())
            + usize::from(!spec.attribute_first_present_of.is_empty())
            + usize::from(spec.json.is_some())
            + usize::from(spec.span_name_strip_prefix.is_some())
            + usize::from(spec.value.is_some())
            + usize::from(spec.raw_span_name)
            + usize::from(spec.event_attribute.is_some());
        if forms == 0 {
            return Err(FieldCompileError::SourceReadsNothing {
                file: file_id.to_string(),
                rule: rule.id.clone(),
            });
        }
        if forms > 1 {
            return Err(FieldCompileError::SourceReadsTwoThings {
                file: file_id.to_string(),
                rule: rule.id.clone(),
            });
        }
        // Both the read and the **witness**, which had no such check: a witness naming no member always
        // answers false, so its source is permanently dead, and one naming both silently ignores the second.
        for json in [&spec.json, &spec.when_json].into_iter().flatten() {
            // A reduction combines the matches of *one* path, so there is nothing for it to do over a
            // first-present group - which names several paths and takes one of them.
            if json.reduce.is_some() && json.path.is_none() {
                return Err(FieldCompileError::ReductionWithoutAPath {
                    file: file_id.to_string(),
                    rule: rule.id.clone(),
                });
            }
            // And nothing for it to do on a **witness**, which asks only whether a member is there, nor on a
            // target that holds anything but a number - a sum is a number, so such a source compiles and can
            // only ever be malformed.
            let is_witness = spec
                .when_json
                .as_ref()
                .is_some_and(|w| std::ptr::eq(w, json));
            // `scalar_only` says each match of one path is a single string. Its whole domain is an unreduced
            // read through a `path` into a list-valued field: a witness only asks whether a member is there, a
            // reduction is already per match, and a first-present group selects a *member* rather than
            // matching many - so on any of those the declaration was accepted and did nothing, which reads as
            // protection that is not there.
            if json.scalar_only
                && (is_witness
                    || json.reduce.is_some()
                    || json.path.is_none()
                    || rule.target.field_type() != FieldType::StringList)
            {
                return Err(FieldCompileError::ScalarOnlyWithoutAPath {
                    file: file_id.to_string(),
                    rule: rule.id.clone(),
                });
            }
            let reduction_fits = match json.reduce {
                None => true,
                Some(Reduction::Sum) => rule.target.field_type() == FieldType::Integer,
                Some(Reduction::CollectAll) => rule.target.field_type() == FieldType::StringList,
            };
            if json.reduce.is_some() && (is_witness || !reduction_fits) {
                return Err(FieldCompileError::ReductionThatCannotYield {
                    file: file_id.to_string(),
                    rule: rule.id.clone(),
                });
            }
            let ways =
                usize::from(json.path.is_some()) + usize::from(!json.first_present_of.is_empty());
            if ways != 1 {
                return Err(FieldCompileError::JsonNamesNoMember {
                    file: file_id.to_string(),
                    rule: rule.id.clone(),
                });
            }
        }
        // The event form's `scalar_only` has the same domain as the `json` form's: a path into a list field.
        if let Some(event) = &spec.event_attribute
            && event.scalar_only
            && (event.path.is_none() || rule.target.field_type() != FieldType::StringList)
        {
            return Err(FieldCompileError::ScalarOnlyWithoutAPath {
                file: file_id.to_string(),
                rule: rule.id.clone(),
            });
        }
        // A literal with no gate is not a source: it answers on every span, so every source after it is dead
        // and the field is a constant.
        if spec.value.is_some() && spec.condition.is_none() && spec.when_json.is_none() {
            return Err(FieldCompileError::UngatedLiteral {
                file: file_id.to_string(),
                rule: rule.id.clone(),
            });
        }
        if spec.attribute.as_deref().is_some_and(str::is_empty)
            || spec
                .json
                .as_ref()
                .is_some_and(|json| json.attribute.is_empty())
            || spec
                .span_name_strip_prefix
                .as_deref()
                .is_some_and(str::is_empty)
            || spec.value.as_deref().is_some_and(str::is_empty)
            || spec.attribute_first_present_of.iter().any(String::is_empty)
            || spec
                .when_json
                .as_ref()
                .is_some_and(|witness| witness.attribute.is_empty())
        {
            return Err(FieldCompileError::EmptyAttribute {
                file: file_id.to_string(),
                rule: rule.id.clone(),
            });
        }
        // The same refusals every section's `where` meets: this stage sees a span's own name and attributes, so
        // a gate reading the resource or the scope would compile and never hold.
        let when = spec
            .condition
            .as_ref()
            .map(|condition| {
                super::super::detect_rules::checked_condition(
                    condition,
                    super::super::span_conditions::Readable::SPAN,
                )
            })
            .transpose()
            .map_err(|refusal| FieldCompileError::DeadGate {
                file: file_id.to_string(),
                rule: rule.id.clone(),
                detail: refusal.to_string(),
            })?;
        sources.push(CompiledSource {
            spec: spec.clone(),
            when,
        });
    }
    Ok(CompiledRule {
        rule_id: rule.id.clone(),
        target: rule.target,
        combine: rule.combine,
        sources,
    })
}
