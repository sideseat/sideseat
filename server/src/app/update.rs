//! Background npm version check for the executable.

use std::time::Duration;

use sideseat_core::constants::{
    NPM_REGISTRY_URL, UPDATE_CHECK_RETRIES, UPDATE_CHECK_RETRY_DELAY_MS, UPDATE_CHECK_TIMEOUT_SECS,
};

const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(serde::Deserialize)]
struct NpmPackage {
    version: String,
}

/// Return the latest stable npm version when it is newer than this executable.
pub(super) async fn check_for_update() -> Option<String> {
    let current = match semver::Version::parse(CURRENT_VERSION) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(
                version = CURRENT_VERSION,
                error = %e,
                "Failed to parse current version (bug)"
            );
            return None;
        }
    };

    let npm_version = fetch_npm_version_with_retry().await?;

    let npm = match semver::Version::parse(&npm_version) {
        Ok(v) => v,
        Err(e) => {
            tracing::debug!(
                version = %npm_version,
                error = %e,
                "Failed to parse npm version"
            );
            return None;
        }
    };

    if !npm.pre.is_empty() {
        tracing::debug!(version = %npm_version, "Skipping prerelease");
        return None;
    }

    if update_is_available(&current, &npm) {
        tracing::debug!(current = %current, npm = %npm, "Update available");
        Some(npm_version)
    } else {
        tracing::debug!(current = %current, npm = %npm, "No update available");
        None
    }
}

fn update_is_available(current: &semver::Version, candidate: &semver::Version) -> bool {
    candidate.pre.is_empty() && candidate > current
}

async fn fetch_npm_version_with_retry() -> Option<String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(UPDATE_CHECK_TIMEOUT_SECS))
        .user_agent(format!("SideSeat/{}", CURRENT_VERSION))
        .build()
        .ok()?;

    for attempt in 1..=UPDATE_CHECK_RETRIES {
        match fetch_npm_version(&client).await {
            Ok(version) => return Some(version),
            Err(e) => {
                tracing::debug!(attempt, error = %e, "Update check attempt failed");
                if attempt < UPDATE_CHECK_RETRIES {
                    tokio::time::sleep(Duration::from_millis(UPDATE_CHECK_RETRY_DELAY_MS)).await;
                }
            }
        }
    }
    None
}

async fn fetch_npm_version(client: &reqwest::Client) -> Result<String, String> {
    let resp = client
        .get(NPM_REGISTRY_URL)
        .send()
        .await
        .map_err(|e| format!("Request failed: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }

    let pkg: NpmPackage = resp
        .json()
        .await
        .map_err(|e| format!("Parse failed: {}", e))?;

    Ok(pkg.version)
}

pub(super) fn current_version() -> &'static str {
    CURRENT_VERSION
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_policy_accepts_only_newer_stable_versions() {
        for (current, candidate, expected) in [
            ("1.0.4", "1.0.5", true),
            ("1.0.4", "2.0.0", true),
            ("1.0.4", "1.0.4", false),
            ("1.0.4", "1.0.3", false),
            ("1.0.4", "1.0.5-beta.1", false),
        ] {
            let current = semver::Version::parse(current).unwrap();
            let candidate = semver::Version::parse(candidate).unwrap();
            assert_eq!(
                update_is_available(&current, &candidate),
                expected,
                "{current} -> {candidate}"
            );
        }
    }

    #[test]
    fn test_current_version_parses() {
        assert!(semver::Version::parse(CURRENT_VERSION).is_ok());
    }

    #[test]
    fn npm_response_requires_a_version_and_ignores_extra_fields() {
        let json = r#"{"name": "sideseat", "version": "1.0.5", "main": "index.js"}"#;
        let pkg: NpmPackage = serde_json::from_str(json).unwrap();
        assert_eq!(pkg.version, "1.0.5");

        let json = r#"{"name": "sideseat"}"#;
        assert!(serde_json::from_str::<NpmPackage>(json).is_err());
    }
}
