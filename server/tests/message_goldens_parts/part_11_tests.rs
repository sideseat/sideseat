// Version provenance: which release each versioned capture is, and the `observed_in` ranges held to it.
//
// A versioned mode directory (`native@1.12.0`, `native@0.7b1+semconv-latest`) is a capture of one release under
// one configuration, recorded by the matrix in `<producer>/versions.json`. The ranges a clause declares in
// `observed_in` are positive evidence about those releases, so they are checked against the captures: each range
// is exercised by a capture inside it, and a range whose releases have all left the support window has aged out.

/// One versioned capture's release, as `versions.json` records it.
#[derive(Debug, Clone, Deserialize)]
struct CaptureRelease {
    package: String,
    version: String,
    profile: String,
}

#[derive(Debug, Deserialize)]
struct VersionsFile {
    format: String,
    modes: BTreeMap<String, CaptureRelease>,
}

/// `producer/mode` -> the release captured there, for every producer with a provenance file.
fn capture_releases() -> BTreeMap<String, CaptureRelease> {
    let mut out = BTreeMap::new();
    for entry in std::fs::read_dir(fixture_root())
        .expect("the fixture root exists")
        .flatten()
    {
        let path = entry.path().join("versions.json");
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let producer = entry.file_name().to_string_lossy().to_string();
        let file: VersionsFile =
            serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        assert_eq!(
            file.format,
            "sideseat.fixture-versions/1",
            "{}",
            path.display()
        );
        for (mode, release) in file.modes {
            out.insert(format!("{producer}/{mode}"), release);
        }
    }
    out
}

/// **Every versioned capture names its release, and every named release is captured.** The directory name is
/// rendered from the record - `<mode>@<version>`, then `+<profile>` unless the profile is the default - rather than
/// parsed, because a `+` is also PEP 440's local-version separator and SemVer's build separator.
#[test]
fn every_versioned_capture_names_its_release() {
    let releases = capture_releases();
    let mut problems = Vec::new();
    for entry in std::fs::read_dir(fixture_root())
        .expect("the fixture root exists")
        .flatten()
    {
        if !entry.path().is_dir() {
            continue;
        }
        let producer = entry.file_name().to_string_lossy().to_string();
        for mode in std::fs::read_dir(entry.path())
            .into_iter()
            .flatten()
            .flatten()
        {
            let name = mode.file_name().to_string_lossy().to_string();
            if mode.path().is_dir()
                && name.contains('@')
                && !releases.contains_key(&format!("{producer}/{name}"))
            {
                problems.push(format!(
                    "{producer}/{name}: a versioned capture with no entry in versions.json"
                ));
            }
        }
    }
    for (key, release) in &releases {
        let (producer, mode) = key.split_once('/').expect("producer/mode");
        let base = mode.split_once('@').map_or(mode, |(base, _)| base);
        let rendered = if release.profile == "default" {
            format!("{base}@{}", release.version)
        } else {
            format!("{base}@{}+{}", release.version, release.profile)
        };
        if rendered != mode {
            problems.push(format!("{key}: the record renders as `{rendered}`"));
        }
        if !fixture_root().join(producer).join(mode).is_dir() {
            problems.push(format!(
                "{key}: versions.json records a capture that is not there"
            ));
        }
    }
    assert!(
        releases.len() > 50,
        "only {} versioned captures were found",
        releases.len()
    );
    assert!(
        problems.is_empty(),
        "fixture provenance disagrees with the tree:\n  {}",
        problems.join("\n  ")
    );
}

/// The detection rule (or alternative) that labelled each span, with the fixtures it did so in.
fn detections_by_fixture() -> BTreeMap<String, BTreeSet<String>> {
    use sideseat_domain::rules::detect_rules::DetectContext;
    use sideseat_ingestion::otlp::extract_attributes;
    let plan = &sideseat_domain::rules::ruleset().detect;
    let mut fired: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (label, paths) in discover_fixtures() {
        for path in &paths {
            let request = decode_request(path);
            for resource in &request.resource_spans {
                let resource_attrs = resource
                    .resource
                    .as_ref()
                    .map(|r| extract_attributes(&r.attributes))
                    .unwrap_or_default();
                for scope in &resource.scope_spans {
                    let scope_name = scope
                        .scope
                        .as_ref()
                        .map(|s| s.name.as_str())
                        .filter(|n| !n.is_empty());
                    for span in &scope.spans {
                        let attrs = extract_attributes(&span.attributes);
                        let ctx = DetectContext {
                            span_name: &span.name,
                            scope_name,
                            span_attrs: &attrs,
                            resource_attrs: &resource_attrs,
                        };
                        if let Some(rule) = plan.resolve(&ctx) {
                            fired
                                .entry(rule.rule_id.clone())
                                .or_default()
                                .insert(label.clone());
                        }
                    }
                }
            }
        }
    }
    fired
}

/// Whether a capture is a release inside the range: the same package, the profile the range names (any, where
/// it names none), and a version the range's scheme places inside it.
fn capture_inside(
    range: &sideseat_domain::rules::schema::Observation,
    release: &CaptureRelease,
) -> bool {
    release.package == range.package
        && range
            .profile
            .as_ref()
            .is_none_or(|profile| *profile == release.profile)
        && range.contains(&release.version) == Some(true)
}

/// **Every `observed_in` range is exercised by a capture inside it.** The clauses that fired in each fixture -
/// message rules and their readings through their emissions' evidence, detection rules through the rule that
/// labelled the span - are joined with each versioned capture's release; a range no capture inside it fired in
/// is a claim with no evidence, and fails. A clause firing in a capture of its package outside every range it
/// declares is reported as an unexplained observation and does not fail: the ranges are evidence, not a
/// boundary.
#[test]
fn every_observed_range_is_exercised_by_a_capture_inside_it() {
    let parsed = sideseat_domain::rules::assets::ParsedAssets::parse(
        &sideseat_domain::rules::schema::embedded_sources(),
    )
    .expect("the embedded assets parse");
    let releases = capture_releases();
    let mut fired = rules_that_emit_by_fixture();
    for (leaf, fixtures) in detections_by_fixture() {
        fired.entry(leaf).or_default().extend(fixtures);
    }
    let release_of = |fixture: &str| {
        let mut parts = fixture.splitn(3, '/');
        let (producer, mode) = (parts.next()?, parts.next()?);
        releases.get(&format!("{producer}/{mode}"))
    };
    let mut unexercised = Vec::new();
    let mut unexplained = Vec::new();
    for observed in parsed.observed() {
        // A selection point fires when any case under it does, since its cases are credited by their paths.
        let fixtures: BTreeSet<String> = fired
            .iter()
            .filter(|(leaf, _)| {
                *leaf == &observed.leaf || leaf.starts_with(&format!("{}/", observed.leaf))
            })
            .flat_map(|(_, fixtures)| fixtures.iter().cloned())
            .collect();
        for range in &observed.ranges {
            let exercised = fixtures
                .iter()
                .filter_map(|fixture| release_of(fixture))
                .any(|release| capture_inside(range, release));
            if !exercised {
                unexercised.push(format!("{} ({}): {range:?}", observed.leaf, observed.asset));
            }
        }
        for fixture in &fixtures {
            if let Some(release) = release_of(fixture)
                && observed
                    .ranges
                    .iter()
                    .any(|range| range.package == release.package)
                && !observed
                    .ranges
                    .iter()
                    .any(|range| capture_inside(range, release))
            {
                unexplained.push(format!(
                    "{} fired in {fixture} ({})",
                    observed.leaf, release.version
                ));
            }
        }
    }
    for line in &unexplained {
        eprintln!("unexplained_observation: {line}");
    }
    assert!(
        unexercised.is_empty(),
        "these `observed_in` ranges are exercised by no capture inside them:\n  {}",
        unexercised.join("\n  ")
    );
}

/// The first day of the support window: every release of the last twelve months is supported. A constant moved
/// deliberately, never the clock, so a run is the same whenever it happens.
const SUPPORT_WINDOW_STARTS: &str = "2025-10-07";

/// Clauses kept although every range they declare has aged out, with the reason and the day to look again.
const AGED_KEPT: &[(&str, &str, &str)] = &[];

#[derive(Debug, Deserialize)]
struct Census {
    package: String,
    releases: Vec<CensusRelease>,
}

#[derive(Debug, Deserialize)]
struct CensusRelease {
    version: String,
    date: String,
    profile: String,
}

/// Every release the matrix censused, by package: its version, release date and profile.
fn censused_releases() -> BTreeMap<String, Vec<CensusRelease>> {
    fn walk(dir: &Path, out: &mut BTreeMap<String, Vec<CensusRelease>>) {
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if path.is_dir() && name != "node_modules" && !name.starts_with('.') {
                walk(&path, out);
            } else if name == "versions.census.json" {
                let census: Census = serde_json::from_str(&std::fs::read_to_string(&path).unwrap())
                    .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
                out.entry(census.package)
                    .or_default()
                    .extend(census.releases);
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples"),
        &mut out,
    );
    out
}

/// **No clause is kept for releases the window no longer supports.** A range is alive while some release inside
/// it - by version, in the range's scheme, not by date order, since a lower version can ship after a higher one -
/// was released inside the window; a range whose package the census does not cover is unknown, never aged out.
/// A clause whose every range has aged out fails, unless it is kept with a reason and a day to look again.
#[test]
fn no_observed_clause_has_aged_out() {
    let parsed = sideseat_domain::rules::assets::ParsedAssets::parse(
        &sideseat_domain::rules::schema::embedded_sources(),
    )
    .expect("the embedded assets parse");
    let census = censused_releases();
    assert!(!census.is_empty(), "the matrix census was not found");
    let mut aged = Vec::new();
    for observed in parsed.observed() {
        let alive_or_unknown =
            observed
                .ranges
                .iter()
                .any(|range| match census.get(&range.package) {
                    None => true,
                    Some(releases) => releases.iter().any(|release| {
                        release.date.as_str() >= SUPPORT_WINDOW_STARTS
                            && range.profile.as_ref().is_none_or(|p| *p == release.profile)
                            && range.contains(&release.version) == Some(true)
                    }),
                });
        if !alive_or_unknown && !AGED_KEPT.iter().any(|(leaf, _, _)| *leaf == observed.leaf) {
            aged.push(format!("{} ({})", observed.leaf, observed.asset));
        }
    }
    assert!(
        aged.is_empty(),
        "these clauses describe only releases older than the support window, which began {SUPPORT_WINDOW_STARTS}; \
         delete them, or keep one in AGED_KEPT with a reason and a day to look again:\n  {}",
        aged.join("\n  ")
    );
    for (leaf, reason, revisit) in AGED_KEPT {
        assert!(
            !reason.is_empty() && revisit.len() == 10,
            "{leaf}: a kept clause states why and when"
        );
        assert!(
            parsed
                .observed()
                .iter()
                .any(|observed| observed.leaf == *leaf),
            "AGED_KEPT names `{leaf}`, which declares no range"
        );
    }
}

/// Which clauses fire in some captured releases of a package and not in others: the candidates for an
/// `observed_in` range. A report for asset work over the matrix, not a gate; run it with `--ignored`.
#[test]
#[ignore = "a report for matrix-driven asset work, not a gate"]
fn report_clauses_whose_firing_depends_on_the_release() {
    let releases = capture_releases();
    let mut fired = rules_that_emit_by_fixture();
    for (leaf, fixtures) in detections_by_fixture() {
        fired.entry(leaf).or_default().extend(fixtures);
    }
    // producer/mode per package, among the captures.
    let mut captured: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for (key, release) in &releases {
        captured
            .entry(release.package.as_str())
            .or_default()
            .insert(key.as_str());
    }
    for (leaf, fixtures) in &fired {
        let modes: BTreeSet<String> = fixtures
            .iter()
            .filter_map(|f| {
                let mut parts = f.splitn(3, '/');
                Some(format!("{}/{}", parts.next()?, parts.next()?))
            })
            .filter(|key| releases.contains_key(key))
            .collect();
        for (package, all) in &captured {
            let here: BTreeSet<&str> = modes
                .iter()
                .map(String::as_str)
                .filter(|m| all.contains(m))
                .collect();
            if !here.is_empty() && here.len() < all.len() {
                let versions = |set: &BTreeSet<&str>| {
                    set.iter()
                        .map(|k| releases[*k].version.clone())
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                let missing: BTreeSet<&str> = all.difference(&here).copied().collect();
                println!(
                    "{leaf} [{package}]: fires in {} | not in {}",
                    versions(&here),
                    versions(&missing)
                );
            }
        }
    }
}
