//! Waiting for the user to grant the macOS Accessibility permission.

use std::future::Future;
use std::time::Duration;

pub const ACCESSIBILITY_WAIT_LIMIT: Duration = Duration::from_secs(120);
pub const ACCESSIBILITY_POLL_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessibilityOutcome {
    AlreadyGranted,
    Granted,
    TimedOut,
    Interrupted,
}

/// A zero interval allows a single poll.
pub fn attempts_for(limit: Duration, interval: Duration) -> u32 {
    match limit.as_nanos().checked_div(interval.as_nanos()) {
        Some(attempts) => u32::try_from(attempts).unwrap_or(u32::MAX),
        None => 1,
    }
}

pub async fn wait_until_granted<F, Fut>(mut probe: F, attempts: u32, interval: Duration) -> AccessibilityOutcome
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    for attempt in 0..attempts {
        if attempt > 0 {
            tokio::time::sleep(interval).await;
        }
        if probe().await {
            return AccessibilityOutcome::Granted;
        }
    }
    AccessibilityOutcome::TimedOut
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    async fn probe_calls(granted_on: Option<u32>, attempts: u32) -> (AccessibilityOutcome, u32) {
        let calls = Cell::new(0);
        let outcome = wait_until_granted(
            || {
                calls.set(calls.get() + 1);
                let granted = granted_on == Some(calls.get());
                async move { granted }
            },
            attempts,
            Duration::ZERO,
        )
        .await;
        (outcome, calls.get())
    }

    #[tokio::test]
    async fn granted_on_first_probe() {
        assert_eq!(probe_calls(Some(1), 5).await, (AccessibilityOutcome::Granted, 1));
    }

    #[tokio::test]
    async fn granted_on_later_probe() {
        assert_eq!(probe_calls(Some(4), 5).await, (AccessibilityOutcome::Granted, 4));
    }

    #[tokio::test]
    async fn times_out_after_every_allowed_probe() {
        assert_eq!(probe_calls(None, 5).await, (AccessibilityOutcome::TimedOut, 5));
    }

    #[test]
    fn attempts_for_wait_limit() {
        assert_eq!(attempts_for(ACCESSIBILITY_WAIT_LIMIT, ACCESSIBILITY_POLL_INTERVAL), 120);
        assert_eq!(attempts_for(ACCESSIBILITY_WAIT_LIMIT, Duration::ZERO), 1);
    }
}
