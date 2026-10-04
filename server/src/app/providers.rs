//! Connects the credential service's connection test to the provider probe.

use std::time::{Duration, Instant};

use async_trait::async_trait;
use sideseat_adapter_provider_probe::{ProbeTarget, probe};
use sideseat_core::constants::CRED_TEST_TIMEOUT_SECS;
use sideseat_domain::providers::{CredentialConnectionTester, ResolvedCredential, TestResult};

pub struct ProbeCredentialConnectionTester;

#[async_trait]
impl CredentialConnectionTester for ProbeCredentialConnectionTester {
    async fn test(&self, resolved: &ResolvedCredential, secret: Option<&str>) -> TestResult {
        let target = ProbeTarget {
            provider: &resolved.provider_key,
            endpoint: resolved.endpoint_url.as_deref(),
            options: resolved.extra_config.as_ref(),
            secret,
        };
        let start = Instant::now();
        let outcome = probe(target, Duration::from_secs(CRED_TEST_TIMEOUT_SECS)).await;
        let latency_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
        match outcome {
            Ok(model_hint) => TestResult {
                success: true,
                latency_ms,
                error: None,
                model_hint,
            },
            Err(error) => TestResult {
                success: false,
                latency_ms,
                error: Some(error.to_string()),
                model_hint: None,
            },
        }
    }
}
