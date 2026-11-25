//! Runs the setup commands off the main thread and pushes each new view to the page.

use fig_install::{
    InstallComponents,
    install,
};
use fig_integrations::input_method::InputMethod;
use fig_os_shim::Env;
use fig_util::accessibility_wait::{
    ACCESSIBILITY_POLL_INTERVAL,
    ACCESSIBILITY_WAIT_LIMIT,
    AccessibilityOutcome,
    attempts_for,
    wait_until_granted,
};
use fig_util::consts::CLI_BINARY_NAME;
use macos_utils::accessibility::accessibility_is_enabled;
use macos_utils::bundle::get_bundle_path_for_executable;
use parking_lot::Mutex;
use tracing::{
    debug,
    warn,
};

use super::bridge::{
    SetupAction,
    SetupCommand,
};
use super::view::{
    AccessibilityStatus,
    AccessibilityStep,
    InputMethodStatus,
    InputMethodStep,
    LaunchAtLoginStatus,
    LaunchAtLoginStep,
    Recheck,
    SetupView,
    ShellStatus,
    ShellStep,
    accepts,
    begin,
    input_method_status,
    recheck_result,
};
use super::{
    SETUP_DECLINED_KEY,
    SetupEvent,
    SetupState,
    Trigger,
    display_path,
    fallback_command,
    should_offer_at_launch,
    statuses,
};
use crate::EventLoopProxy;
use crate::event::Event;
use crate::launch_at_login::{
    agent_path,
    launch_at_login_enabled,
    set_launch_at_login,
};

struct Slot {
    view: Option<SetupView>,
    /// Bumped whenever an action begins or finishes, so a recheck that spans either is discarded.
    generation: u64,
}

/// The latest view, written under the lock before each push and kept after the window closes.
static VIEW: Mutex<Slot> = Mutex::new(Slot {
    view: None,
    generation: 0,
});

static RECHECK: Mutex<Recheck> = Mutex::new(Recheck::Idle);

/// Clears `busy` if the action panics or is cancelled; `finish` defuses it.
struct Busy {
    defused: bool,
}

impl Drop for Busy {
    fn drop(&mut self) {
        if self.defused {
            return;
        }
        if let Some(view) = VIEW.lock().view.as_mut() {
            view.busy = false;
        }
    }
}

/// Lets a later refresh start if this one panics or is cancelled; `refresh` defuses it.
struct Rechecking {
    defused: bool,
}

impl Drop for Rechecking {
    fn drop(&mut self) {
        if self.defused {
            return;
        }
        *RECHECK.lock() = Recheck::Idle;
    }
}

fn send(proxy: &EventLoopProxy, event: SetupEvent) {
    if proxy.send_event(Event::Setup(event)).is_err() {
        warn!("Event loop closed; setup event dropped");
    }
}

fn push(proxy: &EventLoopProxy, view: SetupView) {
    send(proxy, SetupEvent::Push(view));
}

pub fn close(proxy: &EventLoopProxy) {
    send(proxy, SetupEvent::Close);
}

pub fn show(trigger: Trigger, proxy: &EventLoopProxy) {
    debug!(?trigger, "Setup window requested");
    match trigger {
        Trigger::Launch => {
            let proxy = proxy.clone();
            tokio::spawn(async move {
                let env = Env::new();
                let (state, _) = current_state(&env).await;
                let declined = fig_settings::state::get_bool_or(SETUP_DECLINED_KEY, false);
                if should_offer_at_launch(state, declined) {
                    send(&proxy, SetupEvent::Show);
                } else {
                    debug!(?state, declined, "Setup not offered at launch");
                }
            });
        },
        Trigger::Menu | Trigger::Reopen => send(proxy, SetupEvent::Show),
    }
}

/// Called on the main thread, so the work is spawned.
pub fn handle_command(command: SetupCommand, proxy: &EventLoopProxy) {
    debug!(?command, "Setup command");
    match command {
        SetupCommand::Ready => refresh(proxy),
        SetupCommand::NotNow => {
            let proxy = proxy.clone();
            tokio::task::spawn_blocking(move || {
                if let Err(err) = fig_settings::state::set_value(SETUP_DECLINED_KEY, true) {
                    warn!(%err, "Failed to remember that setup was declined");
                }
                close(&proxy);
            });
        },
        SetupCommand::Close => close(proxy),
        SetupCommand::Action(action) => start(action, proxy),
    }
}

/// Rechecks the system unless an action is in flight, whose own push will follow.
pub fn refresh(proxy: &EventLoopProxy) {
    if !RECHECK.lock().request() {
        return;
    }
    let proxy = proxy.clone();
    tokio::spawn(async move {
        let mut guard = Rechecking { defused: false };
        loop {
            recheck(&proxy).await;
            if !RECHECK.lock().finish() {
                break;
            }
        }
        // `Recheck::finish` has set `Idle`; the guard could reset a refresh started since.
        guard.defused = true;
    });
}

async fn recheck(proxy: &EventLoopProxy) {
    let (current, started) = {
        let slot = VIEW.lock();
        (slot.view.clone(), slot.generation)
    };
    if let Some(view) = current.filter(|view| view.busy) {
        push(proxy, view);
        return;
    }
    let fresh = current_view().await;
    let view = {
        let mut slot = VIEW.lock();
        let Some(view) = recheck_result(slot.view.as_ref(), fresh, started, slot.generation) else {
            return;
        };
        slot.view = Some(view.clone());
        view
    };
    push(proxy, view);
}

fn start(action: SetupAction, proxy: &EventLoopProxy) {
    let view = {
        let mut slot = VIEW.lock();
        let Some(view) = slot.view.as_mut() else {
            debug!(?action, "Setup command ignored before the first view");
            return;
        };
        if !accepts(SetupCommand::Action(action), view) {
            debug!(?action, busy = view.busy, "Setup command ignored");
            return;
        }
        begin(view, action);
        let view = view.clone();
        slot.generation += 1;
        view
    };
    push(proxy, view);

    let busy = Busy { defused: false };
    let proxy = proxy.clone();
    tokio::spawn(async move {
        let env = Env::new();
        match action {
            SetupAction::Shell => {
                let result = install(InstallComponents::SHELL_INTEGRATIONS, &env).await;
                debug!(ok = result.is_ok(), "Shell integration step finished");
                finish(&proxy, busy, |view| match result {
                    Ok(()) => view.shell.status = ShellStatus::Done,
                    Err(err) => {
                        view.shell.status = ShellStatus::Failed;
                        view.shell.error = Some(err.to_string());
                    },
                });
            },
            SetupAction::Accessibility => {
                crate::local_ipc::commands::prompt_for_accessibility_permission().await.ok();
                let attempts = attempts_for(ACCESSIBILITY_WAIT_LIMIT, ACCESSIBILITY_POLL_INTERVAL);
                let outcome = wait_until_granted(
                    || async { accessibility_is_enabled() },
                    attempts,
                    ACCESSIBILITY_POLL_INTERVAL,
                )
                .await;
                debug!(?outcome, "Accessibility step finished");
                finish(&proxy, busy, |view| {
                    view.accessibility.status = if outcome == AccessibilityOutcome::Granted {
                        AccessibilityStatus::Granted
                    } else {
                        AccessibilityStatus::TimedOut
                    };
                });
            },
            SetupAction::InputMethod => {
                let result = install(InstallComponents::INPUT_METHOD, &env).await;
                debug!(ok = result.is_ok(), "Input method step finished");
                finish(&proxy, busy, |view| match result {
                    Ok(()) => view.input_method.status = InputMethodStatus::Enabled,
                    Err(err) => {
                        view.input_method.status = InputMethodStatus::Failed;
                        view.input_method.error = Some(err.to_string());
                    },
                });
            },
            SetupAction::LaunchAtLogin => {
                let tick_proxy = proxy.clone();
                let result = match tokio::task::spawn_blocking(move || {
                    let result = set_launch_at_login(true);
                    crate::tray::show_launch_at_login(&tick_proxy);
                    result
                })
                .await
                {
                    Ok(result) => result,
                    Err(err) => Err(err.into()),
                };
                debug!(ok = result.is_ok(), "Launch at login step finished");
                finish(&proxy, busy, |view| match result {
                    Ok(()) => view.launch_at_login.status = LaunchAtLoginStatus::Enabled,
                    Err(err) => {
                        view.launch_at_login.status = LaunchAtLoginStatus::Failed;
                        view.launch_at_login.error = Some(format!("{err:#}"));
                    },
                });
            },
        }
    });
}

fn finish(proxy: &EventLoopProxy, mut busy: Busy, outcome: impl FnOnce(&mut SetupView)) {
    let view = {
        let mut slot = VIEW.lock();
        slot.generation += 1;
        slot.view.as_mut().map(|view| {
            outcome(view);
            view.busy = false;
            view.clone()
        })
    };
    // `busy` was cleared under the lock above; the guard could clear a later action's.
    busy.defused = true;
    if let Some(view) = view {
        push(proxy, view);
    }
}

async fn current_state(env: &Env) -> (SetupState, Vec<String>) {
    let (shell_installed, files) = shell_status(env).await;
    let state = SetupState {
        shell_installed,
        accessibility_granted: accessibility_is_enabled(),
    };
    (state, files)
}

/// Only reads the system.
async fn current_view() -> SetupView {
    let env = Env::new();
    let (state, files) = current_state(&env).await;
    let (shell_status, accessibility_status) = statuses(state);
    let home = fig_util::directories::home_dir().ok();
    // The same directory the shell integrations write their scripts to.
    let scripts_dir = match fig_util::directories::fig_data_dir() {
        Ok(dir) => display_path(&dir.join("shell"), home.as_deref()),
        Err(_) => "its data directory".to_owned(),
    };
    let bin_dir = match fig_util::directories::home_local_bin() {
        Ok(dir) => display_path(&dir, home.as_deref()),
        Err(_) => "~/.local/bin".to_owned(),
    };
    let cli_path = get_bundle_path_for_executable(CLI_BINARY_NAME);
    let input_method_status = current_input_method_status().await;
    let launch_at_login_status = if launch_at_login_enabled() {
        LaunchAtLoginStatus::Enabled
    } else {
        LaunchAtLoginStatus::NotEnabled
    };
    let launch_at_login_file = match agent_path() {
        Ok(path) => display_path(&path, home.as_deref()),
        Err(_) => "its LaunchAgents file".to_owned(),
    };
    SetupView {
        shell: ShellStep {
            status: shell_status,
            files,
            scripts_dir,
            bin_dir,
            error: None,
            command: fallback_command(cli_path.as_deref()),
        },
        accessibility: AccessibilityStep {
            status: accessibility_status,
        },
        input_method: InputMethodStep {
            status: input_method_status,
            directory: display_path(&InputMethod::input_method_directory(), home.as_deref()),
            error: None,
        },
        launch_at_login: LaunchAtLoginStep {
            status: launch_at_login_status,
            file: launch_at_login_file,
            error: None,
        },
        menu_bar_icon_hidden: fig_settings::settings::get_bool_or("app.hideMenubarIcon", false),
        busy: false,
    }
}

/// Only reads: `is_installed` runs the CLI, which selects the input source and writes state.
async fn current_input_method_status() -> InputMethodStatus {
    let input_method = InputMethod::default();
    let linked = input_method.is_linked().await;
    let enabled = input_method.is_enabled().unwrap_or(false);
    input_method_status(linked, enabled)
}

/// Whether every shell integration is installed, and the files of those that are not.
async fn shell_status(env: &Env) -> (bool, Vec<String>) {
    use fig_integrations::shell::ShellExt;
    use fig_util::Shell;

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
