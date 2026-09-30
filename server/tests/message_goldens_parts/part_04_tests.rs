/// Lineage must actually project the evidence, not merely avoid projecting it wrongly.
///
/// Two identical tool calls in one response share a `MessageIdentity` and both survive, keyed by their
/// rank. Recomputing identities after dedup could not tell them apart, so their evidence was left
/// unprojected - safe, but a gap, and `crewai/mcp_tools` is the corpus trace that has the pair. With a
/// lineage the pair projects, so the resolver sees both calls' emissions.
///
/// Asserted as "the resolver's answer still holds every survivor, and the two calls are both there":
/// projection is internal, so what is checked is the observable consequence.
#[test]
fn repeated_identical_calls_keep_both_and_stay_resolvable() {
    let label = "crewai/mcp_tools";
    let (_, paths) = discover_fixtures()
        .into_iter()
        .find(|(l, _)| l == label)
        .unwrap_or_else(|| panic!("fixture {label} not found"));
    let all: Vec<MessageSpanRow> = rows_for(&paths).into_iter().map(|(_, r)| r).collect();
    let first = all[0].trace_id.clone();
    let rows = sorted_by_timestamp(
        all.into_iter()
            .filter(|r| r.trace_id == first && passes_content_filter(r))
            .collect(),
    );

    let (legacy, scaffold) = legacy_and_neutral_order(rows.clone());
    let calls = |blocks: &[sideseat_domain::sideml::feed::BlockEntry]| -> usize {
        blocks.iter().filter(|b| b.entry_type == "tool_use").count()
    };
    assert!(
        calls(&legacy) >= 2,
        "this fixture is here because it has repeated identical calls, found {}",
        calls(&legacy)
    );
    assert_eq!(
        calls(&scaffold),
        calls(&legacy),
        "the resolver dropped or duplicated a call"
    );

    // And under FULL the answer is still every survivor, exactly once.
    let full = shadow_resolved_order(rows);
    assert_eq!(
        full.len(),
        legacy.len(),
        "FULL resolve is not a permutation for a trace with repeated identical calls"
    );
}

/// The feed is newest-first by *response*, and forward inside one.
///
/// The whole-view causality invariant is exempt for the feed, and correctly so: a call in one response
/// and the result in the next legitimately appear reversed there. But *within* a response the feed is
/// forward, and that is the property that lets the order resolver reach this view - the feed takes each
/// response's internal order from the reconstruction instead of re-deriving it from positions, so any
/// ordering the resolver improves shows up here too rather than only in the chronological views.
///
/// Checked as: a call and the result answering it, when they share a response, appear in that order.
#[test]
fn the_feed_keeps_each_response_forward() {
    let mut checked = 0usize;
    for (label, paths) in discover_fixtures() {
        let rows: Vec<MessageSpanRow> = rows_for(&paths).into_iter().map(|(_, r)| r).collect();
        if rows.is_empty() {
            continue;
        }
        let feed = process_feed(rows, &FeedOptions::new()).messages;

        // Where each call sits, per response.
        type Response = (String, chrono::DateTime<chrono::Utc>);
        let mut calls: HashMap<(Response, String), usize> = HashMap::new();
        for (index, block) in feed.iter().enumerate() {
            if block.entry_type != "tool_use" {
                continue;
            }
            if let Some(id) = block.tool_use_id.as_deref().filter(|s| !s.is_empty()) {
                let response = (block.trace_id.clone(), block.order_time);
                calls.entry((response, id.to_string())).or_insert(index);
            }
        }
        for (index, block) in feed.iter().enumerate() {
            if block.entry_type != "tool_result" {
                continue;
            }
            let Some(id) = block.tool_use_id.as_deref().filter(|s| !s.is_empty()) else {
                continue;
            };
            let response = (block.trace_id.clone(), block.order_time);
            let Some(&call_index) = calls.get(&(response, id.to_string())) else {
                continue; // the call is in another response - the feed may show that reversed
            };
            assert!(
                call_index < index,
                "{label}: within one response the feed put result {id} at {index} before its call \
                 at {call_index} - the response is not forward"
            );
            checked += 1;
        }
    }
    assert!(
        checked > 10,
        "expected the corpus to contain same-response call/result pairs, found {checked}"
    );
}

#[test]
#[ignore]
fn bench_pipeline() {
    let want = std::env::var("BENCH").unwrap_or_else(|_| "langgraph/swarm".to_string());
    let (_, paths) = discover_fixtures()
        .into_iter()
        .find(|(l, _)| *l == want)
        .unwrap_or_else(|| panic!("fixture {want} not found"));
    let rows: Vec<MessageSpanRow> = rows_for(&paths).into_iter().map(|(_, r)| r).collect();
    let iterations: u32 = std::env::var("ITERS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(20);

    // The harness has to hand the pipeline owned rows, and cloning a fixture whose payloads are
    // base64 images costs more than the pipeline does - so it is measured and subtracted rather than
    // reported as pipeline time. Production reads its rows from the database and never pays it.
    let _ = process_spans(rows.clone(), &FeedOptions::new());
    let clone_start = std::time::Instant::now();
    for _ in 0..iterations {
        std::hint::black_box(rows.clone());
    }
    let clone_cost = clone_start.elapsed() / iterations;

    let start = std::time::Instant::now();
    let mut blocks = 0usize;
    for _ in 0..iterations {
        blocks = process_spans(rows.clone(), &FeedOptions::new())
            .messages
            .len();
    }
    let elapsed = start.elapsed() / iterations;
    eprintln!(
        "BENCH {want}: {} rows -> {blocks} blocks, {:?} pipeline ({:?} measured - {:?} row clone),          {iterations} runs",
        rows.len(),
        elapsed.saturating_sub(clone_cost),
        elapsed,
        clone_cost
    );
    for (stage, took) in sideseat_domain::sideml::feed::stage_timings(rows) {
        eprintln!("  STAGE {stage}: {took:?}");
    }

    // The CPU half of ingestion: OTLP bytes through extraction, normalisation and enrichment. NOT
    // production ingestion - file extraction and every persistence step are outside this, so the
    // number is a floor rather than a measurement of what a request costs.
    let pricing = PricingService::init_for_test().expect("offline pricing service");
    let requests: Vec<_> = paths.iter().map(|p| decode_request(p)).collect();
    let bytes: usize = paths
        .iter()
        .map(|p| std::fs::metadata(p).map(|m| m.len() as usize).unwrap_or(0))
        .sum();
    let start = std::time::Instant::now();
    let mut produced = 0usize;
    for _ in 0..iterations {
        produced = requests
            .iter()
            .map(|r| normalize_for_test(r, &pricing).len())
            .sum();
    }
    let per_run = start.elapsed() / iterations;
    eprintln!(
        "  INGEST(cpu only) {want}: {produced} rows from {} KiB of OTLP, {per_run:?} per run",
        bytes / 1024
    );
}
/// No captured fixture exhausts the replay matcher's search budget.
///
/// The budget is a resource guard, and `FeedMetadata::replay_matching_complete` reports when it bites - but
/// a corpus that reaches it would mean real telemetry getting duplicated history, which is a different
/// matter from an adversarial shape doing so. Asserted over every fixture so that an extraction change
/// which widens a span's replayable set is caught here rather than by a user seeing a turn twice.
#[test]
fn no_fixture_exhausts_the_replay_matching_budget() {
    let mut checked = 0usize;
    for (label, paths) in discover_fixtures() {
        let rows: Vec<MessageSpanRow> = rows_for(&paths)
            .into_iter()
            .map(|(_, r)| r)
            .filter(passes_content_filter)
            .collect();
        if rows.is_empty() {
            continue;
        }
        let result = sideseat_domain::sideml::feed::process_spans(
            sorted_by_timestamp(rows),
            &FeedOptions::new(),
        );
        assert!(
            result.metadata.replay_matching_complete,
            "{label}: replay matching hit its budget, so this fixture's history may be shown twice"
        );
        checked += 1;
    }
    assert!(checked > 80, "only checked {checked} fixtures");
}

/// The corpus matches the support matrix, parsed from the document that carries it.
///
/// "Correct for all frameworks" is an open-world claim unless the set is written down, so it is written
/// down - and a table in a document drifts the moment someone adds a suite. A second hard-coded table in
/// this file would have drifted with it, which is what this test used to be: it compared the corpus against
/// its own copy and left the document free to be wrong. It reads the document now, so the claim "the table
/// and the corpus agree" is the thing actually checked.
///
/// The document is `server/tests/fixtures/messages/README.md`, **not** `CLAUDE.md`, and that is the whole
/// point of moving it. `CLAUDE.md` is tracked, but project convention keeps it out of routine commits, so its
/// committed content lags the working copy by however much has been written since: a test reading it compares
/// the corpus against whatever state a checkout happens to carry, and so passes or fails on how recently
/// someone committed a document rather than on whether the corpus matches it. This README is maintained
/// beside the fixtures, which is what makes it answerable.
#[test]
fn the_corpus_matches_the_support_matrix() {
    let doc = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/messages/README.md");
    let text = std::fs::read_to_string(&doc).expect("fixtures README");

    // Rows look like: | `suite` | version | samples | requests |
    let mut documented: Vec<(String, usize, usize, String)> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if !line.starts_with("| `") {
            continue;
        }
        let cells: Vec<&str> = line.trim_matches('|').split('|').map(str::trim).collect();
        if cells.len() != 4 {
            continue;
        }
        let suite = cells[0].trim_matches('`');
        let (Ok(samples), Ok(requests)) = (cells[2].parse::<usize>(), cells[3].parse::<usize>())
        else {
            continue;
        };
        documented.push((suite.to_string(), samples, requests, cells[1].to_string()));
    }
    documented.sort();
    assert!(
        documented.len() > 10,
        "the support matrix was not found in the fixtures README; this test is the thing that keeps it \
         true, so a \
         format change here must be matched there"
    );

    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/messages");
    let mut found: Vec<(String, usize, usize)> = Vec::new();
    // Kept honest by `local_only_samples_are_actually_gitignored`, so this cannot drift into excusing a
    // sample that someone simply forgot to commit.
    const LOCAL_ONLY_SAMPLES: [(&str, &str); 2] =
        [("strands-js", "image-gen"), ("vercel-ai-js", "image-gen")];

    for entry in std::fs::read_dir(&root).expect("fixture root") {
        let path = entry.expect("dir entry").path();
        if !path.is_dir() {
            continue;
        }
        let suite = path.file_name().unwrap().to_string_lossy().to_string();
        let mut samples = 0usize;
        let mut requests = 0usize;
        for sample in std::fs::read_dir(&path).expect("suite dir") {
            let sample = sample.expect("sample entry").path();
            if !sample.is_dir() {
                continue;
            }
            // Samples captured locally only are not part of the documented corpus. They are gitignored
            // because their payloads are 15 MB and 7 MB of inlined base64 image data, so a clean checkout
            // does not have them - and comparing the *filesystem* against a table that counted them made
            // this test fail for everyone but whoever captured them, including CI. The table now describes
            // what the repository actually contains.
            let sample_name = sample.file_name().unwrap().to_string_lossy().to_string();
            if LOCAL_ONLY_SAMPLES.contains(&(suite.as_str(), sample_name.as_str())) {
                continue;
            }
            samples += 1;
            requests += std::fs::read_dir(&sample)
                .expect("sample dir")
                .filter_map(Result::ok)
                // The same predicate discovery uses, so the documented count is exactly what runs.
                .filter(|f| is_captured_request(&f.file_name().to_string_lossy()))
                .count();
        }
        found.push((suite, samples, requests));
    }
    found.sort();

    let documented_counts: Vec<(String, usize, usize)> = documented
        .iter()
        .map(|(s, a, b, _)| (s.clone(), *a, *b))
        .collect();
    assert_eq!(
        found, documented_counts,
        "the fixture corpus and the fixtures README's support matrix disagree; update the table, because \
         the \
         boundary of \"correct for all frameworks\" is exactly that list"
    );

    // Every row names the SDK it was captured against: a count without a version does not bound anything,
    // because the same framework's next release can emit a different shape.
    for (suite, _, _, version) in &documented {
        assert!(
            !version.is_empty() && version != "—",
            "the support matrix row for `{suite}` has no version, so it does not say what was verified"
        );
    }
}

/// A cached reconstruction is what recomputation would have produced, byte for byte.
///
/// The memo is only sound if reconstruction is a pure function of the rows, and that is a claim about
/// every stage rather than an intention - a single read of the clock, or an output order that follows a
/// `HashMap`'s iteration, and a cached answer stops matching a fresh one. Checked over the whole corpus
/// rather than argued: reconstruct each fixture cold, then warm, and require the serialised blocks to be
/// identical.
///
/// It also settles the horizontally-scaled question. Instances do not need to share this cache or agree
/// on when to drop entries, because the memo is over a pure function: the same rows give the same answer
/// on any instance, an instance that starts cold simply recomputes it, and an instance that dies loses
/// nothing but the saving. What N instances change is the hit rate, not the answer.
#[test]
fn a_cached_reconstruction_equals_a_fresh_one() {
    use sideseat_domain::sideml::feed::cache::ReconstructionCache;
    use sideseat_domain::sideml::feed::{process_spans, process_spans_cached};

    let mut checked = 0usize;
    for (label, paths) in discover_fixtures() {
        let rows: Vec<MessageSpanRow> = rows_for(&paths)
            .into_iter()
            .map(|(_, r)| r)
            .filter(passes_content_filter)
            .collect();
        if rows.is_empty() {
            continue;
        }
        let rows = sorted_by_timestamp(rows);

        let fresh = process_spans(rows.clone(), &FeedOptions::new());
        let cache = ReconstructionCache::new();
        let cold = process_spans_cached(&cache, rows.clone(), &FeedOptions::new());
        let warm = process_spans_cached(&cache, rows, &FeedOptions::new());
        assert_eq!(
            cache.len(),
            1,
            "{label}: the second read must have been a hit"
        );

        let serialise = |result: &sideseat_domain::sideml::feed::FeedResult| -> String {
            result
                .messages
                .iter()
                .map(|b| {
                    format!(
                        "{}|{}|{}|{:?}|{}|{}",
                        b.trace_id,
                        b.span_id,
                        b.role.as_str(),
                        b.content,
                        b.timestamp.to_rfc3339(),
                        b.is_history
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        };
        assert_eq!(
            serialise(&cold),
            serialise(&fresh),
            "{label}: the cached path's cold read differs from the uncached one"
        );
        assert_eq!(
            serialise(&warm),
            serialise(&fresh),
            "{label}: a warm read differs from recomputing, so the memo is not transparent"
        );
        checked += 1;
    }
    assert!(checked > 80, "only checked {checked} fixtures");
}

/// Changing only the presentation constraints must not change which messages a session returns.
///
/// The acceptance property of the ordering redesign, and it was false until the causal transcript was
/// separated from the presented order: `process_multi_trace_spans` accumulated its cross-trace replay
/// prefix from each trace's *finished, ordered* messages, and the next trace consumed it as a sequence.
/// So promoting an ordering constraint changed what a later trace stripped - `adk/tool_use`'s session
/// view gained five replayed messages, the previous turns' tool results re-appearing, purely because
/// the presented sequence no longer lined up with what ADK replays.
///
/// Run over the whole row set of every fixture, so the multi-trace path is the one under test.
#[test]
fn ordering_constraints_do_not_change_a_session_s_messages() {
    let mut checked = 0usize;
    for (label, paths) in discover_fixtures() {
        let rows: Vec<MessageSpanRow> = rows_for(&paths)
            .into_iter()
            .map(|(_, r)| r)
            .filter(passes_content_filter)
            .collect();
        if rows.is_empty() {
            continue;
        }
        let (presented, unconstrained) = presented_and_unconstrained(sorted_by_timestamp(rows));
        let multiset = |blocks: &[sideseat_domain::sideml::feed::BlockEntry]| -> Vec<String> {
            let mut out: Vec<String> = blocks
                .iter()
                .map(|b| {
                    format!(
                        "{}/{}/{}/{}",
                        b.trace_id,
                        b.role.as_str(),
                        b.entry_type,
                        b.content_hash
                    )
                })
                .collect();
            out.sort();
            out
        };
        assert_eq!(
            multiset(&presented),
            multiset(&unconstrained),
            "{label}: the ordering constraints changed which messages the session returns, so \
             deduplication still depends on presentation"
        );
        checked += 1;
    }
    assert!(checked > 80, "only checked {checked} fixtures");
}

#[test]
#[ignore]
fn probe_pre_dedup() {
    use sideseat_domain::sideml::feed::PREFER_LATER_ON_TIE;

    let want = std::env::var("PROBE").unwrap_or_else(|_| "_synthetic/tool_use".to_string());
    let prefer_later = std::env::var_os("PROBE_LATER").is_some();
    PREFER_LATER_ON_TIE.with(|flag| flag.set(prefer_later));
    let (_, paths) = discover_fixtures()
        .into_iter()
        .find(|(l, _)| *l == want)
        .unwrap_or_else(|| panic!("fixture {want} not found"));
    let mut rows: Vec<MessageSpanRow> = rows_for(&paths)
        .into_iter()
        .map(|(_, r)| r)
        .filter(passes_content_filter)
        .collect();
    if let Ok(span_id) = std::env::var("PROBE_SPAN") {
        rows.retain(|row| row.span_id == span_id);
    }
    let rows = sorted_by_timestamp(rows);
    for (i, (b, ordinal)) in
        sideseat_domain::sideml::feed::classified_blocks_with_ordinals_for_test(rows.clone())
            .iter()
            .enumerate()
    {
        let c: String = format!("{:?}", b.content).chars().take(260).collect();
        eprintln!(
            "{i:2} trace={} span={} time={} msg={} pos={} {:9} {:11} ord={ordinal} out={} protected={} promoted={} hist={} correlated={} obs={:?} \
             name={:?} span_name={:?} scope={:?} carrier={:?}/{:?} path={:?} {c}",
            &b.trace_id[..8],
            &b.span_id[..8],
            b.timestamp,
            b.message_index,
            b.position,
            b.role.as_str(),
            b.entry_type,
            b.is_output_source(),
            b.is_protected(),
            b.promoted_to_span_output,
            b.is_history,
            b.tool_use_id_correlated,
            b.observation_type,
            b.name,
            b.span_name,
            b.scope_name,
            b.event_name,
            b.source_attribute,
            b.span_path
        );
    }
    eprintln!("-- survivors --");
    let survivors =
        sideseat_domain::sideml::feed::deduped_blocks_with_ordinals_for_test(rows.clone());
    for (i, (b, ordinal)) in survivors.iter().enumerate() {
        let c: String = format!("{:?}", b.content).chars().take(260).collect();
        eprintln!(
            "{i:2} trace={} span={} parent={:?} pos={} {:9} {:11} ord={ordinal} id={:?} {c}",
            &b.trace_id[..8],
            &b.span_id[..8],
            b.parent_span_id.as_deref().map(|id| &id[..8]),
            b.position,
            b.role.as_str(),
            b.entry_type,
            b.tool_use_id
        );
    }
    eprintln!("-- resolved --");
    for (i, b) in process_spans(rows, &FeedOptions::new())
        .messages
        .iter()
        .enumerate()
    {
        eprintln!(
            "{i:2} span={} parent={:?} time={} pos={} ord={} {:9} {:11} id={:?}",
            &b.span_id[..8],
            b.parent_span_id.as_deref().map(|id| &id[..8]),
            b.timestamp,
            b.position,
            b.occurrence_ordinal,
            b.role.as_str(),
            b.entry_type,
            b.tool_use_id
        );
    }
    PREFER_LATER_ON_TIE.with(|flag| flag.set(false));
}

/// Every carrier the corpus produces is classified deliberately in `sideml::carrier`.
///
/// `carrier.rs` claims this test exists; until now it did not. It cannot prove a classification is
/// semantically right - identical JSON can be accumulated state or one emission - but it can require
/// that no carrier reaches the resolver on the catch-all default without someone having looked at it.
/// That is the difference between a declaration and a guess.
#[test]
fn carrier_semantics_are_declared() {
    use sideseat_domain::sideml::carrier::declared_semantics;

    // Carriers the corpus contains that are knowingly left on the cautious default. Each one is a
    // decision, not an oversight: the reading is "a conversation as this span saw it", which can
    // under-report but never invent, and the answer invariant would catch under-reporting.
    // The mlflow and traceloop entries this list used to carry were removed by the membership check
    // below: no fixture exercises those carriers, so the exemptions excused nothing - and if a capture
    // ever arrives, failing loudly with "undeclared" is the correct prompt to classify them.
    const KNOWN_DEFAULTED: &[&str] = &[
        // OpenAI Agents currently uses these generic scalar names on local function spans. Their
        // meaning cannot be declared globally without changing an unrelated producer that chooses
        // the same name, and the carrier resolver cannot consume the identifying payload attributes.
        // The cautious snapshot reading is exact for one scalar block and cannot invent a duplicate.
        "attr:input",
        "attr:output",
    ];

    let mut seen: BTreeSet<String> = BTreeSet::new();
    for (_, paths) in discover_fixtures() {
        let rows: Vec<MessageSpanRow> = rows_for(&paths).into_iter().map(|(_, r)| r).collect();
        for block in sideseat_domain::sideml::feed::classified_blocks_for_test(rows) {
            match (&block.event_name, &block.source_attribute) {
                (Some(event), _) => seen.insert(format!("event:{event}")),
                (None, Some(attribute)) => seen.insert(format!("attr:{attribute}")),
                (None, None) => false,
            };
        }
    }
    assert!(
        seen.len() > 20,
        "expected the corpus to exercise many carriers, found {}",
        seen.len()
    );

    // Asked of the table directly, not by comparing values: a declared snapshot carrier and an
    // unclassified one have the same semantics and are completely different facts.
    let mut undeclared: Vec<String> = Vec::new();
    for carrier in &seen {
        let declared = match carrier.strip_prefix("event:") {
            Some(event) => declared_semantics(Some(event), None),
            None => declared_semantics(None, carrier.strip_prefix("attr:")),
        };
        if declared.is_none() && !KNOWN_DEFAULTED.contains(&carrier.as_str()) {
            undeclared.push(carrier.clone());
        }
    }
    assert!(
        undeclared.is_empty(),
        "these carriers reach the resolver on the catch-all default and nobody has classified them:\n  \
         {}\nAdd each to `sideml::carrier::semantics_for`, or to KNOWN_DEFAULTED with the reason the \
         cautious reading is right for it.",
        undeclared.join("\n  ")
    );

    // Both directions, like every list in this file: an entry that has since been *declared* is a
    // contradiction - the table says one thing and this list says another - and it sat unnoticed for a
    // regression when `system_prompt` became a declared frame carrier while still listed here
    // as unclassified.
    for entry in KNOWN_DEFAULTED {
        let declared = match entry.strip_prefix("event:") {
            Some(event) => declared_semantics(Some(event), None),
            None => declared_semantics(None, entry.strip_prefix("attr:")),
        };
        assert!(
            declared.is_none(),
            "{entry} is declared in `sideml::carrier` and still listed in KNOWN_DEFAULTED - remove \
             the entry"
        );
        assert!(
            seen.contains(*entry),
            "{entry} no longer occurs in the corpus - a dead exemption excuses nothing; remove it"
        );
    }
}

/// Which copy of a message survives deduplication must not change the *order* of the answer.
///
/// The central claim of the ordering redesign, and it had no test. Reversing the row order does not
/// reach it - rows are re-sorted by timestamp before dedup - so nothing varied the one thing that used
/// to decide the order: the surviving block's index in the previous sort.
///
/// `PREFER_LATER_ON_TIE` flips the tie-break, which changes the surviving copy in 17 of the 111
/// fixtures, so the perturbation is real rather than theoretical. Content may legitimately differ when
/// it flips - a copy carrying thinking blocks or model info is a better one, which is what quality
/// scoring exists to pick - so what is compared is the sequence of roles and kinds, not the text.
#[test]
fn which_copy_survives_does_not_change_the_order() {
    use sideseat_domain::sideml::feed::PREFER_LATER_ON_TIE;

    let set = |value: bool| PREFER_LATER_ON_TIE.with(|flag| flag.set(value));

    let mut differing: Vec<String> = Vec::new();
    let mut perturbed = 0usize;
    let content_of = |result: &sideseat_domain::sideml::feed::FeedResult| -> Vec<String> {
        result
            .messages
            .iter()
            .map(|b| b.content_hash.clone())
            .collect()
    };
    for (label, paths) in discover_fixtures() {
        let rows: Vec<MessageSpanRow> = rows_for(&paths)
            .into_iter()
            .map(|(_, r)| r)
            .filter(passes_content_filter)
            .collect();
        if rows.is_empty() {
            continue;
        }
        let rows = sorted_by_timestamp(rows);
        let rows_again = rows.clone();

        let shape = |blocks: &[sideseat_domain::sideml::feed::BlockEntry]| -> Vec<String> {
            blocks
                .iter()
                .map(|b| format!("{}/{}", b.role.as_str(), b.entry_type))
                .collect()
        };

        set(false);
        let early = shape(&process_spans(rows.clone(), &FeedOptions::new()).messages);
        set(true);
        let late = shape(&process_spans(rows, &FeedOptions::new()).messages);
        set(false);

        if early != late {
            differing.push(format!(
                "{label}:\n    early: {early:?}\n    late:  {late:?}"
            ));
        }

        // The perturbation has to be *reaching* dedup, or this test proves nothing. A flipped
        // tie-break selects a different copy, and copies differ in content, so somewhere in the corpus
        // the content must move even while the shape does not.
        set(false);
        let early_content = content_of(&process_spans(rows_again.clone(), &FeedOptions::new()));
        set(true);
        let late_content = content_of(&process_spans(rows_again, &FeedOptions::new()));
        set(false);
        if early_content != late_content {
            perturbed += 1;
        }
    }

    assert!(
        perturbed > 0,
        "flipping the dedup tie-break changed nothing anywhere, so this test is vacuous - the hook is \
         not reaching dedup"
    );
    assert!(
        differing.is_empty(),
        "the order of the answer depends on which copy survived dedup, in {} view(s):\n  {}",
        differing.len(),
        differing.join("\n  ")
    );
}

/// A barrier node orders exactly as the pairwise edges it replaces.
///
/// "Everything a generation received precedes everything it produced" is the *product* of the two sets
/// as edges, and a span re-sending a long history has hundreds of inputs - so the graph grew
/// quadratically in a span's message count. One barrier expresses the same relation in `inputs + outputs`
/// edges, keyed to pop exactly when the earliest output would have.
///
/// That is only sound if the two agree, and they do not agree unconditionally: where a span both
/// received and produced the same message the sets overlap, and a barrier would assert
/// `u -> barrier -> u` while the product contains a real `u -> b`. Those spans keep the pairwise form,
/// which is why this test compares the whole corpus rather than a constructed case.
#[test]
fn a_barrier_orders_exactly_as_pairwise_edges_do() {
    use sideseat_domain::sideml::feed::barrier_and_pairwise_order;

    let mut checked = 0usize;
    for (label, paths) in discover_fixtures() {
        let rows: Vec<MessageSpanRow> = rows_for(&paths)
            .into_iter()
            .map(|(_, r)| r)
            .filter(passes_content_filter)
            .collect();
        if rows.is_empty() {
            continue;
        }
        let (barrier, pairwise) = barrier_and_pairwise_order(sorted_by_timestamp(rows));
        let shape = |blocks: &[sideseat_domain::sideml::feed::BlockEntry]| -> Vec<String> {
            blocks
                .iter()
                .map(|b| {
                    format!(
                        "{}/{}/{}/{}",
                        b.trace_id,
                        b.role.as_str(),
                        b.entry_type,
                        b.content_hash
                    )
                })
                .collect()
        };
        assert_eq!(
            shape(&barrier),
            shape(&pairwise),
            "{label}: the barrier node changed the order the pairwise edges produce"
        );
        checked += 1;
    }
    assert!(checked > 80, "only checked {checked} fixtures");
}

/// End-to-end ingestion: OTLP bytes through extraction, enrichment, file storage and both writes.
///
/// `bench_pipeline`'s ingestion number is explicitly the CPU half - it stops before file extraction and
/// every persistence step - so what a request actually costs was unmeasured, which is not a claim anyone
/// should accept about a write path. This runs the real `run_batch`: a temp DuckDB, a temp SQLite, real
/// filesystem file storage, and the fence lookups included.
///
/// ```bash
/// cargo test --locked --release -p sideseat-server bench_ingestion -- --ignored --nocapture
/// ```
// The multi-threaded runtime, because `run_batch` uses `block_in_place` for its parallel extraction -
// so the current-thread runtime a plain `#[tokio::test]` gives would panic rather than measure.
#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn bench_ingestion_end_to_end() {
    use sideseat_core::config::{FilesConfig, StorageBackend};
    use sideseat_core::storage::AppStorage;
    use sideseat_ingestion::traces::TracePipeline;
    use sideseat_server::app::files::create_file_service;
    use sideseat_server::app::storage::{AnalyticsService, TransactionalService};
    use std::sync::Arc;

    let want = std::env::var("BENCH").unwrap_or_else(|_| "langgraph/swarm".to_string());
    let (label, paths) = discover_fixtures()
        .into_iter()
        .find(|(l, _)| *l == want)
        .unwrap_or_else(|| panic!("fixture {want} not found; set BENCH=<suite>/<sample>"));
    let requests: Vec<ExportTraceServiceRequest> =
        paths.iter().map(|p| decode_request(p)).collect();
    let bytes: usize = paths
        .iter()
        .map(|p| std::fs::metadata(p).map(|m| m.len() as usize).unwrap_or(0))
        .sum();

    let iterations = std::env::var("ITERATIONS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10u32);

    // Whatever refuses a batch says so through `tracing`, and a refused batch measures nothing - so the
    // reason has to be visible rather than inferred.
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .with_test_writer()
        .try_init();

    let mut totals: Vec<std::time::Duration> = Vec::new();
    let mut spans_written = 0u64;
    for _ in 0..iterations {
        // A fresh instance per iteration: ingestion is idempotent by span id, so re-delivering the same
        // requests into one database would measure an update path rather than a first write.
        let temp = tempfile::TempDir::new().expect("temp dir");
        let storage = AppStorage::init_for_test(temp.path().to_path_buf());
        tokio::fs::create_dir_all(temp.path().join("duckdb"))
            .await
            .expect("duckdb dir");
        // Both, and `files_temp` is the one that is easy to miss: `AppStorage::subdir` does not create
        // directories, and file writes stage into `files_temp` before being renamed into place - so
        // without it every write fails and the batch is refused rather than measured.
        tokio::fs::create_dir_all(temp.path().join("files"))
            .await
            .expect("files dir");
        tokio::fs::create_dir_all(temp.path().join("files_temp"))
            .await
            .expect("files temp dir");
        let analytics = Arc::new(AnalyticsService::Duckdb(Arc::new(
            sideseat_adapter_duckdb::DuckdbService::init(
                &storage,
                std::sync::Arc::new(sideseat_server::runtime::clock::SystemClock),
            )
            .await
            .expect("duckdb"),
        )));
        let sqlite_pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect(":memory:")
            .await
            .expect("sqlite");
        sqlx::raw_sql(sideseat_adapter_sqlite::schema::SCHEMA)
            .execute(&sqlite_pool)
            .await
            .expect("sqlite schema");
        let database = Arc::new(TransactionalService::Sqlite(Arc::new(
            sideseat_adapter_sqlite::SqliteService::from_pool(
                sqlite_pool,
                Arc::new(sideseat_server::runtime::clock::SystemClock),
            ),
        )));
        let files = Arc::new(
            create_file_service(
                FilesConfig {
                    enabled: true,
                    storage: StorageBackend::Filesystem,
                    quota_bytes: u64::MAX,
                    filesystem_path: Some(temp.path().join("files").display().to_string()),
                    s3: None,
                },
                &storage,
                Arc::clone(&database),
                Arc::new(
                    sideseat_adapter_cache::CacheService::new(
                        &sideseat_core::config::CacheConfig {
                            backend: sideseat_core::config::CacheBackendType::Memory,
                            max_entries: 1000,
                            eviction_policy: sideseat_core::config::EvictionPolicy::TinyLfu,
                            redis_url: None,
                        },
                    )
                    .await
                    .expect("memory cache"),
                ),
            )
            .await
            .expect("file service"),
        );
        let analytics_port: Arc<dyn sideseat_ports::traits::AnalyticsRepository + Send + Sync> =
            Arc::from(analytics.repository());
        let database_port: Arc<dyn sideseat_ports::traits::TransactionalRepository + Send + Sync> =
            Arc::from(database.repository());
        let pipeline = TracePipeline::new(
            Arc::clone(&analytics_port),
            Arc::new(PricingService::init_for_test().expect("offline pricing service")),
            Arc::new(sideseat_messaging::TopicService::new(
                sideseat_adapter_topics::memory_backend(),
            )),
            Arc::clone(&files),
            Arc::new(sideseat_ingestion::staging::StagingService::new(
                Arc::clone(files.storage()),
                database_port,
                analytics_port,
                Arc::new(sideseat_server::runtime::clock::SystemClock),
                sideseat_core::config::RetentionConfig::default(),
                5,
            )),
        );

        // `PER_REQUEST=1` measures the same work with no batching, which is what acknowledging only
        // what is stored would cost: one write per request instead of one per batch.
        let per_request = std::env::var("PER_REQUEST").is_ok();
        let start = std::time::Instant::now();
        let ok = if per_request {
            let mut all = true;
            for request in &requests {
                all &= pipeline
                    .run_batch_for_test(std::slice::from_ref(request))
                    .await;
            }
            all
        } else {
            pipeline.run_batch_for_test(&requests).await
        };
        totals.push(start.elapsed());
        assert!(
            ok,
            "{label}: the batch was refused, so this measures nothing"
        );

        spans_written = analytics
            .repository()
            .count_spans_by_project(&[ProjectId::from("default")])
            .await
            .map(|c| c.values().sum())
            .unwrap_or(0);
        assert!(
            spans_written > 0,
            "{label}: nothing was written - the batch was accepted but its spans went nowhere"
        );
    }

    totals.sort();
    let sum: std::time::Duration = totals.iter().sum();
    eprintln!(
        "INGEST(end to end) {label}: {} requests / {} KB -> {spans_written} spans, \
         mean {:?}, p50 {:?}, max {:?} over {iterations} runs",
        requests.len(),
        bytes / 1024,
        sum / iterations,
        totals[totals.len() / 2],
        totals[totals.len() - 1],
    );
    assert!(
        spans_written > 0,
        "nothing was written, so nothing was measured"
    );
}
