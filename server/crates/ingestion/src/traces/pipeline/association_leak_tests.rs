/// Every early return between creating a file association and committing the rows must release them.
///
/// The invariant: **from the moment a file association exists until it is confirmed or released, no path
/// may return.** A forgotten release is a *permanent* quota leak rather than a transient one - the orphan
/// sweeper selects on `ref_count = 0`, so an association left above zero means the bytes are never
/// reclaimed and the project's usable quota shrinks for good. Nothing user-visible happens, which is
/// exactly why four such returns were written on two separate occasions without anyone noticing.
///
/// Checked structurally, by reading every pipeline module, because that is the only form that catches the
/// *next* one:
/// a new early return added between these two calls compiles, passes every behavioural test, and leaks.
/// An RAII guard was tried first and rejected - releasing is asynchronous, so `Drop` cannot do it, and
/// the association set is read several times after the risky region, so a consuming guard does not fit
/// the flow without restructuring the write path to add a safety net to it.
///
/// The rule is deliberately coarse: *count* returns against releases inside the region. It cannot tell
/// which release belongs to which return, and a maintainer can defeat it. What it cannot do is stay
/// silent when someone adds a bare `return` to this region.
#[test]
fn every_early_return_between_files_and_the_write_releases_its_associations() {
    // The region is per path: each of the two write paths creates associations with
    // `persist_extracted_files` and ends the risky window at its own `write_to_duckdb`.
    let module_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/traces/pipeline");
    let sources: Vec<_> = std::fs::read_dir(module_dir)
        .expect("pipeline module directory should be readable")
        .map(|entry| {
            entry
                .expect("pipeline module entry should be readable")
                .path()
        })
        .filter(|path| path.extension().is_some_and(|extension| extension == "rs"))
        .map(|path| {
            let source =
                std::fs::read_to_string(&path).expect("pipeline module should be readable");
            (path, source)
        })
        .collect();

    let mut region_count = 0;
    for (path, source) in sources {
        let mut cursor = 0usize;
        while let Some(start) = source[cursor..].find("persist_extracted_files(") {
            let start = cursor + start;
            let Some(end) = source[start..].find("write_to_duckdb(") else {
                break;
            };
            let end = start + end;
            let region = &source[start..end];
            region_count += 1;

            // Adjacency, not a count. Counting was tried first and is too weak: the region holds two
            // *kinds* of release call, so a total that includes them all leaves slack, and removing one
            // real release still satisfied the sum. Verified by reverting a release and watching the
            // count-based form pass.
            const LOOKBACK: usize = 8;
            let lines: Vec<&str> = region.lines().collect();
            for (n, line) in lines.iter().enumerate() {
                if !line.contains("return ") {
                    continue;
                }
                let from = n.saturating_sub(LOOKBACK);
                let discharged = lines[from..n].iter().any(|l| {
                    l.contains("release_created_associations(")
                        || l.contains("release_associations_of_dropped(")
                        || l.contains("settle_associations_of_dropped(")
                });
                assert!(
                    discharged,
                    "{}: `{}` returns between creating file associations and the write without \
                         releasing them within the preceding {LOOKBACK} lines. An unreleased association \
                         holds its project's quota forever, because the orphan sweeper only reclaims at \
                         zero references.",
                    path.display(),
                    line.trim()
                );
            }
            cursor = end + "write_to_duckdb(".len();
        }
    }
    assert!(
        region_count >= 2,
        "both write paths should have been found; the anchors have moved"
    );
}
