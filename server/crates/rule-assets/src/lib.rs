//! Embedded framework-rule assets.

use std::collections::BTreeMap;

use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "../../assets/rules/"]
struct RuleAssets;

/// Returns every JSON rule asset in deterministic path order.
pub fn sources() -> BTreeMap<String, Vec<u8>> {
    RuleAssets::iter()
        .filter(|path| path.ends_with(".json"))
        .filter_map(|path| {
            RuleAssets::get(&path).map(|file| (path.to_string(), file.data.into_owned()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embeds_json_rule_assets() {
        let sources = sources();
        assert!(!sources.is_empty());
        assert!(sources.keys().all(|path| path.ends_with(".json")));
    }

    /// The embedded set is the tree's set, byte for byte. A build that missed an added, removed or edited asset
    /// would otherwise serve a ruleset the tree does not contain, and only an unrelated check far downstream
    /// would notice.
    #[test]
    fn the_embedded_assets_are_the_tree_s_assets() {
        fn walk(
            dir: &std::path::Path,
            root: &std::path::Path,
            out: &mut BTreeMap<String, Vec<u8>>,
        ) {
            for entry in std::fs::read_dir(dir).expect("the assets directory is readable") {
                let path = entry.expect("a directory entry").path();
                if path.is_dir() {
                    walk(&path, root, out);
                } else if path.extension().is_some_and(|ext| ext == "json") {
                    let relative = path.strip_prefix(root).expect("under the root");
                    let key = relative.to_string_lossy().replace('\\', "/");
                    out.insert(key, std::fs::read(&path).expect("the asset is readable"));
                }
            }
        }
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/rules");
        let mut on_disk = BTreeMap::new();
        walk(&root, &root, &mut on_disk);
        let embedded = sources();
        assert_eq!(
            embedded.keys().collect::<Vec<_>>(),
            on_disk.keys().collect::<Vec<_>>(),
            "the embedded asset list differs from the tree: rebuild sideseat-rule-assets"
        );
        for (path, bytes) in &on_disk {
            assert!(
                embedded[path] == *bytes,
                "{path} is embedded with different contents than the tree holds"
            );
        }
    }
}
