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
}
