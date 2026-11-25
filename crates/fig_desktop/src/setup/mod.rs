//! The setup window: one page that offers each setup step and does it only when asked.

mod assets;
pub mod bridge;
#[cfg(target_os = "macos")]
mod flow;
pub mod view;
#[cfg(target_os = "macos")]
mod window;

use std::path::Path;

use fig_util::consts::CLI_BINARY_NAME;
#[cfg(target_os = "macos")]
pub use bridge::update_script;
#[cfg(target_os = "macos")]
pub use flow::{
    close,
    handle_command,
    refresh,
    show,
};
#[cfg(target_os = "macos")]
pub use window::build_window;

#[cfg(target_os = "macos")]
const SETUP_DECLINED_KEY: &str = "desktop.setupDeclined";

/// What asked for the setup window.
#[cfg(target_os = "macos")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    Launch,
    Menu,
    Reopen,
}

/// Requests for the main thread, which owns the setup window.
#[cfg(target_os = "macos")]
#[derive(Debug)]
pub enum SetupEvent {
    Show,
    Command(bridge::SetupCommand),
    Push(view::SetupView),
    Close,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SetupState {
    shell_installed: bool,
    accessibility_granted: bool,
}

/// "Not now" is remembered, so launch stops offering setup.
fn should_offer_at_launch(state: SetupState, declined: bool) -> bool {
    !state.shell_installed && !declined
}

fn statuses(state: SetupState) -> (view::ShellStatus, view::AccessibilityStatus) {
    let shell = if state.shell_installed {
        view::ShellStatus::Done
    } else {
        view::ShellStatus::Needed
    };
    let accessibility = if state.accessibility_granted {
        view::AccessibilityStatus::Granted
    } else {
        view::AccessibilityStatus::Needed
    };
    (shell, accessibility)
}

fn display_path(path: &Path, home: Option<&Path>) -> String {
    match home.and_then(|home| path.strip_prefix(home).ok()) {
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

fn fallback_command(cli_path: Option<&Path>) -> String {
    match cli_path {
        Some(path) => format!("{} setup", path.display()),
        None => format!("{CLI_BINARY_NAME} setup"),
    }
}

#[cfg(test)]
mod tests {
    use super::view::{
        AccessibilityStatus,
        ShellStatus,
    };
    use super::*;

    const NEITHER: SetupState = SetupState {
        shell_installed: false,
        accessibility_granted: false,
    };
    const NO_SHELL: SetupState = SetupState {
        shell_installed: false,
        accessibility_granted: true,
    };
    const NO_ACCESS: SetupState = SetupState {
        shell_installed: true,
        accessibility_granted: false,
    };
    const COMPLETE: SetupState = SetupState {
        shell_installed: true,
        accessibility_granted: true,
    };

    #[test]
    fn offer_at_launch() {
        assert!(should_offer_at_launch(NEITHER, false));
        assert!(should_offer_at_launch(NO_SHELL, false));
        assert!(!should_offer_at_launch(NO_ACCESS, false));
        assert!(!should_offer_at_launch(COMPLETE, false));
        assert!(!should_offer_at_launch(NEITHER, true));
        assert!(!should_offer_at_launch(NO_SHELL, true));
        assert!(!should_offer_at_launch(NO_ACCESS, true));
        assert!(!should_offer_at_launch(COMPLETE, true));
    }

    #[test]
    fn statuses_follow_state() {
        assert_eq!(statuses(NEITHER), (ShellStatus::Needed, AccessibilityStatus::Needed));
        assert_eq!(statuses(NO_SHELL), (ShellStatus::Needed, AccessibilityStatus::Granted));
        assert_eq!(statuses(NO_ACCESS), (ShellStatus::Done, AccessibilityStatus::Needed));
        assert_eq!(statuses(COMPLETE), (ShellStatus::Done, AccessibilityStatus::Granted));
    }

    #[test]
    fn display_path_shortens_home() {
        let home = Path::new("/Users/someone");
        assert_eq!(
            display_path(Path::new("/Users/someone/.zshrc"), Some(home)),
            "~/.zshrc"
        );
        assert_eq!(
            display_path(Path::new("/Users/someone/zsh/.zprofile"), Some(home)),
            "~/zsh/.zprofile"
        );
        assert_eq!(display_path(Path::new("/etc/zshrc"), Some(home)), "/etc/zshrc");
        assert_eq!(display_path(Path::new("/Users/someone/.zshrc"), None), "/Users/someone/.zshrc");
    }

    #[test]
    fn fallback_command_names_the_cli() {
        assert_eq!(
            fallback_command(Some(Path::new("/Applications/ShellCue.app/Contents/MacOS/shellcue"))),
            "/Applications/ShellCue.app/Contents/MacOS/shellcue setup"
        );
        assert_eq!(fallback_command(None), "shellcue setup");
    }
}
