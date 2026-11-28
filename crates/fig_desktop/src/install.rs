use std::sync::Arc;

use fig_integrations::Integration;
use fig_os_shim::Context;
use semver::Version;
use tracing::error;
#[cfg(target_os = "linux")]
use tracing::info;

#[allow(unused_imports)]
use crate::utils::is_cargo_debug_build;

const PREVIOUS_VERSION_KEY: &str = "desktop.versionAtPreviousLaunch";

#[cfg(target_os = "macos")]
fn run_input_method_migration() {
    use fig_integrations::input_method::InputMethod;
    use tokio::time::{
        Duration,
        sleep,
    };
    use tracing::warn;

    let input_method = InputMethod::default();
    match input_method.target_bundle_path() {
        Ok(target_bundle_path) if target_bundle_path.exists() => {
            tokio::spawn(async move {
                input_method.terminate().ok();
                if let Err(err) = input_method.migrate().await {
                    warn!(%err, "Failed to migrate input method");
                }

                sleep(Duration::from_secs(1)).await;
                input_method.launch();
            });
        },
        Ok(_) => warn!("Input method bundle path does not exist"),
        Err(err) => warn!(%err, "Failed to get input method bundle path"),
    }
}

/// Run items at launch
#[allow(unused_variables)]
pub async fn run_install(ctx: Arc<Context>) {
    #[cfg(target_os = "macos")]
    {
        refresh_shell_integration().await;
        crate::launch_at_login::reconcile();
    }

    #[cfg(target_os = "macos")]
    // Add any items that are only once per version
    if should_run_install_script() {
        run_input_method_migration();
    }

    #[cfg(target_os = "linux")]
    run_linux_install(Arc::clone(&ctx), Arc::new(fig_settings::Settings::new()))
        .await;

    if let Err(err) = set_previous_version(current_version()) {
        error!(%err, "Failed to set previous version");
    }
}

/// When any dotfile sources a block, even with outdated or missing scripts, refreshes the links and
/// the scripts so the integration follows the app; never adds dotfile blocks.
#[cfg(target_os = "macos")]
async fn refresh_shell_integration() {
    use fig_integrations::shell::ShellExt;
    use fig_util::Shell;

    let env = fig_os_shim::Env::new();
    let mut installed = false;
    for shell in Shell::all() {
        match shell.has_dotfile_block(&env) {
            Ok(has_block) => installed |= has_block,
            Err(err) => error!(%err, "Failed to read the dotfiles for {shell}"),
        }
    }
    if !installed {
        return;
    }

    if let Err(err) = fig_install::macos::link_commands().await {
        error!(%err, "Failed to link the commands");
    }
    if let Err(err) = fig_install::install_script_integrations().await {
        error!(%err, "Failed installing the shell integration scripts");
    }
}

#[cfg(target_os = "linux")]
async fn run_linux_install(ctx: Arc<Context>, settings: Arc<fig_settings::Settings>) {
    use fig_util::system_info::linux::get_display_server;

    // Important we log an error if we cannot detect the display server in use.
    // If this isn't wayland or x11, the user will probably just see a blank screen.
    match get_display_server(&ctx) {
        Ok(_) => (),
        Err(fig_util::Error::UnknownDisplayServer(server)) => {
            error!(
                "Unknown value set for XDG_SESSION_TYPE: {}. This must be set to x11 or wayland.",
                server
            );
        },
        Err(err) => {
            error!(
                "Unknown error occurred when detecting the display server: {:?}. Is XDG_SESSION_TYPE set to x11 or wayland?",
                err
            );
        },
    }

    // Autostart entry
    {
        let ctx_clone = Arc::clone(&ctx);
        let settings_clone = Arc::clone(&settings);
        tokio::spawn(async move {
            install_autostart_entry(&ctx_clone, &settings_clone)
                .await
                .map_err(|err| error!(?err, "Unable to install autostart entry"))
                .ok();
        });
    }

    // TODO: is this correct?
    // launch_ibus().await;
}

/// Installs the autostart entry if required.
#[cfg(target_os = "linux")]
async fn install_autostart_entry(ctx: &Context, settings: &fig_settings::Settings) -> anyhow::Result<()> {
    use fig_integrations::desktop_entry::{
        AutostartIntegration,
        should_install_autostart_entry,
    };

    if !should_install_autostart_entry(settings) {
        return Ok(());
    }

    AutostartIntegration::new(ctx)?.install().await?;

    Ok(())
}

#[cfg(target_os = "linux")]
#[derive(Debug)]
enum SystemdUserService {
    IBusGeneric,
    IBusGnome,
}

#[cfg(target_os = "linux")]
impl SystemdUserService {
    fn service_name(&self) -> &'static str {
        match self {
            SystemdUserService::IBusGeneric => "org.freedesktop.IBus.session.generic.service",
            SystemdUserService::IBusGnome => "org.freedesktop.IBus.session.GNOME.service",
        }
    }
}

#[cfg(target_os = "linux")]
impl std::fmt::Display for SystemdUserService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.service_name())
    }
}

#[cfg(target_os = "linux")]
async fn launch_systemd_user_service(service: SystemdUserService) -> anyhow::Result<()> {
    use tokio::process::Command;
    let output = Command::new("systemctl")
        .args(["--user", "restart", service.service_name()])
        .output()
        .await?;
    if !output.status.success() {
        anyhow::bail!("{}", String::from_utf8_lossy(&output.stderr))
    }
    Ok(())
}

#[cfg(target_os = "linux")]
#[allow(dead_code)]
async fn launch_ibus(ctx: &Context) {
    use std::ffi::OsString;

    use sysinfo::{
        ProcessRefreshKind,
        RefreshKind,
        System,
    };
    use tokio::process::Command;

    let system = tokio::task::block_in_place(|| {
        System::new_with_specifics(RefreshKind::nothing().with_processes(ProcessRefreshKind::nothing()))
    });
    let ibus_daemon = OsString::from("ibus-daemon");
    if system.processes_by_name(&ibus_daemon).next().is_none() {
        info!("Launching ibus via systemd");

        match Command::new("systemctl")
            .args(["--user", "is-active", "gnome-session-initialized.target"])
            .output()
            .await
        {
            Ok(gnome_session_output) => match std::str::from_utf8(&gnome_session_output.stdout).map(|s| s.trim()) {
                Ok("active") => match launch_systemd_user_service(SystemdUserService::IBusGnome).await {
                    Ok(_) => info!("Launched '{}", SystemdUserService::IBusGnome),
                    Err(err) => error!(%err, "Failed to launch '{}'", SystemdUserService::IBusGnome),
                },
                Ok("inactive") => match launch_systemd_user_service(SystemdUserService::IBusGeneric).await {
                    Ok(_) => info!("Launched '{}'", SystemdUserService::IBusGeneric),
                    Err(err) => error!(%err, "Failed to launch '{}'", SystemdUserService::IBusGeneric),
                },
                result => error!(
                    ?result,
                    "Failed to determine if gnome-session-initialized.target is running"
                ),
            },
            Err(err) => error!(%err, "Failed to run 'systemctl --user is-active gnome-session-initialized.target'"),
        }
    }

    // Wait up to 2 sec for ibus activation
    for _ in 0..10 {
        if dbus::ibus::connect_to_ibus_daemon(ctx).await.is_ok() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    error!("Timed out after 2 sec waiting for ibus activation");
}

#[cfg(target_os = "macos")]
fn should_run_install_script() -> bool {
    let current_version = current_version();
    let previous_version = match previous_version() {
        Some(previous_version) => previous_version,
        None => return true,
    };

    !is_cargo_debug_build() && current_version > previous_version
}

/// The current version of the desktop app
fn current_version() -> Version {
    Version::parse(env!("CARGO_PKG_VERSION")).unwrap()
}

/// The previous version of the desktop app stored in local state
#[cfg(target_os = "macos")]
fn previous_version() -> Option<Version> {
    fig_settings::state::get_string(PREVIOUS_VERSION_KEY)
        .ok()
        .flatten()
        .and_then(|ref v| Version::parse(v).ok())
}

fn set_previous_version(version: Version) -> anyhow::Result<()> {
    fig_settings::state::set_value(PREVIOUS_VERSION_KEY, version.to_string())?;
    Ok(())
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_current_version() {
        current_version();
    }
}

#[cfg(all(test, target_os = "linux"))]
mod linux_autostart_tests {
    use fig_integrations::desktop_entry::{
        AutostartIntegration,
        global_entry_path,
        local_autostart_path,
    };
    use fig_settings::Settings;
    use fig_util::consts::LAUNCH_ON_STARTUP_KEY;

    use super::*;

    async fn test_context_with_global_entry() -> Arc<Context> {
        let ctx = Context::builder().with_test_home().await.unwrap().build_fake();
        let global_path = global_entry_path(&ctx);
        ctx.fs().create_dir_all(global_path.parent().unwrap()).await.unwrap();
        ctx.fs().write(global_path, "[Desktop Entry]").await.unwrap();
        ctx
    }

    #[tokio::test]
    async fn autostart_entry_is_installed_when_the_setting_is_on() {
        let ctx = test_context_with_global_entry().await;
        let settings = Settings::from_slice(&[(LAUNCH_ON_STARTUP_KEY, true.into())]);

        install_autostart_entry(&ctx, &settings).await.unwrap();

        assert!(AutostartIntegration::new(&ctx).unwrap().is_installed().await.is_ok());
        assert!(ctx.fs().exists(local_autostart_path(&ctx).unwrap()));
    }

    #[tokio::test]
    async fn autostart_entry_is_not_installed_when_the_setting_is_off() {
        let ctx = test_context_with_global_entry().await;

        install_autostart_entry(&ctx, &Settings::new_fake()).await.unwrap();

        assert!(AutostartIntegration::new(&ctx).unwrap().is_installed().await.is_err());
        assert!(!ctx.fs().exists(local_autostart_path(&ctx).unwrap()));
    }
}
