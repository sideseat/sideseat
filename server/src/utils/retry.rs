//! Async retry utilities with exponential backoff

use std::time::Duration;

/// Default maximum retry attempts for database operations
pub const DEFAULT_MAX_ATTEMPTS: u32 = 3;

/// Default base delay in milliseconds for exponential backoff
pub const DEFAULT_BASE_DELAY_MS: u64 = 100;

/// Retry a synchronous operation with exponential backoff.
///
/// Returns `Ok(attempts)` on success, or `Err((error, attempts))` on failure.
pub async fn retry_with_backoff<F, E>(
    max_attempts: u32,
    base_delay_ms: u64,
    mut operation: F,
) -> Result<u32, (E, u32)>
where
    F: FnMut() -> Result<(), E>,
    E: std::fmt::Display,
{
    let mut attempts = 0u32;

    loop {
        attempts += 1;
        match operation() {
            Ok(()) => return Ok(attempts),
            Err(e) => {
                if attempts >= max_attempts {
                    return Err((e, attempts));
                }
                let delay = Duration::from_millis(base_delay_ms * 2_u64.pow(attempts - 1));
                tracing::warn!(
                    error = %e,
                    attempt = attempts,
                    delay_ms = delay.as_millis(),
                    "Retrying after transient error"
                );
                tokio::time::sleep(delay).await;
            }
        }
    }
}

/// Retry an async operation with exponential backoff.
///
/// Returns `Ok(attempts)` on success, or `Err((error, attempts))` on failure.
pub async fn retry_with_backoff_async<F, Fut, E>(
    max_attempts: u32,
    base_delay_ms: u64,
    mut operation: F,
) -> Result<u32, (E, u32)>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<(), E>>,
    E: std::fmt::Display,
{
    let mut attempts = 0u32;

    loop {
        attempts += 1;
        match operation().await {
            Ok(()) => return Ok(attempts),
            Err(e) => {
                if attempts >= max_attempts {
                    return Err((e, attempts));
                }
                let delay = Duration::from_millis(base_delay_ms * 2_u64.pow(attempts - 1));
                tracing::warn!(
                    error = %e,
                    attempt = attempts,
                    delay_ms = delay.as_millis(),
                    "Retrying after transient error"
                );
                tokio::time::sleep(delay).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[tokio::test]
    async fn test_success_on_first_try() {
        let result = retry_with_backoff(3, 10, || Ok::<(), &str>(())).await;
        assert_eq!(result, Ok(1));
    }

    #[tokio::test]
    async fn test_success_after_retry() {
        let attempts = RefCell::new(0);
        let result = retry_with_backoff(3, 10, || {
            *attempts.borrow_mut() += 1;
            if *attempts.borrow() < 2 {
                Err("transient error")
            } else {
                Ok(())
            }
        })
        .await;
        assert_eq!(result, Ok(2));
    }

    #[tokio::test]
    async fn test_failure_after_max_retries() {
        let result = retry_with_backoff(3, 10, || Err::<(), _>("persistent error")).await;
        assert!(result.is_err());
        let (error, attempts) = result.unwrap_err();
        assert_eq!(error, "persistent error");
        assert_eq!(attempts, 3);
    }
}
