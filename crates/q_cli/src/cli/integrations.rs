use std::process::ExitCode;

use anstream::println;
use clap::Subcommand;
use crossterm::style::Stylize;
use eyre::Result;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use fig_integrations::Integration as _;
use fig_integrations::shell::ShellExt;
use fig_os_shim::Env;
use fig_util::Shell;
use serde_json::json;
use tracing::debug;

use super::OutputFormat;

#[derive(Debug, PartialEq, Eq, Subcommand)]
pub enum IntegrationsSubcommands {
    Install {
        /// Integration to install
        #[command(subcommand)]
        integration: Integration,
        /// Suppress status messages
        #[arg(long, short)]
        silent: bool,
    },
    Uninstall {
        /// Integration to uninstall
        #[command(subcommand)]
        integration: Integration,
        /// Suppress status messages
        #[arg(long, short)]
        silent: bool,
    },
    Reinstall {
        /// Integration to reinstall
        #[command(subcommand)]
        integration: Integration,
        /// Suppress status messages
        #[arg(long, short)]
        silent: bool,
    },
    Status {
        /// Integration to check status of
        #[command(subcommand)]
        integration: Integration,
        #[arg(long, short, value_enum, default_value_t)]
        format: OutputFormat,
    },
}

#[derive(Debug, Subcommand, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Integration {
    Dotfiles {
        /// Only install the integrations for a single shell
        #[arg(value_enum)]
        shell: Option<Shell>,
    },
    InputMethod,
    AutostartEntry,
    GnomeShellExtension,
    #[doc(hidden)]
    All,
}

impl IntegrationsSubcommands {
    pub async fn execute(self) -> Result<ExitCode> {
        match self {
            IntegrationsSubcommands::Install { integration, silent } => {
                if let Integration::All = integration {
                    install(Integration::Dotfiles { shell: None }, silent).await?;
                    #[cfg(target_os = "macos")]
                    install(Integration::InputMethod, silent).await?;
                } else {
                    install(integration, silent).await?;
                }
                Ok(ExitCode::SUCCESS)
            },
            IntegrationsSubcommands::Uninstall { integration, silent } => {
                if let Integration::All = integration {
                    uninstall(Integration::Dotfiles { shell: None }, silent).await?;
                    #[cfg(target_os = "macos")]
                    uninstall(Integration::InputMethod, silent).await?;
                    #[cfg(target_os = "linux")]
                    uninstall(Integration::AutostartEntry, silent).await?;
                    #[cfg(target_os = "linux")]
                    if let Err(err) = uninstall(Integration::GnomeShellExtension, silent).await {
                        if !silent {
                            println!(
                                "Could not switch off the {} GNOME Shell extension: {err}",
                                fig_util::PRODUCT_NAME
                            );
                        }
                    }
                } else {
                    uninstall(integration, silent).await?;
                }
                Ok(ExitCode::SUCCESS)
            },
            IntegrationsSubcommands::Status { integration, format } => status(integration, format).await,
            IntegrationsSubcommands::Reinstall { integration, silent } => {
                if let Integration::All = integration {
                    uninstall(Integration::Dotfiles { shell: None }, silent).await?;
                    #[cfg(target_os = "macos")]
                    uninstall(Integration::InputMethod, silent).await?;
                    install(Integration::Dotfiles { shell: None }, silent).await?;
                    #[cfg(target_os = "macos")]
                    install(Integration::InputMethod, silent).await?;
                } else {
                    uninstall(integration, silent).await?;
                    install(integration, silent).await?;
                }
                Ok(ExitCode::SUCCESS)
            },
        }
    }
}

#[allow(unused_mut)]
async fn install(integration: Integration, silent: bool) -> Result<()> {
    let mut installed = false;
    let mut errored = false;
    let mut status: Option<&str> = None;
    let mut report: Option<String> = None;

    let result = match integration {
        Integration::All => Ok(()),
        Integration::Dotfiles { shell } => {
            let shells = match shell {
                Some(shell) => vec![shell],
                None => vec![Shell::Zsh],
            };

            let mut errs: Vec<String> = vec![];
            for shell in &shells {
                match shell.get_shell_integrations(&Env::new()) {
                    Ok(integrations) => {
                        for integration in integrations {
                            match integration.is_installed().await {
                                Ok(_) => {
                                    debug!("Skipping {}", integration.describe());
                                },
                                Err(_) => {
                                    installed = true;
                                    if let Err(e) = integration.install().await {
                                        errs.push(format!(
                                            "{}: {}",
                                            integration.describe().bold(),
                                            e.verbose_message()
                                        ));
                                    }
                                },
                            }
                        }
                    },
                    Err(e) => {
                        errs.push(format!("{shell}: {e}"));
                    },
                }
            }

            if errs.is_empty() {
                Ok(())
            } else {
                Err(eyre::eyre!("\n\n{}", errs.join("\n\n")))
            }
        },
        Integration::InputMethod => {
            cfg_if::cfg_if! {
                if #[cfg(target_os = "macos")] {
                    fig_settings::state::set_value("input-method.enabled", true).ok();
                    fig_integrations::input_method::InputMethod::default().install().await?;
                    installed = true;
                    status = Some("You must restart your terminal to finish installing the input method.");
                    Ok(())
                } else {
                    errored = true;
                    Err(eyre::eyre!("Input method integration is only supported on macOS"))
                }
            }
        },
        Integration::AutostartEntry => {
            cfg_if::cfg_if! {
                if #[cfg(target_os = "linux")] {
                    use fig_os_shim::Context;
                    use fig_settings::Settings;
                    crate::util::desktop::set_launch_at_login(&Context::new(), &Settings::new(), true).await?;
                    installed = true;
                    Ok(())
                } else {
                    errored = true;
                    Err(eyre::eyre!("The autostart integration is only supported on Linux"))
                }
            }
        },
        Integration::GnomeShellExtension => {
            cfg_if::cfg_if! {
                if #[cfg(target_os = "linux")] {
                    use std::sync::Arc;
                    use dbus::gnome_shell::ShellExtensions;
                    use fig_integrations::gnome_extension::{
                        GnomeExtensionIntegration,
                        switch_on_text,
                    };
                    use fig_os_shim::Context;
                    let ctx = Context::new();
                    let shell_extensions = ShellExtensions::new(Arc::downgrade(&ctx));
                    let outcome = GnomeExtensionIntegration::new(&ctx, &shell_extensions).switch_on().await?;
                    report = Some(switch_on_text(outcome));
                    Ok(())
                } else {
                    errored = true;
                    Err(eyre::eyre!("The GNOME Shell extension is only supported on Linux"))
                }
            }
        },
    };

    if let Some(report) = report {
        if !silent {
            println!("{report}");
        }
        return result;
    }

    if installed && result.is_ok() && !silent {
        println!("Installed!");

        if let Some(status) = status {
            println!("{status}");
        }
    }

    if !errored && !installed && !silent {
        println!("Already installed");
    }

    result
}

#[allow(unused_mut)]
async fn uninstall(integration: Integration, silent: bool) -> Result<()> {
    let mut uninstalled = false;
    let mut report: Option<String> = None;

    let result = match integration {
        Integration::All => Ok(()),
        Integration::Dotfiles { shell } => {
            let shells = match shell {
                Some(shell) => vec![shell],
                None => vec![Shell::Zsh],
            };

            let mut errs: Vec<String> = vec![];
            for shell in &shells {
                match shell.get_shell_integrations(&Env::new()) {
                    Ok(integrations) => {
                        for integration in integrations {
                            match integration.is_installed().await {
                                Ok(_) => {
                                    uninstalled = true;
                                    if let Err(e) = integration.uninstall().await {
                                        errs.push(format!(
                                            "{}: {}",
                                            integration.describe().bold(),
                                            e.verbose_message()
                                        ));
                                    }
                                },
                                Err(_) => {
                                    debug!("Skipping {}", integration.describe());
                                },
                            }
                        }
                    },
                    Err(e) => {
                        errs.push(format!("{shell}: {e}"));
                    },
                }
            }

            if errs.is_empty() {
                Ok(())
            } else {
                Err(eyre::eyre!("\n\n{}", errs.join("\n\n")))
            }
        },
        Integration::InputMethod => {
            cfg_if::cfg_if! {
                if #[cfg(target_os = "macos")] {
                    fig_integrations::input_method::InputMethod::default().uninstall().await?;
                    uninstalled = true;
                    Ok(())
                } else {
                    Err(eyre::eyre!("Input method integration is only supported on macOS"))
                }
            }
        },
        Integration::AutostartEntry => {
            cfg_if::cfg_if! {
                if #[cfg(target_os = "linux")] {
                    use fig_os_shim::Context;
                    use fig_settings::Settings;
                    crate::util::desktop::set_launch_at_login(&Context::new(), &Settings::new(), false).await?;
                    uninstalled = true;
                    Ok(())
                } else {
                    Err(eyre::eyre!("The autostart integration is only supported on Linux"))
                }
            }
        },
        Integration::GnomeShellExtension => {
            cfg_if::cfg_if! {
                if #[cfg(target_os = "linux")] {
                    use std::sync::Arc;
                    use dbus::gnome_shell::ShellExtensions;
                    use fig_integrations::gnome_extension::{
                        GnomeExtensionIntegration,
                        switch_off_text,
                    };
                    use fig_os_shim::Context;
                    let ctx = Context::new();
                    let shell_extensions = ShellExtensions::new(Arc::downgrade(&ctx));
                    let changed = GnomeExtensionIntegration::new(&ctx, &shell_extensions).switch_off().await?;
                    report = Some(switch_off_text(changed));
                    Ok(())
                } else {
                    Err(eyre::eyre!("The GNOME Shell extension is only supported on Linux"))
                }
            }
        },
    };

    if let Some(report) = report {
        if !silent {
            println!("{report}");
        }
        return result;
    }

    if uninstalled && result.is_ok() && !silent {
        println!("Uninstalled!");
    }

    if !uninstalled && !silent {
        println!("Not installed");
    }

    result
}

async fn status(integration: Integration, format: OutputFormat) -> Result<ExitCode> {
    match integration {
        Integration::All => Err(eyre::eyre!(
            "Checking the status for all integrations is currently not supported"
        )),
        Integration::Dotfiles { .. } => {
            let mut all_integrations = vec![];
            let mut errors = vec![];

            for shell in Shell::all() {
                match shell.get_shell_integrations(&Env::new()) {
                    Ok(integrations) => {
                        for integration in integrations {
                            all_integrations.push((
                                integration.is_installed().await.is_ok(),
                                integration.describe(),
                                integration.get_shell(),
                                integration.file_name().to_owned(),
                            ));
                        }
                    },
                    Err(e) => {
                        errors.push((shell.to_string(), e.verbose_message()));
                    },
                }
            }

            format.print(
                || {
                    let mut s = String::new();
                    for (installed, describe, _, _) in &all_integrations {
                        s.push_str(&if *installed {
                            "✔ ".green().to_string()
                        } else {
                            "✘ ".red().to_string()
                        });
                        s.push_str(describe);
                        s.push('\n');
                    }

                    for (shell, error) in &errors {
                        s.push_str(&format!("{shell}: {error}\n"));
                    }

                    s
                },
                || {
                    let integrations = all_integrations
                        .iter()
                        .map(|(installed, describe, shell, file_name)| {
                            json!({
                                "installed": installed,
                                "description": describe,
                                "shell": shell,
                                "file_name": file_name,
                            })
                        })
                        .collect::<Vec<_>>();

                    let errors = errors
                        .iter()
                        .map(|(shell, error)| {
                            json!({
                                "shell": shell,
                                "error": error,
                            })
                        })
                        .collect::<Vec<_>>();

                    json!({
                        "integrations": integrations,
                        "errors": errors,
                    })
                },
            );

            Ok(ExitCode::SUCCESS)
        },
        Integration::InputMethod => {
            cfg_if::cfg_if! {
                if #[cfg(target_os = "macos")] {
                    let input_method = fig_integrations::input_method::InputMethod::default();
                    let installed = input_method.is_installed().await.is_ok();
                    format.print(
                        || if installed { "Installed" } else { "Not installed" },
                        || json!({
                            "installed": installed,
                        })
                    );
                    Ok(ExitCode::SUCCESS)
                } else {
                    Err(eyre::eyre!("Input method integration is only supported on macOS"))
                }
            }
        },
        Integration::AutostartEntry => {
            cfg_if::cfg_if! {
                if #[cfg(target_os = "linux")] {
                    use fig_integrations::desktop_entry::AutostartIntegration;
                    use fig_os_shim::Context;
                    let ctx = Context::new();
                    let installed = AutostartIntegration::new(&ctx)?.is_installed().await.is_ok();
                    format.print(
                        || if installed { "Installed" } else { "Not installed" },
                        || json!({
                            "installed": installed,
                        })
                    );
                    Ok(ExitCode::SUCCESS)
                } else {
                    Err(eyre::eyre!("The autostart integration is only supported on Linux"))
                }
            }
        },
        Integration::GnomeShellExtension => {
            cfg_if::cfg_if! {
                if #[cfg(target_os = "linux")] {
                    use std::sync::Arc;
                    use dbus::gnome_shell::{
                        ExtensionInstallationStatus,
                        ShellExtensions,
                        get_extension_status,
                    };
                    use fig_integrations::gnome_extension::{
                        GnomeExtensionIntegration,
                        status_text,
                    };
                    use fig_os_shim::Context;
                    let ctx = Context::new();
                    let shell_extensions = ShellExtensions::new(Arc::downgrade(&ctx));
                    let installed = GnomeExtensionIntegration::new(&ctx, &shell_extensions)
                        .is_installed()
                        .await
                        .is_ok();
                    let status = get_extension_status(&ctx, &shell_extensions).await?;
                    let listed = shell_extensions.is_listed_enabled().await.unwrap_or(false);
                    let version = match status {
                        ExtensionInstallationStatus::OutOfDate => shell_extensions.gnome_shell_version().await.ok(),
                        _ => None,
                    };
                    let text = status_text(status, listed, version.as_ref());
                    format.print(
                        || text,
                        || json!({
                            "installed": installed,
                        })
                    );
                    Ok(ExitCode::SUCCESS)
                } else {
                    Err(eyre::eyre!("The GNOME Shell extension is only supported on Linux"))
                }
            }
        },
    }
}
