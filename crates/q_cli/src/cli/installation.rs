//! Installation, uninstallation, and update of the CLI.

use std::process::ExitCode;

use anstream::{
    eprintln,
    println,
};
use crossterm::style::Stylize;
use eyre::{
    Result,
    bail,
};
use fig_install::{
    InstallComponents,
    install,
    install_script_integrations,
};
#[cfg(target_os = "linux")]
use fig_integrations::gnome_extension::SWITCH_ON_COMMAND;
use fig_os_shim::Env;
use fig_util::{
    CLI_BINARY_NAME,
    PRODUCT_NAME,
};
use tracing::warn;

#[cfg(any(target_os = "macos", test))]
use crate::util::accessibility::AccessibilityOutcome;
use crate::util::choose;

/// Stands in for the Linux-only `fig_integrations::gnome_extension::SWITCH_ON_COMMAND`.
#[cfg(all(test, not(target_os = "linux")))]
const SWITCH_ON_COMMAND: &str = "integrations install gnome-shell-extension";

/// How the guided Accessibility step of `setup` ended.
#[cfg(any(target_os = "macos", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AccessibilityStep {
    /// `--no-confirm`, `--global`, or an install narrowed by a flag.
    NotAttempted,
    #[cfg(any(target_os = "macos", test))]
    AppNotStarted,
    #[cfg(any(target_os = "macos", test))]
    Failed,
    #[cfg(any(target_os = "macos", test))]
    Finished(AccessibilityOutcome),
}

pub async fn install_cli(
    install_components: InstallComponents,
    no_confirm: bool,
    force: bool,
    global: bool,
) -> Result<ExitCode> {
    let env = Env::new();

    #[cfg(unix)]
    {
        use nix::unistd::geteuid;
        if geteuid().is_root() && !global {
            eprintln!("{}", "Installing as root is not supported.".red().bold());
            if !force {
                eprintln!(
                    "{}",
                    "If you know what you're doing, run the command again with --force.".red()
                );
                return Ok(ExitCode::FAILURE);
            }
        }
    }

    if install_components.contains(InstallComponents::SHELL_INTEGRATIONS) && !global {
        let mut manual_install = if no_confirm {
            false
        } else {
            if !dialoguer::console::user_attended() {
                eyre::bail!("You must run with --no-confirm if unattended");
            }

            #[cfg(target_os = "macos")]
            let prompt = format!(
                "Do you want {CLI_BINARY_NAME} to modify your shell config and link the {CLI_BINARY_NAME} and {} commands into ~/.local/bin, replacing anything there with those names (you will have to manually do this otherwise)?",
                fig_util::PTY_BINARY_NAME,
            );
            #[cfg(not(target_os = "macos"))]
            let prompt = format!("Let {CLI_BINARY_NAME} edit your shell config? (Otherwise you do it by hand.)");
            match choose(prompt, &["Yes", "No"])? {
                Some(1) => true,
                Some(_) => false,
                None => bail!("No option selected"),
            }
        };
        if !manual_install {
            if let Err(err) = install(InstallComponents::SHELL_INTEGRATIONS, &env).await {
                println!("{}", "Could not automatically install:".bold());
                println!("{err}");
                manual_install = true;
            }
        }
        if !no_confirm && manual_install {
            let shell_dir = fig_util::directories::fig_data_dir_utf8()?.join("shell");
            let shell_dir = shell_dir
                .strip_prefix(fig_util::directories::home_dir()?)
                .unwrap_or(&shell_dir);

            println!();
            println!("To install the integrations manually, you will have to add the following to your rc files");
            println!("This step is required for the application to function properly");
            println!();
            println!("At the top of your .zshrc file:");
            println!("zsh:     . \"$HOME/{shell_dir}/zshrc.pre.zsh\"");
            println!();
            println!("At the bottom of your .zshrc file:");
            println!("zsh:     . \"$HOME/{shell_dir}/zshrc.post.zsh\"");
            println!();
            #[cfg(target_os = "macos")]
            {
                let commands_dir = fig_install::macos::commands_dir().ok();
                println!("{}", manual_link_instructions(commands_dir.as_deref()));
                println!();
            }

            if let Err(err) = install_script_integrations().await {
                println!("Could not install required files:");
                println!("{err}");
            }
        }
    }

    if install_components.contains(InstallComponents::SHELL_INTEGRATIONS) && global {
        // TODO: fix this, we do not error out at the moment as the fix is in `install.sh`
        warn!("Global install is not supported for shell integrations");
    }

    if install_components.contains(InstallComponents::INPUT_METHOD) && !no_confirm && !global {
        cfg_if::cfg_if! {
            if #[cfg(target_os = "macos")] {
                if !dialoguer::console::user_attended() {
                    eyre::bail!("You must run with --no-confirm if unattended");
                }

                let full_setup = install_components == InstallComponents::all();
                let terminal_needs_it = fig_util::Terminal::parent_terminal(&fig_os_shim::Context::new())
                    .is_some_and(|terminal| terminal.supports_macos_input_method());

                if should_offer_input_method(full_setup, terminal_needs_it) {
                    println!();
                    println!("To enable support for some terminals like Kitty, Alacritty, and Wezterm,");
                    println!("you must enable our Input Method integration.");
                    println!();
                    println!("To enable the integration, select \"yes\" below and then choose Allow when macOS asks.");
                    println!("macOS warns about typing; the input method only asks the terminal where the cursor is.");
                    println!();

                    match choose("Do you want to enable support for input method backed terminals?", &["Yes", "No"])? {
                        Some(0) => match install(InstallComponents::INPUT_METHOD, &env).await {
                            Ok(()) => {}
                            Err(err) if !full_setup => return Err(err.into()),
                            Err(err) => {
                                println!("Could not enable the input method: {err}");
                                println!("To try again later, run `{CLI_BINARY_NAME} integrations install input-method`.");
                            }
                        },
                        Some(_) => {}
                        None => bail!("No option selected"),
                    }
                } else {
                    println!();
                    println!(
                        "Skipping the input method: this terminal does not appear to need it. If you use Kitty, Alacritty, WezTerm or Ghostty, run `{CLI_BINARY_NAME} integrations install input-method`."
                    );
                }
            }
        }
    }

    #[cfg(target_os = "linux")]
    let linux_next_steps = if is_guided_setup(install_components, no_confirm, global) && person_at_terminal() {
        let mut next_steps = LinuxNextSteps::default();
        gnome_extension_step(&mut next_steps).await?;
        start_at_login_step(&mut next_steps).await?;
        next_steps
    } else {
        LinuxNextSteps::default()
    };

    // Must stay last: its wait leaves a Ctrl-C handler installed for the rest of the process.
    #[cfg(target_os = "macos")]
    let accessibility = if is_guided_setup(install_components, no_confirm, global) {
        accessibility_step().await
    } else {
        AccessibilityStep::NotAttempted
    };

    #[cfg(target_os = "macos")]
    println!("{}", closing_text(accessibility));
    #[cfg(target_os = "linux")]
    println!("{}", linux_closing_text(linux_next_steps));

    #[cfg(target_os = "macos")]
    if accessibility == AccessibilityStep::Finished(AccessibilityOutcome::Interrupted) {
        return Ok(ExitCode::from(130));
    }

    Ok(ExitCode::SUCCESS)
}

#[cfg(target_os = "macos")]
async fn accessibility_step() -> AccessibilityStep {
    use crate::util::accessibility::{
        ACCESSIBILITY_WAIT_LIMIT,
        CtrlC,
        accessibility_granted,
        grant_instructions,
        prompt_and_wait_for_accessibility,
    };
    use crate::util::desktop::{
        LaunchArgs,
        launch_fig_desktop,
    };

    println!();
    println!("{}", "Accessibility permission".bold());

    if let Err(err) = launch_fig_desktop(LaunchArgs {
        wait_for_socket: true,
        verbose: true,
    }) {
        println!("Could not start the {PRODUCT_NAME} app: {err}");
        return AccessibilityStep::AppNotStarted;
    }

    if accessibility_granted().await {
        return AccessibilityStep::Finished(AccessibilityOutcome::AlreadyGranted);
    }

    println!("{PRODUCT_NAME} needs the Accessibility permission to place the completion window at your cursor.");
    for line in grant_instructions() {
        println!("{line}");
    }
    println!();

    match prompt_and_wait_for_accessibility(ACCESSIBILITY_WAIT_LIMIT, CtrlC::Handle).await {
        Ok(outcome) => AccessibilityStep::Finished(outcome),
        Err(err) => {
            println!("Could not ask for the Accessibility permission: {err}");
            AccessibilityStep::Failed
        },
    }
}

/// An explicit `--input-method` always asks; the full setup asks only in a terminal that needs it.
#[cfg(any(target_os = "macos", test))]
fn should_offer_input_method(full_setup: bool, terminal_needs_it: bool) -> bool {
    !full_setup || terminal_needs_it
}

/// Only an interactive, per-user, full setup is guided: through the Accessibility permission on
/// macOS, and through the GNOME Shell extension and start at login on Linux.
#[cfg(any(target_os = "macos", target_os = "linux", test))]
fn is_guided_setup(install_components: InstallComponents, no_confirm: bool, global: bool) -> bool {
    install_components == InstallComponents::all() && !no_confirm && !global
}

/// `choose` answers by itself when stdout is not a terminal, so a guided step needs a person there.
#[cfg(target_os = "linux")]
fn person_at_terminal() -> bool {
    use std::io::IsTerminal;

    std::io::stdin().is_terminal() && std::io::stdout().is_terminal()
}

/// What the GNOME Shell extension step found, for [extension_step_plan].
#[cfg(any(target_os = "linux", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExtensionStepState {
    /// Loaded and switched on.
    On,
    /// Loaded and switched off.
    Off,
    /// Packaged, not loaded, not listed as on.
    OffNotLoaded,
    /// Packaged, not loaded, listed as on.
    ListedNotLoaded,
    /// Any other status, or the status could not be read.
    NotReady,
}

/// What the GNOME Shell extension step does.
#[cfg(any(target_os = "linux", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExtensionStepPlan {
    Nothing,
    SayGnomeOnly,
    ReloginOnly,
    Ask,
    NotReady,
}

#[cfg(any(target_os = "linux", test))]
fn extension_step_plan(wayland: bool, gnome: bool, state: ExtensionStepState) -> ExtensionStepPlan {
    if !wayland {
        return ExtensionStepPlan::Nothing;
    }
    if !gnome {
        return ExtensionStepPlan::SayGnomeOnly;
    }
    match state {
        ExtensionStepState::On => ExtensionStepPlan::Nothing,
        ExtensionStepState::ListedNotLoaded => ExtensionStepPlan::ReloginOnly,
        ExtensionStepState::Off | ExtensionStepState::OffNotLoaded => ExtensionStepPlan::Ask,
        ExtensionStepState::NotReady => ExtensionStepPlan::NotReady,
    }
}

/// The GNOME Shell extension step of a guided Linux setup; only GNOME on Wayland needs it.
#[cfg(target_os = "linux")]
async fn gnome_extension_step(next_steps: &mut LinuxNextSteps) -> Result<()> {
    use std::sync::Arc;

    use dbus::gnome_shell::{
        ExtensionInstallationStatus,
        ShellExtensions,
        get_extension_status,
    };
    use fig_integrations::gnome_extension::{
        GnomeExtensionIntegration,
        SwitchOn,
    };
    use fig_util::system_info::linux::{
        DesktopEnvironment,
        DisplayServer,
        get_desktop_environment,
        get_display_server,
    };

    let ctx = fig_os_shim::Context::new();
    let (Ok(desktop), Ok(display_server)) = (get_desktop_environment(&ctx), get_display_server(&ctx)) else {
        return Ok(());
    };
    let wayland = display_server == DisplayServer::Wayland;
    let gnome = desktop == DesktopEnvironment::Gnome;

    let shell_extensions = ShellExtensions::new(Arc::downgrade(&ctx));
    let state = if wayland && gnome {
        match get_extension_status(&ctx, &shell_extensions).await {
            Ok(ExtensionInstallationStatus::Enabled) => ExtensionStepState::On,
            Ok(ExtensionInstallationStatus::NotEnabled) => ExtensionStepState::Off,
            Ok(ExtensionInstallationStatus::RequiresRelogin) => match shell_extensions.is_listed_enabled().await {
                Ok(true) => ExtensionStepState::ListedNotLoaded,
                _ => ExtensionStepState::OffNotLoaded,
            },
            _ => ExtensionStepState::NotReady,
        }
    } else {
        ExtensionStepState::NotReady
    };

    match extension_step_plan(wayland, gnome, state) {
        ExtensionStepPlan::Nothing => {},
        ExtensionStepPlan::SayGnomeOnly => println!("On Wayland, {PRODUCT_NAME} only works with GNOME Shell."),
        ExtensionStepPlan::ReloginOnly => next_steps.relogin_needed = true,
        ExtensionStepPlan::NotReady => next_steps.extension_not_ready = true,
        ExtensionStepPlan::Ask => {
            println!();
            println!("{}", "GNOME Shell extension".bold());
            println!("On Wayland, {PRODUCT_NAME} cannot find your terminal window by itself.");
            println!("Its GNOME Shell extension, installed with the {PRODUCT_NAME} package, tells the");
            println!("{PRODUCT_NAME} app which application has focus and where its window is on screen.");
            println!("It does this for every window, not only terminals, and sends it only to the");
            println!("{PRODUCT_NAME} app on this computer.");
            println!("It stays switched off unless you say yes.");
            println!();

            match choose(format!("Switch on the {PRODUCT_NAME} GNOME Shell extension?"), &["Yes", "No"])? {
                Some(0) => match GnomeExtensionIntegration::new(&ctx, &shell_extensions).switch_on().await {
                    Ok(SwitchOn::OnAfterRelogin | SwitchOn::AlreadyOnAfterRelogin) => next_steps.relogin_needed = true,
                    Ok(SwitchOn::On | SwitchOn::AlreadyOn) => {},
                    Err(err) => {
                        println!("Could not switch the extension on:");
                        println!("{err}");
                        next_steps.extension_not_ready = true;
                    },
                },
                Some(_) => next_steps.extension_declined = true,
                None => bail!("No option selected"),
            }
        },
    }
    Ok(())
}

/// The start at login step of a guided Linux setup.
#[cfg(target_os = "linux")]
async fn start_at_login_step(next_steps: &mut LinuxNextSteps) -> Result<()> {
    use fig_integrations::Integration as _;
    use fig_integrations::desktop_entry::{
        AutostartIntegration,
        local_autostart_path,
    };
    use fig_util::consts::{
        LAUNCH_ON_STARTUP_DEFAULT,
        LAUNCH_ON_STARTUP_KEY,
    };
    use fig_util::directories::home_dir_ctx;

    let ctx = fig_os_shim::Context::new();
    if fig_settings::settings::get_bool_or(LAUNCH_ON_STARTUP_KEY, LAUNCH_ON_STARTUP_DEFAULT) {
        // The app restores a missing link at its next launch.
        next_steps.starts_at_login = AutostartIntegration::new(&ctx)?.is_installed().await.is_ok();
        return Ok(());
    }

    let link = local_autostart_path(&ctx)?;
    let link = home_dir_ctx(&ctx)
        .ok()
        .and_then(|home| link.strip_prefix(home).ok().map(|rel| format!("~/{}", rel.display())))
        .unwrap_or_else(|| link.display().to_string());

    println!();
    println!("{}", "Start at login".bold());
    println!("{PRODUCT_NAME} shows completions only while its app is running.");
    println!("Saying yes links {link} to the app's launcher,");
    println!("so the app starts when you log in.");
    println!();

    match choose(format!("Start {PRODUCT_NAME} when you log in?"), &["Yes", "No"])? {
        Some(0) => match crate::util::desktop::set_launch_at_login(&ctx, &fig_settings::Settings::new(), true).await {
            Ok(()) => next_steps.starts_at_login = true,
            Err(err) => println!("Could not set this up: {err}"),
        },
        Some(_) => {},
        None => bail!("No option selected"),
    }
    Ok(())
}

/// The script files run nothing until these links exist.
#[cfg(any(target_os = "macos", test))]
fn manual_link_instructions(commands_dir: Option<&std::path::Path>) -> String {
    let dir = match commands_dir {
        Some(dir) => dir.display().to_string(),
        None => format!("<path to {}>/Contents/MacOS", fig_util::consts::APP_BUNDLE_NAME),
    };
    let links: String = [CLI_BINARY_NAME, fig_util::PTY_BINARY_NAME]
        .into_iter()
        .map(|name| format!("\nln -s \"{dir}/{name}\" \"$HOME/.local/bin/{name}\""))
        .collect();
    format!("To link the commands into ~/.local/bin, run:\nmkdir -p \"$HOME/.local/bin\"{links}")
}

#[cfg(any(target_os = "macos", test))]
fn closing_text(accessibility: AccessibilityStep) -> String {
    let new_terminal = "Open a new terminal window so the shell integration loads.".to_owned();
    let doctor = format!("Run `{CLI_BINARY_NAME} doctor` if completions do not appear.");
    let grant = format!(
        "Grant {PRODUCT_NAME} the Accessibility permission in System Settings > Privacy & Security > Accessibility."
    );
    let (status, steps): (Option<String>, Vec<String>) = match accessibility {
        AccessibilityStep::NotAttempted => (None, vec![new_terminal, grant, doctor]),
        #[cfg(any(target_os = "macos", test))]
        AccessibilityStep::Finished(AccessibilityOutcome::AlreadyGranted | AccessibilityOutcome::Granted) => (
            Some(format!("{PRODUCT_NAME} has the Accessibility permission.")),
            vec![new_terminal, doctor],
        ),
        #[cfg(any(target_os = "macos", test))]
        AccessibilityStep::Failed
        | AccessibilityStep::Finished(AccessibilityOutcome::TimedOut | AccessibilityOutcome::Interrupted) => (
            Some(format!("{PRODUCT_NAME} has not been granted the Accessibility permission yet.")),
            vec![grant, format!("Run `{CLI_BINARY_NAME} setup` again to confirm."), new_terminal],
        ),
        #[cfg(any(target_os = "macos", test))]
        AccessibilityStep::AppNotStarted => (
            Some(format!("The {PRODUCT_NAME} app could not be started.")),
            vec![
                format!("Run `{CLI_BINARY_NAME} launch` to start it."),
                format!("Run `{CLI_BINARY_NAME} setup` again."),
                new_terminal,
            ],
        ),
    };
    next_steps_text(status, steps)
}

/// What the Linux closing text of `setup` depends on.
#[cfg(any(target_os = "linux", test))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct LinuxNextSteps {
    relogin_needed: bool,
    starts_at_login: bool,
    /// The person answered No to switching the extension on.
    extension_declined: bool,
    /// The extension cannot be switched on as things stand; doctor says why.
    extension_not_ready: bool,
}

#[cfg(any(target_os = "linux", test))]
fn linux_closing_text(next_steps: LinuxNextSteps) -> String {
    let LinuxNextSteps {
        relogin_needed,
        starts_at_login,
        extension_declined,
        extension_not_ready,
    } = next_steps;
    let mut steps = Vec::new();
    if relogin_needed {
        steps.push("Log out and log in again so GNOME Shell loads the extension.".to_owned());
    }
    steps.push(match (relogin_needed, starts_at_login) {
        (true, true) => format!("The {PRODUCT_NAME} app starts by itself when you log in."),
        (false, true) => format!(
            "Run `{CLI_BINARY_NAME} launch` to start the {PRODUCT_NAME} app now; after this it starts\n     \
             when you log in."
        ),
        (true, false) => format!("After logging in, run `{CLI_BINARY_NAME} launch` to start the {PRODUCT_NAME} app."),
        (false, false) => format!("Run `{CLI_BINARY_NAME} launch` to start the {PRODUCT_NAME} app."),
    });
    steps.push("Open a new terminal window so the shell integration loads.".to_owned());
    steps.push(format!("Run `{CLI_BINARY_NAME} doctor` if completions do not appear."));
    let status = if extension_not_ready {
        Some(format!(
            "Completions will not appear on Wayland until the GNOME Shell extension works.\n\
             Run `{CLI_BINARY_NAME} doctor` for the reason."
        ))
    } else if extension_declined {
        Some(format!(
            "Completions will not appear on Wayland until the GNOME Shell extension is on.\n\
             To switch it on: `{CLI_BINARY_NAME} {SWITCH_ON_COMMAND}`."
        ))
    } else {
        None
    };
    next_steps_text(status, steps)
}

fn next_steps_text(status: Option<String>, steps: Vec<String>) -> String {
    let steps: String = steps
        .into_iter()
        .enumerate()
        .map(|(i, step)| format!("\n  {}. {step}", i + 1))
        .collect();
    match status {
        Some(status) => format!("\n{status}\n\nNext steps:{steps}"),
        None => format!("\nNext steps:{steps}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closing_text_not_attempted() {
        assert_eq!(
            closing_text(AccessibilityStep::NotAttempted),
            "\nNext steps:\n  \
             1. Open a new terminal window so the shell integration loads.\n  \
             2. Grant ShellCue the Accessibility permission in System Settings > Privacy & Security > Accessibility.\n  \
             3. Run `shellcue doctor` if completions do not appear."
        );
    }

    #[test]
    fn closing_text_granted() {
        for outcome in [AccessibilityOutcome::AlreadyGranted, AccessibilityOutcome::Granted] {
            assert_eq!(
                closing_text(AccessibilityStep::Finished(outcome)),
                "\nShellCue has the Accessibility permission.\n\nNext steps:\n  \
                 1. Open a new terminal window so the shell integration loads.\n  \
                 2. Run `shellcue doctor` if completions do not appear."
            );
        }
    }

    #[test]
    fn closing_text_not_granted() {
        for step in [
            AccessibilityStep::Finished(AccessibilityOutcome::TimedOut),
            AccessibilityStep::Finished(AccessibilityOutcome::Interrupted),
            AccessibilityStep::Failed,
        ] {
            assert_eq!(
                closing_text(step),
                "\nShellCue has not been granted the Accessibility permission yet.\n\nNext steps:\n  \
                 1. Grant ShellCue the Accessibility permission in System Settings > Privacy & Security > Accessibility.\n  \
                 2. Run `shellcue setup` again to confirm.\n  \
                 3. Open a new terminal window so the shell integration loads."
            );
        }
    }

    #[test]
    fn closing_text_app_not_started() {
        assert_eq!(
            closing_text(AccessibilityStep::AppNotStarted),
            "\nThe ShellCue app could not be started.\n\nNext steps:\n  \
             1. Run `shellcue launch` to start it.\n  \
             2. Run `shellcue setup` again.\n  \
             3. Open a new terminal window so the shell integration loads."
        );
    }

    #[test]
    fn input_method_offer() {
        assert!(should_offer_input_method(false, false));
        assert!(should_offer_input_method(false, true));
        assert!(!should_offer_input_method(true, false));
        assert!(should_offer_input_method(true, true));
    }

    #[test]
    fn linux_closing_text_plain() {
        assert_eq!(
            linux_closing_text(LinuxNextSteps::default()),
            "\nNext steps:\n  \
             1. Run `shellcue launch` to start the ShellCue app.\n  \
             2. Open a new terminal window so the shell integration loads.\n  \
             3. Run `shellcue doctor` if completions do not appear."
        );
    }

    #[test]
    fn linux_closing_text_relogin_and_starts_at_login() {
        assert_eq!(
            linux_closing_text(LinuxNextSteps {
                relogin_needed: true,
                starts_at_login: true,
                ..LinuxNextSteps::default()
            }),
            "\nNext steps:\n  \
             1. Log out and log in again so GNOME Shell loads the extension.\n  \
             2. The ShellCue app starts by itself when you log in.\n  \
             3. Open a new terminal window so the shell integration loads.\n  \
             4. Run `shellcue doctor` if completions do not appear."
        );
    }

    #[test]
    fn linux_closing_text_relogin_only() {
        assert_eq!(
            linux_closing_text(LinuxNextSteps {
                relogin_needed: true,
                ..LinuxNextSteps::default()
            }),
            "\nNext steps:\n  \
             1. Log out and log in again so GNOME Shell loads the extension.\n  \
             2. After logging in, run `shellcue launch` to start the ShellCue app.\n  \
             3. Open a new terminal window so the shell integration loads.\n  \
             4. Run `shellcue doctor` if completions do not appear."
        );
    }

    #[test]
    fn linux_closing_text_starts_at_login_only() {
        assert_eq!(
            linux_closing_text(LinuxNextSteps {
                starts_at_login: true,
                ..LinuxNextSteps::default()
            }),
            "\nNext steps:\n  \
             1. Run `shellcue launch` to start the ShellCue app now; after this it starts\n     \
             when you log in.\n  \
             2. Open a new terminal window so the shell integration loads.\n  \
             3. Run `shellcue doctor` if completions do not appear."
        );
    }

    #[test]
    fn linux_closing_text_extension_declined() {
        assert_eq!(
            linux_closing_text(LinuxNextSteps {
                extension_declined: true,
                ..LinuxNextSteps::default()
            }),
            "\nCompletions will not appear on Wayland until the GNOME Shell extension is on.\n\
             To switch it on: `shellcue integrations install gnome-shell-extension`.\n\n\
             Next steps:\n  \
             1. Run `shellcue launch` to start the ShellCue app.\n  \
             2. Open a new terminal window so the shell integration loads.\n  \
             3. Run `shellcue doctor` if completions do not appear."
        );
    }

    #[test]
    fn linux_closing_text_starts_at_login_and_extension_declined() {
        assert_eq!(
            linux_closing_text(LinuxNextSteps {
                starts_at_login: true,
                extension_declined: true,
                ..LinuxNextSteps::default()
            }),
            "\nCompletions will not appear on Wayland until the GNOME Shell extension is on.\n\
             To switch it on: `shellcue integrations install gnome-shell-extension`.\n\n\
             Next steps:\n  \
             1. Run `shellcue launch` to start the ShellCue app now; after this it starts\n     \
             when you log in.\n  \
             2. Open a new terminal window so the shell integration loads.\n  \
             3. Run `shellcue doctor` if completions do not appear."
        );
    }

    #[test]
    fn linux_closing_text_extension_not_ready() {
        assert_eq!(
            linux_closing_text(LinuxNextSteps {
                extension_not_ready: true,
                ..LinuxNextSteps::default()
            }),
            "\nCompletions will not appear on Wayland until the GNOME Shell extension works.\n\
             Run `shellcue doctor` for the reason.\n\n\
             Next steps:\n  \
             1. Run `shellcue launch` to start the ShellCue app.\n  \
             2. Open a new terminal window so the shell integration loads.\n  \
             3. Run `shellcue doctor` if completions do not appear."
        );
    }

    #[test]
    fn linux_closing_text_lines_fit() {
        for relogin_needed in [false, true] {
            for starts_at_login in [false, true] {
                for extension_declined in [false, true] {
                    for extension_not_ready in [false, true] {
                        let text = linux_closing_text(LinuxNextSteps {
                            relogin_needed,
                            starts_at_login,
                            extension_declined,
                            extension_not_ready,
                        });
                        for line in text.lines() {
                            assert!(line.chars().count() < 80, "{line}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn extension_step_plan_table() {
        use super::{
            ExtensionStepPlan as Plan,
            ExtensionStepState as State,
        };

        let table = [
            (false, false, State::On, Plan::Nothing),
            (false, false, State::Off, Plan::Nothing),
            (false, false, State::OffNotLoaded, Plan::Nothing),
            (false, false, State::ListedNotLoaded, Plan::Nothing),
            (false, false, State::NotReady, Plan::Nothing),
            (false, true, State::On, Plan::Nothing),
            (false, true, State::Off, Plan::Nothing),
            (false, true, State::OffNotLoaded, Plan::Nothing),
            (false, true, State::ListedNotLoaded, Plan::Nothing),
            (false, true, State::NotReady, Plan::Nothing),
            (true, false, State::On, Plan::SayGnomeOnly),
            (true, false, State::Off, Plan::SayGnomeOnly),
            (true, false, State::OffNotLoaded, Plan::SayGnomeOnly),
            (true, false, State::ListedNotLoaded, Plan::SayGnomeOnly),
            (true, false, State::NotReady, Plan::SayGnomeOnly),
            (true, true, State::On, Plan::Nothing),
            (true, true, State::Off, Plan::Ask),
            (true, true, State::OffNotLoaded, Plan::Ask),
            (true, true, State::ListedNotLoaded, Plan::ReloginOnly),
            (true, true, State::NotReady, Plan::NotReady),
        ];
        for (wayland, gnome, state, expected) in table {
            assert_eq!(
                extension_step_plan(wayland, gnome, state),
                expected,
                "wayland {wayland}, gnome {gnome}, {state:?}"
            );
        }
    }

    #[test]
    fn guided_setup() {
        assert!(is_guided_setup(InstallComponents::all(), false, false));
        assert!(!is_guided_setup(InstallComponents::all(), true, false));
        assert!(!is_guided_setup(InstallComponents::all(), false, true));
        assert!(!is_guided_setup(InstallComponents::SHELL_INTEGRATIONS, false, false));
        assert!(!is_guided_setup(InstallComponents::INPUT_METHOD, false, false));
    }

    #[test]
    fn manual_link_instructions_name_both_links() {
        let dir = std::path::Path::new("/Applications/ShellCue.app/Contents/MacOS");
        let text = manual_link_instructions(Some(dir));
        assert_eq!(
            text,
            "To link the commands into ~/.local/bin, run:\n\
             mkdir -p \"$HOME/.local/bin\"\n\
             ln -s \"/Applications/ShellCue.app/Contents/MacOS/shellcue\" \"$HOME/.local/bin/shellcue\"\n\
             ln -s \"/Applications/ShellCue.app/Contents/MacOS/shellcue-term\" \"$HOME/.local/bin/shellcue-term\""
        );
        assert!(!text.contains("zshrc") && !text.contains("zprofile"));

        let fallback = manual_link_instructions(None);
        for name in ["shellcue", "shellcue-term"] {
            let line = format!("ln -s \"<path to ShellCue.app>/Contents/MacOS/{name}\" \"$HOME/.local/bin/{name}\"");
            assert!(fallback.contains(&line), "{fallback}");
        }
    }
}
