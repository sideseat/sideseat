//! Rebuild the embedded rule set whenever anything under the assets directory changes.
//!
//! `rust-embed` reads each file through `include_bytes!`, so cargo notices an edit to a file it already
//! embedded but not a file added, removed or renamed - a build kept serving the old set, which surfaced as a
//! ruleset that contradicted the tree it was built from. A directory passed to `rerun-if-changed` is scanned
//! recursively.
fn main() {
    println!("cargo:rerun-if-changed=../../assets/rules");
}
