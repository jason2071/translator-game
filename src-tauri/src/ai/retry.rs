//! Transient-failure retry with exponential backoff.
//!
//! Providers classify each attempt's outcome: [`CallError::Retryable`] for
//! 429 / 5xx / network / timeout, [`CallError::Fatal`] for auth and malformed
//! requests. [`with_retry`] backs off on the former and gives up on the latter.

use anyhow::Result;
use std::future::Future;
use std::time::Duration;

pub enum CallError {
    Retryable(anyhow::Error),
    Fatal(anyhow::Error),
}

/// Map an HTTP status to a retry decision (`true` = retryable).
pub fn status_is_retryable(status: u16) -> bool {
    status == 429 || (500..=599).contains(&status)
}

/// Run `f` up to `max_tries` times, backing off `base_ms * 2^attempt` (+ jitter)
/// between retryable failures.
pub async fn with_retry<F, Fut, T>(max_tries: u32, base_ms: u64, mut f: F) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = std::result::Result<T, CallError>>,
{
    let mut attempt = 0u32;
    loop {
        match f().await {
            Ok(v) => return Ok(v),
            Err(CallError::Fatal(e)) => return Err(e),
            Err(CallError::Retryable(e)) => {
                attempt += 1;
                if attempt >= max_tries {
                    return Err(e);
                }
                // Deterministic jitter (no RNG needed): vary by attempt.
                let jitter = (attempt as u64 % 5) * 90;
                let delay = base_ms.saturating_mul(1u64 << (attempt - 1)) + jitter;
                tokio::time::sleep(Duration::from_millis(delay.min(30_000))).await;
            }
        }
    }
}
