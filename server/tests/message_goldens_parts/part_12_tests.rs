// The content-block chain's type index, held to the walk over every case it replaced, on the whole corpus.

/// Every distinct question the content-block chain was asked while the corpus was ingested and every view built:
/// the block, the position (`None` for an envelope expansion), and whether the block was a message's own.
fn content_block_questions() -> Vec<(
    String,
    sideseat_domain::rules::content_blocks::corpus_record::Asked,
)> {
    use sideseat_domain::rules::content_blocks::corpus_record::questions_asked;
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for (label, paths) in discover_fixtures() {
        let (_, asked) = questions_asked(|| {
            let rows = rows_for(&paths);
            build_golden(&label, &paths, &rows)
        });
        for question in asked {
            let (block, at, consult) = &question;
            if seen.insert(format!("{at:?}\u{1f}{consult}\u{1f}{block}")) {
                out.push((label.clone(), question));
            }
        }
    }
    out
}

/// What an envelope expansion answered, comparably.
fn expansion_of(
    expansion: Option<sideseat_domain::rules::content_blocks::Expansion<'_>>,
) -> Option<(bool, Vec<serde_json::Value>)> {
    use sideseat_domain::rules::content_blocks::Expansion;
    expansion.map(|expansion| match expansion {
        Expansion::Members(members) => (true, members.clone()),
        Expansion::Built(blocks) => (false, blocks),
    })
}

/// **The type index answers every question the corpus asks exactly as walking every case did.** The index asks a
/// block only the cases its `type` can satisfy, in declaration order, so the case that answers must be the one the
/// full walk reaches first - and here every distinct block the chain is asked while the corpus is ingested and its
/// four views are built is asked again of the full walk. A nested question is recorded and compared on its own,
/// so equal answers at every level are equal answers through the chain.
#[test]
fn the_content_block_type_index_answers_every_corpus_question_as_every_case_did() {
    let plan = &sideseat_domain::rules::ruleset().content_blocks;
    let questions = content_block_questions();
    let mut by_position: BTreeMap<String, usize> = BTreeMap::new();
    for (label, (block, at, consult)) in &questions {
        match at {
            Some(at) => assert_eq!(
                plan.normalize_with(block, *at, *consult),
                plan.normalize_with_every_case(block, *at, *consult),
                "{label}: {at:?} answers {block} differently through the index"
            ),
            None => assert_eq!(
                expansion_of(plan.expand(block)),
                expansion_of(plan.expand_with_every_case(block)),
                "{label}: the envelope expansion of {block} differs through the index"
            ),
        }
        *by_position.entry(format!("{at:?}")).or_default() += 1;
    }
    eprintln!(
        "content-block index: {} distinct questions compared, {by_position:?}",
        questions.len()
    );
    assert!(
        questions.len() > 4_000 && by_position.len() == 5,
        "the corpus asked too little to prove anything: {by_position:?}"
    );
}

/// The cost of one question, through the index and through every case. Run on demand, on a quiet machine:
/// `cargo test --locked -p sideseat-server --test message_goldens content_block_index_cost -- --ignored --nocapture`.
#[test]
#[ignore = "a measurement, not a check"]
fn content_block_index_cost() {
    let plan = &sideseat_domain::rules::ruleset().content_blocks;
    let questions: Vec<_> = content_block_questions()
        .into_iter()
        .map(|(_, question)| question)
        .filter(|(_, at, _)| at.is_some())
        .collect();
    let rounds = 20;
    let time = |every_case: bool| {
        let started = std::time::Instant::now();
        for _ in 0..rounds {
            for (block, at, consult) in &questions {
                let at = at.expect("filtered to positions");
                std::hint::black_box(if every_case {
                    plan.normalize_with_every_case(block, at, *consult)
                } else {
                    plan.normalize_with(block, at, *consult)
                });
            }
        }
        started.elapsed().as_nanos() as f64 / (rounds * questions.len()) as f64
    };
    // Warm both, then measure each twice, alternating, and keep the faster of each.
    time(false);
    time(true);
    let indexed = time(false).min(time(false));
    let every = time(true).min(time(true));
    eprintln!(
        "MEASURE content-block question: {} distinct, every case {every:.0} ns, indexed {indexed:.0} ns ({:.2}x)",
        questions.len(),
        every / indexed
    );
}
