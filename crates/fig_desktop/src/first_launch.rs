//! Offering the guided setup inside the app through native dialogs.

use std::path::Path;

use fig_util::consts::{
    CLI_BINARY_NAME,
    PRODUCT_NAME,
};

const SET_UP: &str = "Set up";
const ACCESSIBILITY_STEP: &str =
    "Ask for the macOS Accessibility permission, so it can place the completion window at your cursor.";

const NOT_NOW: &str = "Not now";
const ENABLE_INPUT_METHOD: &str = "Yes, enable it";
#[cfg(target_os = "macos")]
const SETUP_DECLINED_KEY: &str = "desktop.setupDeclined";

/// What started the setup flow.
#[cfg(target_os = "macos")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    Launch,
    Menu,
    Reopen,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SetupState {
    shell_installed: bool,
    accessibility_granted: bool,
}

impl SetupState {
    fn complete(self) -> bool {
        self.shell_installed && self.accessibility_granted
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReopenAction {
    Setup,
    RunningNotice,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum InputMethodStep {
    NotAsked,
    Enabled,
    Failed(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IntroChoice {
    Proceed,
    Decline,
    Dismiss,
}

#[derive(Debug)]
struct DialogText {
    title: String,
    body: String,
}

/// "Not now" is remembered, so launch stops offering setup.
fn should_offer_at_launch(state: SetupState, declined: bool) -> bool {
    !state.shell_installed && !declined
}

/// A deliberate double-click ignores an earlier "Not now".
fn reopen_action(state: SetupState) -> ReopenAction {
    if state.complete() {
        ReopenAction::RunningNotice
    } else {
        ReopenAction::Setup
    }
}

/// A dismissal is not a "Not now", so it is not remembered.
fn intro_choice(result: &rfd::MessageDialogResult) -> IntroChoice {
    match result {
        rfd::MessageDialogResult::Custom(label) if label == SET_UP => IntroChoice::Proceed,
        rfd::MessageDialogResult::Custom(label) if label == NOT_NOW => IntroChoice::Decline,
        _ => IntroChoice::Dismiss,
    }
}

fn enables_input_method(result: &rfd::MessageDialogResult) -> bool {
    matches!(result, rfd::MessageDialogResult::Custom(label) if label == ENABLE_INPUT_METHOD)
}

fn display_path(path: &Path, home: Option<&Path>) -> String {
    match home.and_then(|home| path.strip_prefix(home).ok()) {
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

fn running_notice_text(menu_bar_icon_hidden: bool) -> DialogText {
    let body = if menu_bar_icon_hidden {
        format!("{PRODUCT_NAME} is running. Its menu bar icon is hidden by the app.hideMenubarIcon setting.")
    } else {
        format!("{PRODUCT_NAME} is running. Look for its icon in the menu bar.")
    };
    DialogText {
        title: format!("{PRODUCT_NAME} is running"),
        body,
    }
}

fn already_set_up_text() -> DialogText {
    DialogText {
        title: format!("{PRODUCT_NAME} is set up"),
        body: format!("{PRODUCT_NAME} is already set up. Open a new terminal window if completions do not appear."),
    }
}

fn intro_text(state: SetupState, files: &[String], scripts_dir: &str) -> DialogText {
    let mut steps = vec![];
    if !state.shell_installed {
        let edit = if files.is_empty() {
            format!(
                "Add a short block to your zsh start-up files ({PRODUCT_NAME} could not work out which files; existing files are backed up first)."
            )
        } else {
            let files: String = files.iter().map(|file| format!("\n   {file}")).collect();
            format!(
                "Add a short block at the top and another at the bottom of each of these files, creating them if they do not exist (existing files are backed up first):{files}"
            )
        };
        steps.push(format!("{edit}\n   It also writes its helper scripts in {scripts_dir}."));
    }
    if !state.accessibility_granted {
        steps.push(ACCESSIBILITY_STEP.to_owned());
    }
    let steps: String = steps
        .into_iter()
        .enumerate()
        .map(|(i, step)| format!("{}. {step}\n\n", i + 1))
        .collect();
    DialogText {
        title: format!("Set up {PRODUCT_NAME}"),
        body: format!(
            "{PRODUCT_NAME} shows completions as you type in your terminal. To do that it needs to:\n\n{steps}These changes are made only when you choose {SET_UP}."
        ),
    }
}

fn input_method_text(location: &str) -> DialogText {
    DialogText {
        title: "Input method".to_owned(),
        body: format!(
            "Do you use Kitty, Alacritty, WezTerm or Ghostty? These terminals need the {PRODUCT_NAME} input method to tell {PRODUCT_NAME} where your cursor is. If you choose Yes, {PRODUCT_NAME} adds its input method to {location} and macOS will ask you to confirm."
        ),
    }
}

fn shell_error_text(err: &str, cli_path: Option<&Path>) -> DialogText {
    let command = match cli_path {
        Some(path) => format!("{} setup", path.display()),
        None => format!("{CLI_BINARY_NAME} setup"),
    };
    DialogText {
        title: format!("{PRODUCT_NAME} setup is not finished"),
        body: format!(
            "{PRODUCT_NAME} could not edit your shell configuration: {err}\n\nYou can run setup in a terminal instead:\n{command}"
        ),
    }
}

// The sentences shared with `shellcue setup` repeat its wording in q_cli's installation.rs.
fn closing_text(state: SetupState, input_method: &InputMethodStep) -> DialogText {
    let mut lines = vec![];
    if state.accessibility_granted {
        lines.push(format!("{PRODUCT_NAME} has the Accessibility permission."));
    } else {
        lines.push(format!("{PRODUCT_NAME} has not been granted the Accessibility permission yet."));
        lines.push(format!(
            "Grant {PRODUCT_NAME} the Accessibility permission in System Settings > Privacy & Security > Accessibility."
        ));
        lines.push(format!(
            "Then choose \"Set up {PRODUCT_NAME}…\" from the menu bar icon, or open {PRODUCT_NAME} again, to confirm."
        ));
    }
    match input_method {
        InputMethodStep::NotAsked => {},
        InputMethodStep::Enabled => lines.push(
            "The input method is enabled. Restart Kitty, Alacritty, WezTerm or Ghostty if it is open.".to_owned(),
        ),
        InputMethodStep::Failed(err) => {
            lines.push(format!("The input method could not be enabled: {err}"));
            lines.push(format!("To try again later, run `{CLI_BINARY_NAME} integrations install input-method`."));
        },
    }
    if state.shell_installed {
        lines.push("Open a new terminal window so the shell integration loads.".to_owned());
    }
    let input_method_failed = matches!(input_method, InputMethodStep::Failed(_));
    let title = if state.complete() && !input_method_failed {
        format!("{PRODUCT_NAME} is ready")
    } else {
        format!("{PRODUCT_NAME} setup is not finished")
    };
    DialogText {
        title,
        body: lines.join("\n"),
    }
}

#[cfg(target_os = "macos")]
static RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[cfg(target_os = "macos")]
struct RunningGuard;

#[cfg(target_os = "macos")]
impl Drop for RunningGuard {
    fn drop(&mut self) {
        RUNNING.store(false, std::sync::atomic::Ordering::Release);
    }
}

#[cfg(target_os = "macos")]
pub async fn run(trigger: Trigger) {
    use fig_install::{
        InstallComponents,
        install,
    };
    use fig_integrations::Integration as _;
    use fig_integrations::input_method::InputMethod;
    use fig_os_shim::Env;
    use fig_util::accessibility_wait::{
        ACCESSIBILITY_POLL_INTERVAL,
        ACCESSIBILITY_WAIT_LIMIT,
        AccessibilityOutcome,
        attempts_for,
        wait_until_granted,
    };
    use macos_utils::accessibility::accessibility_is_enabled;
    use macos_utils::bundle::get_bundle_path_for_executable;
    use rfd::{
        MessageButtons,
        MessageLevel,
    };
    use tracing::warn;

    if RUNNING.swap(true, std::sync::atomic::Ordering::AcqRel) {
        return;
    }
    let _guard = RunningGuard;

    let env = Env::new();
    let (shell_installed, files) = shell_status(&env).await;
    let mut state = SetupState {
        shell_installed,
        accessibility_granted: accessibility_is_enabled(),
    };

    match trigger {
        Trigger::Launch => {
            let declined = fig_settings::state::get_bool_or(SETUP_DECLINED_KEY, false);
            if !should_offer_at_launch(state, declined) {
                return;
            }
        },
        Trigger::Reopen => {
            if reopen_action(state) == ReopenAction::RunningNotice {
                let hidden = fig_settings::settings::get_bool_or("app.hideMenubarIcon", false);
                let DialogText { title, body } = running_notice_text(hidden);
                show_dialog(MessageLevel::Info, title, body, MessageButtons::Ok).await;
                return;
            }
        },
        Trigger::Menu => {
            if state.complete() {
                let DialogText { title, body } = already_set_up_text();
                show_dialog(MessageLevel::Info, title, body, MessageButtons::Ok).await;
                return;
            }
        },
    }

    let home = fig_util::directories::home_dir().ok();
    // The same directory the shell integrations write their scripts to.
    let scripts_dir = match fig_util::directories::fig_data_dir() {
        Ok(dir) => display_path(&dir.join("shell"), home.as_deref()),
        Err(_) => "its data directory".to_owned(),
    };
    let DialogText { title, body } = intro_text(state, &files, &scripts_dir);
    let buttons = MessageButtons::OkCancelCustom(SET_UP.to_owned(), NOT_NOW.to_owned());
    match intro_choice(&show_dialog(MessageLevel::Info, title, body, buttons).await) {
        IntroChoice::Proceed => {},
        IntroChoice::Decline => {
            if let Err(err) = fig_settings::state::set_value(SETUP_DECLINED_KEY, true) {
                warn!(%err, "Failed to remember that setup was declined");
            }
            return;
        },
        IntroChoice::Dismiss => return,
    }

    if !state.shell_installed {
        if let Err(err) = install(InstallComponents::SHELL_INTEGRATIONS, &env).await {
            let cli_path = get_bundle_path_for_executable(CLI_BINARY_NAME);
            let DialogText { title, body } = shell_error_text(&err.to_string(), cli_path.as_deref());
            show_dialog(MessageLevel::Error, title, body, MessageButtons::Ok).await;
            return;
        }
        state.shell_installed = true;
    }

    let mut input_method = InputMethodStep::NotAsked;
    if InputMethod::default().is_installed().await.is_err() {
        let location = display_path(&InputMethod::input_method_directory(), home.as_deref());
        let DialogText { title, body } = input_method_text(&location);
        let buttons = MessageButtons::OkCancelCustom(ENABLE_INPUT_METHOD.to_owned(), "No".to_owned());
        if enables_input_method(&show_dialog(MessageLevel::Info, title, body, buttons).await) {
            input_method = match install(InstallComponents::INPUT_METHOD, &env).await {
                Ok(()) => InputMethodStep::Enabled,
                Err(err) => InputMethodStep::Failed(err.to_string()),
            };
        }
    }

    if !state.accessibility_granted {
        crate::local_ipc::commands::prompt_for_accessibility_permission().await.ok();
        let attempts = attempts_for(ACCESSIBILITY_WAIT_LIMIT, ACCESSIBILITY_POLL_INTERVAL);
        let outcome = wait_until_granted(
            || async { accessibility_is_enabled() },
            attempts,
            ACCESSIBILITY_POLL_INTERVAL,
        )
        .await;
        state.accessibility_granted = outcome == AccessibilityOutcome::Granted;
    }

    let DialogText { title, body } = closing_text(state, &input_method);
    show_dialog(MessageLevel::Info, title, body, MessageButtons::Ok).await;
}

/// Whether every shell integration is installed, and the files of those that are not.
#[cfg(target_os = "macos")]
async fn shell_status(env: &fig_os_shim::Env) -> (bool, Vec<String>) {
    use fig_integrations::shell::ShellExt;
    use fig_util::Shell;
    use tracing::warn;

    let home = fig_util::directories::home_dir().ok();
    let mut installed = true;
    let mut files = vec![];
    for shell in Shell::all() {
        match shell.get_shell_integrations(env) {
            Ok(integrations) => {
                for integration in integrations {
                    if integration.is_installed().await.is_err() {
                        installed = false;
                        files.push(display_path(&integration.path(), home.as_deref()));
                    }
                }
            },
            Err(err) => {
                warn!(%err, "Failed to get the shell integrations for {shell}");
                installed = false;
            },
        }
    }
    (installed, files)
}

/// The rfd future may not be `Send`, and the main thread must stay free for the event loop.
#[cfg(target_os = "macos")]
async fn show_dialog(
    level: rfd::MessageLevel,
    title: String,
    body: String,
    buttons: rfd::MessageButtons,
) -> rfd::MessageDialogResult {
    let handle = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
        handle.block_on(async move {
            rfd::AsyncMessageDialog::new()
                .set_level(level)
                .set_title(title)
                .set_description(body)
                .set_buttons(buttons)
                .show()
                .await
        })
    })
    .await
    .unwrap_or(rfd::MessageDialogResult::Cancel)
}

#[cfg(test)]
mod tests {
    use rfd::MessageDialogResult;

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

    fn files() -> Vec<String> {
        vec!["~/.zshrc".to_owned(), "~/.zprofile".to_owned()]
    }

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
    fn reopen_decision() {
        assert_eq!(reopen_action(NEITHER), ReopenAction::Setup);
        assert_eq!(reopen_action(NO_SHELL), ReopenAction::Setup);
        assert_eq!(reopen_action(NO_ACCESS), ReopenAction::Setup);
        assert_eq!(reopen_action(COMPLETE), ReopenAction::RunningNotice);
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

    #[track_caller]
    fn assert_text(text: DialogText, title: &str, body: &str) {
        assert_eq!(text.title, title);
        assert_eq!(text.body, body);
    }

    const SCRIPTS: &str = "~/Library/Application Support/shellcue/shell";

    #[test]
    fn intro_both_needed() {
        assert_text(
            intro_text(NEITHER, &files(), SCRIPTS),
            "Set up ShellCue",
            "ShellCue shows completions as you type in your terminal. To do that it needs to:\n\n\
             1. Add a short block at the top and another at the bottom of each of these files, creating them if they do not exist (existing files are backed up first):\n   \
             ~/.zshrc\n   \
             ~/.zprofile\n   \
             It also writes its helper scripts in ~/Library/Application Support/shellcue/shell.\n\n\
             2. Ask for the macOS Accessibility permission, so it can place the completion window at your cursor.\n\n\
             These changes are made only when you choose Set up.",
        );
    }

    #[test]
    fn intro_shell_only() {
        assert_text(
            intro_text(NO_SHELL, &files(), SCRIPTS),
            "Set up ShellCue",
            "ShellCue shows completions as you type in your terminal. To do that it needs to:\n\n\
             1. Add a short block at the top and another at the bottom of each of these files, creating them if they do not exist (existing files are backed up first):\n   \
             ~/.zshrc\n   \
             ~/.zprofile\n   \
             It also writes its helper scripts in ~/Library/Application Support/shellcue/shell.\n\n\
             These changes are made only when you choose Set up.",
        );
    }

    #[test]
    fn intro_shell_without_files() {
        assert_text(
            intro_text(NO_SHELL, &[], SCRIPTS),
            "Set up ShellCue",
            "ShellCue shows completions as you type in your terminal. To do that it needs to:\n\n\
             1. Add a short block to your zsh start-up files (ShellCue could not work out which files; existing files are backed up first).\n   \
             It also writes its helper scripts in ~/Library/Application Support/shellcue/shell.\n\n\
             These changes are made only when you choose Set up.",
        );
    }

    #[test]
    fn intro_accessibility_only() {
        assert_text(
            intro_text(NO_ACCESS, &[], SCRIPTS),
            "Set up ShellCue",
            "ShellCue shows completions as you type in your terminal. To do that it needs to:\n\n\
             1. Ask for the macOS Accessibility permission, so it can place the completion window at your cursor.\n\n\
             These changes are made only when you choose Set up.",
        );
    }

    #[test]
    fn intro_choices() {
        assert_eq!(intro_choice(&MessageDialogResult::Custom("Set up".to_owned())), IntroChoice::Proceed);
        assert_eq!(intro_choice(&MessageDialogResult::Custom("Not now".to_owned())), IntroChoice::Decline);
        assert_eq!(intro_choice(&MessageDialogResult::Cancel), IntroChoice::Dismiss);
        assert_eq!(intro_choice(&MessageDialogResult::Ok), IntroChoice::Dismiss);
        assert_eq!(intro_choice(&MessageDialogResult::Custom("Other".to_owned())), IntroChoice::Dismiss);
    }

    #[test]
    fn input_method_choices() {
        assert!(enables_input_method(&MessageDialogResult::Custom("Yes, enable it".to_owned())));
        assert!(!enables_input_method(&MessageDialogResult::Custom("No".to_owned())));
        assert!(!enables_input_method(&MessageDialogResult::Custom("Yes".to_owned())));
        assert!(!enables_input_method(&MessageDialogResult::Yes));
        assert!(!enables_input_method(&MessageDialogResult::Ok));
        assert!(!enables_input_method(&MessageDialogResult::Cancel));
    }

    #[test]
    fn input_method_question() {
        assert_text(
            input_method_text("~/Library/Input Methods"),
            "Input method",
            "Do you use Kitty, Alacritty, WezTerm or Ghostty? These terminals need the ShellCue input method to tell ShellCue where your cursor is. \
             If you choose Yes, ShellCue adds its input method to ~/Library/Input Methods and macOS will ask you to confirm.",
        );
    }

    #[test]
    fn running_notice() {
        assert_text(
            running_notice_text(false),
            "ShellCue is running",
            "ShellCue is running. Look for its icon in the menu bar.",
        );
        assert_text(
            running_notice_text(true),
            "ShellCue is running",
            "ShellCue is running. Its menu bar icon is hidden by the app.hideMenubarIcon setting.",
        );
    }

    #[test]
    fn already_set_up() {
        assert_text(
            already_set_up_text(),
            "ShellCue is set up",
            "ShellCue is already set up. Open a new terminal window if completions do not appear.",
        );
    }

    #[test]
    fn shell_error() {
        assert_text(
            shell_error_text(
                "Permission denied",
                Some(Path::new("/Applications/ShellCue.app/Contents/MacOS/shellcue")),
            ),
            "ShellCue setup is not finished",
            "ShellCue could not edit your shell configuration: Permission denied\n\n\
             You can run setup in a terminal instead:\n\
             /Applications/ShellCue.app/Contents/MacOS/shellcue setup",
        );
        assert_text(
            shell_error_text("Permission denied", None),
            "ShellCue setup is not finished",
            "ShellCue could not edit your shell configuration: Permission denied\n\n\
             You can run setup in a terminal instead:\n\
             shellcue setup",
        );
    }

    #[test]
    fn closing_all_good() {
        assert_text(
            closing_text(COMPLETE, &InputMethodStep::NotAsked),
            "ShellCue is ready",
            "ShellCue has the Accessibility permission.\n\
             Open a new terminal window so the shell integration loads.",
        );
    }

    #[test]
    fn closing_shell_not_installed() {
        assert_text(
            closing_text(NO_SHELL, &InputMethodStep::NotAsked),
            "ShellCue setup is not finished",
            "ShellCue has the Accessibility permission.",
        );
    }

    #[test]
    fn closing_accessibility_not_granted() {
        assert_text(
            closing_text(NO_ACCESS, &InputMethodStep::NotAsked),
            "ShellCue setup is not finished",
            "ShellCue has not been granted the Accessibility permission yet.\n\
             Grant ShellCue the Accessibility permission in System Settings > Privacy & Security > Accessibility.\n\
             Then choose \"Set up ShellCue…\" from the menu bar icon, or open ShellCue again, to confirm.\n\
             Open a new terminal window so the shell integration loads.",
        );
    }

    #[test]
    fn closing_input_method_enabled() {
        assert_text(
            closing_text(COMPLETE, &InputMethodStep::Enabled),
            "ShellCue is ready",
            "ShellCue has the Accessibility permission.\n\
             The input method is enabled. Restart Kitty, Alacritty, WezTerm or Ghostty if it is open.\n\
             Open a new terminal window so the shell integration loads.",
        );
    }

    #[test]
    fn closing_input_method_failed() {
        assert_text(
            closing_text(COMPLETE, &InputMethodStep::Failed("OSStatus error code: -50".to_owned())),
            "ShellCue setup is not finished",
            "ShellCue has the Accessibility permission.\n\
             The input method could not be enabled: OSStatus error code: -50\n\
             To try again later, run `shellcue integrations install input-method`.\n\
             Open a new terminal window so the shell integration loads.",
        );
    }

    #[test]
    fn closing_accessibility_not_granted_input_method_enabled() {
        assert_text(
            closing_text(NO_ACCESS, &InputMethodStep::Enabled),
            "ShellCue setup is not finished",
            "ShellCue has not been granted the Accessibility permission yet.\n\
             Grant ShellCue the Accessibility permission in System Settings > Privacy & Security > Accessibility.\n\
             Then choose \"Set up ShellCue…\" from the menu bar icon, or open ShellCue again, to confirm.\n\
             The input method is enabled. Restart Kitty, Alacritty, WezTerm or Ghostty if it is open.\n\
             Open a new terminal window so the shell integration loads.",
        );
    }

    #[test]
    fn closing_accessibility_not_granted_input_method_failed() {
        assert_text(
            closing_text(NO_ACCESS, &InputMethodStep::Failed("OSStatus error code: -50".to_owned())),
            "ShellCue setup is not finished",
            "ShellCue has not been granted the Accessibility permission yet.\n\
             Grant ShellCue the Accessibility permission in System Settings > Privacy & Security > Accessibility.\n\
             Then choose \"Set up ShellCue…\" from the menu bar icon, or open ShellCue again, to confirm.\n\
             The input method could not be enabled: OSStatus error code: -50\n\
             To try again later, run `shellcue integrations install input-method`.\n\
             Open a new terminal window so the shell integration loads.",
        );
    }
}
