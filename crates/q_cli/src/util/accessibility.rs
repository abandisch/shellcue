//! Asking the app for the macOS Accessibility permission and waiting for the user to grant it.

#[cfg(target_os = "macos")]
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
pub use fig_util::accessibility_wait::ACCESSIBILITY_WAIT_LIMIT;
pub use fig_util::accessibility_wait::AccessibilityOutcome;
#[cfg(target_os = "macos")]
use fig_util::accessibility_wait::{
    ACCESSIBILITY_POLL_INTERVAL,
    attempts_for,
    wait_until_granted,
};
#[cfg(target_os = "macos")]
use spinners::{
    Spinner,
    Spinners,
};

/// Whether the wait stops on Ctrl-C itself or leaves Ctrl-C to a handler the caller installed.
#[cfg(target_os = "macos")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CtrlC {
    Handle,
    LeaveToCaller,
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
