//! Asking the app for the macOS Accessibility permission and waiting for the user to grant it.

use std::future::Future;
use std::time::Duration;

#[cfg(target_os = "macos")]
use crossterm::terminal::{
    Clear,
    ClearType,
};
#[cfg(target_os = "macos")]
use crossterm::{
    cursor,
    execute,
};
#[cfg(target_os = "macos")]
use eyre::Result;
#[cfg(target_os = "macos")]
use fig_util::PRODUCT_NAME;
#[cfg(target_os = "macos")]
use spinners::{
    Spinner,
    Spinners,
};

pub const ACCESSIBILITY_WAIT_LIMIT: Duration = Duration::from_secs(120);
const ACCESSIBILITY_POLL_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessibilityOutcome {
    AlreadyGranted,
    Granted,
    TimedOut,
    Interrupted,
}

/// Whether the wait stops on Ctrl-C itself or leaves Ctrl-C to a handler the caller installed.
#[cfg(target_os = "macos")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CtrlC {
    Handle,
    LeaveToCaller,
}

/// A zero interval allows a single poll.
fn attempts_for(limit: Duration, interval: Duration) -> u32 {
    match limit.as_nanos().checked_div(interval.as_nanos()) {
        Some(attempts) => u32::try_from(attempts).unwrap_or(u32::MAX),
        None => 1,
    }
}

async fn wait_until_granted<F, Fut>(mut probe: F, attempts: u32, interval: Duration) -> AccessibilityOutcome
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

#[cfg(target_os = "macos")]
pub async fn accessibility_granted() -> bool {
    // The app may still be starting or busy, so a failed request counts as not granted yet.
    crate::cli::diagnostics::get_diagnostics()
        .await
        .is_ok_and(|diagnostics| diagnostics.accessibility == "true")
}

#[cfg(target_os = "macos")]
pub fn grant_instructions() -> [String; 2] {
    [
        "If macOS shows a dialog, choose Open System Settings. Otherwise open System Settings > Privacy & Security > Accessibility yourself.".to_owned(),
        format!("Then switch on {PRODUCT_NAME} in the list."),
    ]
}

#[cfg(target_os = "macos")]
pub async fn prompt_and_wait_for_accessibility(limit: Duration, ctrl_c: CtrlC) -> Result<AccessibilityOutcome> {
    if accessibility_granted().await {
        return Ok(AccessibilityOutcome::AlreadyGranted);
    }

    fig_ipc::local::prompt_accessibility_command().await?;

    let attempts = attempts_for(limit, ACCESSIBILITY_POLL_INTERVAL);
    let mut spinner = Spinner::new(Spinners::Dots, "Waiting for the Accessibility permission...".into());
    let wait = wait_until_granted(accessibility_granted, attempts, ACCESSIBILITY_POLL_INTERVAL);
    let outcome = match ctrl_c {
        CtrlC::Handle => tokio::select! {
            outcome = wait => outcome,
            Ok(()) = tokio::signal::ctrl_c() => AccessibilityOutcome::Interrupted,
        },
        CtrlC::LeaveToCaller => wait.await,
    };
    spinner.stop();
    execute!(std::io::stdout(), Clear(ClearType::CurrentLine), cursor::Show)?;
    println!();

    Ok(outcome)
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
