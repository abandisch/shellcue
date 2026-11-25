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
use fig_os_shim::Env;
use fig_util::{
    CLI_BINARY_NAME,
    PRODUCT_NAME,
};
use tracing::warn;

#[cfg(any(target_os = "macos", test))]
use crate::util::accessibility::AccessibilityOutcome;
use crate::util::choose;

/// How the guided Accessibility step of `setup` ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AccessibilityStep {
    /// Linux, `--no-confirm`, `--global`, or an install narrowed by a flag.
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
            let prompt = format!(
                "Do you want {CLI_BINARY_NAME} to modify your shell config (you will have to manually do this otherwise)?",
            );
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
                    println!("To enable the integration, select \"yes\" below and then click Ok in the popup.");
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

    // Must stay last: its wait leaves a Ctrl-C handler installed for the rest of the process.
    #[cfg(target_os = "macos")]
    let accessibility = if should_guide_accessibility(install_components, no_confirm, global) {
        accessibility_step().await
    } else {
        AccessibilityStep::NotAttempted
    };
    #[cfg(not(target_os = "macos"))]
    let accessibility = AccessibilityStep::NotAttempted;

    println!("{}", closing_text(accessibility));

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

/// Only an interactive, per-user, full setup is guided through the Accessibility permission.
#[cfg(any(target_os = "macos", test))]
fn should_guide_accessibility(install_components: InstallComponents, no_confirm: bool, global: bool) -> bool {
    install_components == InstallComponents::all() && !no_confirm && !global
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

fn closing_text(accessibility: AccessibilityStep) -> String {
    let new_terminal = "Open a new terminal window so the shell integration loads.".to_owned();
    let doctor = format!("Run `{CLI_BINARY_NAME} doctor` if completions do not appear.");
    let grant = format!(
        "Grant {PRODUCT_NAME} the Accessibility permission in System Settings > Privacy & Security > Accessibility."
    );
    let (status, steps): (Option<String>, Vec<String>) = match accessibility {
        AccessibilityStep::NotAttempted => {
            let steps = if cfg!(target_os = "macos") {
                vec![new_terminal, grant, doctor]
            } else {
                vec![new_terminal, doctor]
            };
            (None, steps)
        },
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
        #[cfg(not(target_os = "macos"))]
        assert_eq!(
            closing_text(AccessibilityStep::NotAttempted),
            "\nNext steps:\n  \
             1. Open a new terminal window so the shell integration loads.\n  \
             2. Run `shellcue doctor` if completions do not appear."
        );
        #[cfg(target_os = "macos")]
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
    fn accessibility_guide() {
        assert!(should_guide_accessibility(InstallComponents::all(), false, false));
        assert!(!should_guide_accessibility(InstallComponents::all(), true, false));
        assert!(!should_guide_accessibility(InstallComponents::all(), false, true));
        assert!(!should_guide_accessibility(InstallComponents::SHELL_INTEGRATIONS, false, false));
        assert!(!should_guide_accessibility(InstallComponents::INPUT_METHOD, false, false));
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
