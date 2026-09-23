//! NPM version update checker

use std::time::Duration;

use crate::core::constants::{
    NPM_REGISTRY_URL, UPDATE_CHECK_RETRIES, UPDATE_CHECK_RETRY_DELAY_MS, UPDATE_CHECK_TIMEOUT_SECS,
};

const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Check npm registry for newer version.
/// Returns Some(version) if update available, None otherwise.
/// All errors logged at debug level - never fails.
pub async fn check_for_update() -> Option<String> {
    // Parse current version first - if this fails, it's a bug
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

    // Fetch with retry
    let npm_version = fetch_npm_version_with_retry().await?;

    // Parse npm version
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

    // Skip prereleases (e.g., 1.0.5-beta)
    if !npm.pre.is_empty() {
        tracing::debug!(version = %npm_version, "Skipping prerelease");
        return None;
    }

    // Compare
    if npm > current {
        tracing::debug!(current = %current, npm = %npm, "Update available");
        Some(npm_version)
    } else {
        tracing::debug!(current = %current, npm = %npm, "No update available");
        None
    }
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

    #[derive(serde::Deserialize)]
    struct NpmPackage {
        version: String,
    }

    let pkg: NpmPackage = resp
        .json()
        .await
        .map_err(|e| format!("Parse failed: {}", e))?;

    Ok(pkg.version)
}

/// Get the current version string
pub fn current_version() -> &'static str {
    CURRENT_VERSION
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(dead_code)]
    #[derive(serde::Deserialize)]
    struct TestNpmPackage {
        version: String,
    }

    #[test]
    fn test_version_comparison_newer() {
        let current = semver::Version::parse("1.0.4").unwrap();
        let npm = semver::Version::parse("1.0.5").unwrap();
        assert!(npm > current);
    }

    #[test]
    fn test_version_comparison_same() {
        let current = semver::Version::parse("1.0.4").unwrap();
        let npm = semver::Version::parse("1.0.4").unwrap();
        assert!(npm <= current);
    }

    #[test]
    fn test_version_comparison_older() {
        let current = semver::Version::parse("1.0.4").unwrap();
        let npm = semver::Version::parse("1.0.3").unwrap();
        assert!(npm <= current);
    }

    #[test]
    fn test_version_comparison_major() {
        let current = semver::Version::parse("1.0.4").unwrap();
        let npm = semver::Version::parse("2.0.0").unwrap();
        assert!(npm > current);
    }

    #[test]
    fn test_prerelease_detected() {
        let npm = semver::Version::parse("1.0.5-beta").unwrap();
        assert!(!npm.pre.is_empty());
    }

    #[test]
    fn test_stable_no_prerelease() {
        let npm = semver::Version::parse("1.0.5").unwrap();
        assert!(npm.pre.is_empty());
    }

    #[test]
    fn test_current_version_parses() {
        // Ensures Cargo.toml version is valid semver
        assert!(semver::Version::parse(CURRENT_VERSION).is_ok());
    }

    #[test]
    fn test_npm_response_parsing() {
        let json = r#"{"version": "1.0.5"}"#;
        let pkg: TestNpmPackage = serde_json::from_str(json).unwrap();
        assert_eq!(pkg.version, "1.0.5");
    }

    #[test]
    fn test_npm_response_extra_fields() {
        let json = r#"{"name": "sideseat", "version": "1.0.5", "main": "index.js"}"#;
        let pkg: TestNpmPackage = serde_json::from_str(json).unwrap();
        assert_eq!(pkg.version, "1.0.5");
    }

    #[test]
    fn test_npm_response_missing_version() {
        let json = r#"{"name": "sideseat"}"#;
        assert!(serde_json::from_str::<TestNpmPackage>(json).is_err());
    }
}
