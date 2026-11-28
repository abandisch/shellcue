use std::process::Command;

use eyre::{
    Result,
    eyre,
};
use fig_util::{
    PRODUCT_NAME,
    directories,
    system_info,
};

pub struct LaunchArgs {
    /// Should we wait for the socket to continue execution
    pub wait_for_socket: bool,
    /// Print output to user
    pub verbose: bool,
}

#[cfg(target_os = "macos")]
pub fn desktop_app_running() -> bool {
    use std::ffi::OsString;

    use fig_util::consts::{
        APP_BUNDLE_ID,
        APP_PROCESS_NAME,
    };
    use objc2_app_kit::NSRunningApplication;
    use objc2_foundation::ns_string;
    use sysinfo::{
        ProcessRefreshKind,
        RefreshKind,
        System,
    };

    let bundle_id = ns_string!(APP_BUNDLE_ID);
    let running_applications = unsafe { NSRunningApplication::runningApplicationsWithBundleIdentifier(bundle_id) };

    if !running_applications.is_empty() {
        return true;
    }

    // Fallback to process name check
    let app_process_name = OsString::from(APP_PROCESS_NAME);
    let system = System::new_with_specifics(RefreshKind::nothing().with_processes(ProcessRefreshKind::nothing()));
    let mut processes = system.processes_by_exact_name(&app_process_name);
    processes.next().is_some()
}

#[cfg(not(target_os = "macos"))]
pub fn desktop_app_running() -> bool {
    use std::ffi::OsString;

    use fig_util::consts::app_process_listing_name;
    use sysinfo::{
        ProcessRefreshKind,
        RefreshKind,
        System,
    };

    let s = System::new_with_specifics(RefreshKind::nothing().with_processes(ProcessRefreshKind::nothing()));
    let app_process_name = OsString::from(app_process_listing_name());
    let mut processes = s.processes_by_exact_name(&app_process_name);
    processes.next().is_some()
}

pub fn launch_fig_desktop(args: LaunchArgs) -> Result<()> {
    if system_info::is_remote() {
        return Err(eyre!(
            "launching {PRODUCT_NAME} from remote installs is not yet supported"
        ));
    }

    match desktop_app_running() {
        true => return Ok(()),
        false => {
            if args.verbose {
                println!("Launching {PRODUCT_NAME}...");
            }
        },
    }

    std::fs::remove_file(directories::desktop_socket_path()?).ok();

    cfg_if::cfg_if! {
        if #[cfg(target_os = "macos")] {
            let output = Command::new("open")
                .args(["-g", "-b", fig_util::consts::APP_BUNDLE_ID])
                .output()?;

            if !output.status.success() {
                eyre::bail!("failed to launch: {}", String::from_utf8_lossy(&output.stderr));
            }
        } else {
            let state = fig_settings::State::new();
            let ctx = fig_os_shim::Context::new();
            launch_linux_desktop(ctx, &state)?;
            // Need to wait some time for the app to launch and appear in the process list.
            // 1 second to be safe.
            std::thread::sleep(std::time::Duration::from_millis(1000));
        }
    }

    if !args.wait_for_socket {
        return Ok(());
    }

    if !desktop_app_running() {
        return Err(eyre!("{PRODUCT_NAME} was unable launch successfully"));
    }

    // Wait for socket to exist
    let path = directories::desktop_socket_path()?;

    for _ in 0..30 {
        // Wait for socket to exist
        if path.exists() {
            return Ok(());
        }

        std::thread::sleep(std::time::Duration::from_millis(500));
    }

    Err(eyre!("failed to connect to socket".to_owned()))
}

#[cfg(target_os = "linux")]
fn launch_linux_desktop(ctx: std::sync::Arc<fig_os_shim::Context>, state: &fig_settings::State) -> eyre::Result<()> {
    use std::process::Stdio;

    use eyre::Context;
    use fig_integrations::desktop_entry::{
        EntryContents,
        local_entry_path,
    };
    use fig_util::APP_PROCESS_NAME;
    use tracing::error;

    if state.get_bool_or("appimage.manageDesktopEntry", false) {
        if let Some(exec) = EntryContents::from_path_sync(&ctx, local_entry_path(&ctx)?)?.get_field("Exec") {
            match Command::new(exec)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
            {
                Ok(_) => return Ok(()),
                Err(err) => {
                    error!(
                        ?err,
                        "Unable to launch desktop app according to the local desktop entry."
                    );
                },
            }
        }
        // Fall back to calling shellcue-desktop if on the user's path
    }

    Command::new(APP_PROCESS_NAME)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context(format!("Executable '{}' not in the user's path", APP_PROCESS_NAME))?;
    Ok(())
}

/// Sets `app.launchOnStartup` and adds or removes the autostart link to match; the app puts the
/// link back at launch while the setting is on.
#[cfg(target_os = "linux")]
pub async fn set_launch_at_login(
    ctx: &fig_os_shim::Context,
    settings: &fig_settings::Settings,
    on: bool,
) -> Result<()> {
    use fig_integrations::Integration as _;
    use fig_integrations::desktop_entry::AutostartIntegration;
    use fig_util::consts::LAUNCH_ON_STARTUP_KEY;

    settings.set_value(LAUNCH_ON_STARTUP_KEY, on)?;
    if on {
        AutostartIntegration::new(ctx)?.install().await?;
    } else {
        AutostartIntegration::uninstall(ctx).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "not in ci"]
    fn test_e2e_desktop_app_running() {
        println!("{}", desktop_app_running());
    }

    #[test]
    #[ignore = "not in ci"]
    fn test_e2e_launch_fig_desktop() {
        launch_fig_desktop(LaunchArgs {
            wait_for_socket: true,
            verbose: true,
        })
        .unwrap();
    }

    #[test]
    #[ignore = "not in ci"]
    #[cfg(target_os = "linux")]
    fn test_e2e_launch_linux_desktop() {
        use fig_os_shim::Context;
        use fig_settings::State;

        launch_linux_desktop(Context::new(), &State::new()).unwrap();
    }

    #[cfg(target_os = "linux")]
    mod launch_at_login {
        use std::sync::Arc;

        use fig_integrations::Integration as _;
        use fig_integrations::desktop_entry::{
            AutostartIntegration,
            global_entry_path,
        };
        use fig_os_shim::Context;
        use fig_settings::Settings;
        use fig_util::consts::LAUNCH_ON_STARTUP_KEY;

        use super::*;

        async fn make_ctx() -> Arc<Context> {
            let ctx = Context::builder().with_test_home().await.unwrap().build_fake();
            let global = global_entry_path(&*ctx);
            ctx.fs().create_dir_all(global.parent().unwrap()).await.unwrap();
            ctx.fs().write(global, "[Desktop Entry]").await.unwrap();
            ctx
        }

        #[tokio::test]
        async fn test_set_launch_at_login_on_then_off() {
            let ctx = make_ctx().await;
            let settings = Settings::new_fake();

            set_launch_at_login(&ctx, &settings, true).await.unwrap();
            assert_eq!(settings.get_bool(LAUNCH_ON_STARTUP_KEY).unwrap(), Some(true));
            assert!(AutostartIntegration::new(&*ctx).unwrap().is_installed().await.is_ok());

            set_launch_at_login(&ctx, &settings, false).await.unwrap();
            assert_eq!(settings.get_bool(LAUNCH_ON_STARTUP_KEY).unwrap(), Some(false));
            assert!(AutostartIntegration::new(&*ctx).unwrap().is_installed().await.is_err());
        }

        #[tokio::test]
        async fn test_set_launch_at_login_off_when_never_on() {
            let ctx = make_ctx().await;
            let settings = Settings::new_fake();

            set_launch_at_login(&ctx, &settings, false).await.unwrap();
            assert_eq!(settings.get_bool(LAUNCH_ON_STARTUP_KEY).unwrap(), Some(false));
            assert!(AutostartIntegration::new(&*ctx).unwrap().is_installed().await.is_err());
        }
    }
}
