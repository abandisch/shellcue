use std::borrow::Cow;
use std::process::Command;
use std::sync::Arc;

use async_trait::async_trait;
use dbus::gnome_shell::{
    ExtensionInstallationStatus,
    ShellExtensions,
    foreign_extension_copy,
    get_extension_status,
};
use fig_integrations::gnome_extension::{
    SWITCH_ON_COMMAND,
    errored_message,
    foreign_copy_message,
    not_found_message,
    out_of_date_message,
};
use fig_ipc::local::send_recv_command_to_socket;
use fig_os_shim::Context;
use fig_proto::local::command::Command as IpcCommand;
use fig_proto::local::command_response::Response;
use fig_proto::local::{
    CommandResponse,
    ConnectToIBusCommand,
};
use fig_util::Terminal;
use fig_util::consts::{
    CLI_BINARY_NAME,
    PRODUCT_NAME,
};
use fig_util::system_info::linux::{
    DesktopEnvironment,
    DisplayServer,
    get_desktop_environment,
    get_display_server,
};

use crate::cli::doctor::{
    DoctorCheck,
    DoctorCheckType,
    DoctorError,
    DoctorFix,
    Platform,
    doctor_error,
    doctor_fix,
    doctor_warning,
};

#[derive(Debug)]
pub struct LinuxContext {
    ctx: Arc<Context>,
    shell_extensions: Arc<ShellExtensions<Context>>,
}

impl LinuxContext {
    fn new(ctx: Arc<Context>, shell_extensions: Arc<ShellExtensions<Context>>) -> Self {
        Self { ctx, shell_extensions }
    }
}

impl From<Arc<Context>> for LinuxContext {
    fn from(ctx: Arc<Context>) -> Self {
        let shell_extensions = Arc::new(ShellExtensions::new(Arc::downgrade(&ctx)));
        Self { ctx, shell_extensions }
    }
}

pub async fn get_linux_context() -> eyre::Result<LinuxContext> {
    let ctx = Context::new();
    let shell_extensions = Arc::new(ShellExtensions::new(Arc::downgrade(&ctx)));
    Ok(LinuxContext::new(ctx, shell_extensions))
}

pub struct DisplayServerCheck;

#[async_trait]
impl DoctorCheck<LinuxContext> for DisplayServerCheck {
    fn name(&self) -> Cow<'static, str> {
        "Display Server Check".into()
    }

    async fn get_type(&self, _: &LinuxContext, _: Platform) -> DoctorCheckType {
        DoctorCheckType::NormalCheck
    }

    async fn check(&self, ctx: &LinuxContext) -> Result<(), DoctorError> {
        match get_display_server(&ctx.ctx) {
            Ok(_) => Ok(()),
            Err(fig_util::Error::UnknownDisplayServer(server)) => Err(doctor_error!(
                "Unknown value set for XDG_SESSION_TYPE: {}. This must be set to x11 or wayland.",
                server
            )),
            Err(err) => Err(doctor_error!(
                "Unknown error occurred when detecting the display server: {:?}. Is XDG_SESSION_TYPE set to x11 or wayland?",
                err
            )),
        }
    }
}

pub struct IBusEnvCheck;

#[async_trait]
impl DoctorCheck<LinuxContext> for IBusEnvCheck {
    fn name(&self) -> Cow<'static, str> {
        "IBus Env Check".into()
    }

    async fn get_type(&self, _: &LinuxContext, _: Platform) -> DoctorCheckType {
        DoctorCheckType::NormalCheck
    }

    async fn check(&self, ctx: &LinuxContext) -> Result<(), DoctorError> {
        let ctx = &ctx.ctx;

        #[derive(Debug)]
        struct EnvErr {
            var: Cow<'static, str>,
            actual: Option<String>,
            expected: Cow<'static, str>,
        }

        let env = ctx.env();
        let mut checks = vec![("QT_IM_MODULE", "ibus"), ("XMODIFIERS", "@im=ibus")];
        let mut errors: Vec<EnvErr> = vec![];

        // GNOME uses IBus natively, so GTK_IM_MODULE may be unset (as on stock Ubuntu).
        // Only error if it's set to something other than ibus.
        match get_desktop_environment(ctx)? {
            DesktopEnvironment::Gnome => match env.get("GTK_IM_MODULE") {
                Ok(actual) if actual != "ibus" => errors.push(EnvErr {
                    var: "GTK_IM_MODULE".into(),
                    actual: actual.into(),
                    expected: "ibus".into(),
                }),
                _ => (),
            },
            _ => checks.push(("GTK_IM_MODULE", "ibus")),
        };

        // IME is disabled in Kitty by default.
        // https://github.com/kovidgoyal/kitty/issues/469#issuecomment-419406438
        if let (Some(Terminal::Kitty), DisplayServer::X11) = (Terminal::parent_terminal(ctx), get_display_server(ctx)?)
        {
            match env.get("GLFW_IM_MODULE") {
                Ok(actual) if actual == "ibus" => (),
                Ok(actual) => errors.push(EnvErr {
                    var: "GLFW_IM_MODULE".into(),
                    actual: Some(actual),
                    expected: "ibus".into(),
                }),
                _ => errors.push(EnvErr {
                    var: "GLFW_IM_MODULE".into(),
                    actual: None,
                    expected: "ibus".into(),
                }),
            }
        }

        errors.append(
            &mut checks
                .iter()
                .filter_map(|(var, expected)| match env.get(var) {
                    Ok(actual) if actual.contains(expected) => None,
                    Ok(actual) => Some(EnvErr {
                        var: (*var).into(),
                        actual: Some(actual),
                        expected: (*expected).into(),
                    }),
                    Err(_) => Some(EnvErr {
                        var: (*var).into(),
                        actual: None,
                        expected: (*expected).into(),
                    }),
                })
                .collect::<Vec<_>>(),
        );

        if !errors.is_empty() {
            let mut info = vec![
                "The input method is required to be configured for IBus in order for autocomplete to work.".into(),
            ];
            info.append(
                &mut errors
                    .iter()
                    .map(|err| {
                        if let Some(actual) = &err.actual {
                            format!("{} is '{}', expected '{}'", err.var, actual, err.expected).into()
                        } else {
                            format!("{} is not set, expected '{}'", err.var, err.expected).into()
                        }
                    })
                    .collect::<Vec<_>>(),
            );
            Err(DoctorError::Error {
                reason: "IBus environment variable is not set".into(),
                info,
                fix: None,
                error: None,
            })
        } else {
            Ok(())
        }
    }
}

pub struct GnomeExtensionCheck;

#[async_trait]
impl DoctorCheck<LinuxContext> for GnomeExtensionCheck {
    fn name(&self) -> Cow<'static, str> {
        "GNOME Shell Extension Check".into()
    }

    async fn get_type(&self, _: &LinuxContext, _: Platform) -> DoctorCheckType {
        DoctorCheckType::NormalCheck
    }

    async fn check(&self, ctx: &LinuxContext) -> Result<(), DoctorError> {
        let (ctx, shell_extensions) = (&ctx.ctx, ctx.shell_extensions.as_ref());

        if get_desktop_environment(ctx)? != DesktopEnvironment::Gnome {
            return Ok(());
        }

        if get_display_server(ctx)? == DisplayServer::X11 {
            return Ok(());
        }

        let status = get_extension_status(ctx, shell_extensions)
            .await
            .map_err(eyre::Report::from)?;
        if status != ExtensionInstallationStatus::GnomeShellNotRunning {
            if let Ok(Some(copy)) = foreign_extension_copy(ctx, shell_extensions).await {
                return Err(DoctorError::error(foreign_copy_message(&copy)));
            }
        }
        match status {
            ExtensionInstallationStatus::GnomeShellNotRunning => Err(DoctorError::Error {
                reason: "The gnome-shell process doesn't appear to be running.".into(),
                info: vec![],
                fix: None,
                error: None,
            }),
            ExtensionInstallationStatus::NotInstalled => Err(DoctorError::error(not_found_message())),
            ExtensionInstallationStatus::RequiresRelogin => match shell_extensions.is_listed_enabled().await {
                Ok(true) => Err(doctor_error!(
                    "The {PRODUCT_NAME} GNOME Shell extension is switched on but GNOME Shell has not loaded it yet. Log out and log in again."
                )),
                _ => Err(doctor_error!(
                    "The {PRODUCT_NAME} GNOME Shell extension is installed but GNOME Shell has not loaded it. Run `{CLI_BINARY_NAME} {SWITCH_ON_COMMAND}`, then log out and log in again."
                )),
            },
            ExtensionInstallationStatus::OutOfDate => {
                let version = shell_extensions.gnome_shell_version().await.ok();
                Err(DoctorError::error(out_of_date_message(version.as_ref())))
            },
            ExtensionInstallationStatus::Errored => Err(DoctorError::error(errored_message())),
            ExtensionInstallationStatus::NotEnabled => Err(doctor_error!(
                "The {PRODUCT_NAME} GNOME Shell extension is switched off. To switch it on, run `{CLI_BINARY_NAME} {SWITCH_ON_COMMAND}`."
            )),
            ExtensionInstallationStatus::Enabled => Ok(()),
        }
    }
}

pub struct IBusRunningCheck;

#[async_trait]
impl DoctorCheck<LinuxContext> for IBusRunningCheck {
    fn name(&self) -> Cow<'static, str> {
        "IBus Check".into()
    }

    async fn get_type(&self, _: &LinuxContext, _: Platform) -> DoctorCheckType {
        DoctorCheckType::NormalCheck
    }

    async fn check(&self, _: &LinuxContext) -> Result<(), DoctorError> {
        use std::ffi::OsString;

        use sysinfo::{
            ProcessRefreshKind,
            RefreshKind,
        };

        let system =
            sysinfo::System::new_with_specifics(RefreshKind::nothing().with_processes(ProcessRefreshKind::nothing()));
        let ibus_daemon = OsString::from("ibus-daemon");

        if system.processes_by_exact_name(&ibus_daemon).next().is_none() {
            return Err(doctor_fix!({
                reason: "ibus-daemon is not running",
                fix: || {
                    // Launches a new ibus-daemon process.
                    // -d - run in the background (daemonize)
                    // -r - replace current ibus-daemon, if running
                    // -x - execute XIM server
                    // -R - restarts other ibus subprocesses if they end
                    match Command::new("ibus-daemon").arg("-drxR").output() {
                        Ok(output) if !output.status.success() => {
                            let stdout = String::from_utf8_lossy(&output.stdout);
                            let stderr = String::from_utf8_lossy(&output.stderr);
                            eyre::bail!("ibus-daemon launch failed:\nstdout: {stdout}\nstderr: {stderr}\n");
                        },
                        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                            eyre::bail!("Could not find ibus-daemon. Is ibus installed?");
                        },
                        Err(err) => {
                            eyre::bail!("An unknown error occurred launching ibus-daemon: {:?}", err);
                        }
                        Ok(_) => ()
                    }
                    // Wait some time for ibus-daemon to launch.
                    std::thread::sleep(std::time::Duration::from_secs(1));
                    Ok(())
            }}));
        }

        Ok(())
    }
}

pub struct IBusConnectionCheck;

#[async_trait]
impl DoctorCheck<LinuxContext> for IBusConnectionCheck {
    fn name(&self) -> Cow<'static, str> {
        "IBus Connection Check".into()
    }

    async fn get_type(&self, _: &LinuxContext, _: Platform) -> DoctorCheckType {
        DoctorCheckType::NormalCheck
    }

    async fn check(&self, _: &LinuxContext) -> Result<(), DoctorError> {
        match send_recv_command_to_socket(IpcCommand::ConnectToIbus(ConnectToIBusCommand {})).await {
            Ok(Some(CommandResponse {
                response: Some(Response::Success(_)),
                ..
            })) => Ok(()),
            Ok(Some(CommandResponse { response, .. })) => {
                Err(doctor_error!("Desktop app failed to connect to ibus: {:?}", response))
            },
            Ok(None) => Err(doctor_error!("Desktop app failed to connect to ibus")),
            Err(err) => Err(doctor_error!(
                "Failed to send the ibus connection command to the desktop: {}",
                err
            )),
        }
    }
}

pub struct SandboxCheck;

#[async_trait]
impl DoctorCheck<LinuxContext> for SandboxCheck {
    fn name(&self) -> Cow<'static, str> {
        "App is not running in a sandbox".into()
    }

    async fn check(&self, _: &LinuxContext) -> Result<(), DoctorError> {
        use fig_util::system_info::linux::SandboxKind;

        let kind = fig_util::system_info::linux::detect_sandbox();

        match kind {
            SandboxKind::None => Ok(()),
            SandboxKind::Flatpak => Err(doctor_error!("Running under Flatpak is not supported.")),
            SandboxKind::Snap => Err(doctor_error!("Running under Snap is not supported.")),
            SandboxKind::Docker => Err(doctor_warning!(
                "Support for Docker is in development. It may not work properly on your system."
            )),
            SandboxKind::Container(Some(engine)) => {
                Err(doctor_error!("Running under `{engine}` containers is not supported."))
            },
            SandboxKind::Container(None) => Err(doctor_error!("Running under non-docker containers is not supported.")),
        }
    }
}

#[cfg(test)]
mod tests {
    use dbus::gnome_shell::{
        ExtensionState,
        GNOME_SHELL_PROCESS_NAME,
        user_extension_directory,
    };
    use fig_os_shim::{
        Env,
        ProcessInfo,
    };

    use super::*;

    #[tokio::test]
    async fn test_ibus_env_check() {
        let ctx = Context::builder()
            .with_env(Env::from_slice(&[
                ("XDG_CURRENT_DESKTOP", "ubuntu:GNOME"),
                ("GTK_IM_MODULE", "ibus"),
                ("QT_IM_MODULE", "ibus"),
                ("XMODIFIERS", "@im=ibus"),
            ]))
            .build_fake();
        assert!(
            IBusEnvCheck.check(&ctx.into()).await.is_ok(),
            "should succeed with all env vars set"
        );

        let ctx = Context::builder()
            .with_env(Env::from_slice(&[
                ("XDG_CURRENT_DESKTOP", "ubuntu:GNOME"),
                ("QT_IM_MODULE", "ibus"),
                ("XMODIFIERS", "@im=ibus"),
            ]))
            .build_fake();
        assert!(
            IBusEnvCheck.check(&ctx.into()).await.is_ok(),
            "GTK_IM_MODULE may be unset on GNOME"
        );

        let ctx = Context::builder()
            .with_process_info(ProcessInfo::from_exes(vec!["shellcue", "bash", "kitty"]))
            .with_env(Env::from_slice(&[
                ("XDG_CURRENT_DESKTOP", "ubuntu:GNOME"),
                ("GTK_IM_MODULE", "ibus"),
                ("QT_IM_MODULE", "ibus"),
                ("XMODIFIERS", "@im=ibus"),
            ]))
            .build_fake();
        assert!(
            IBusEnvCheck.check(&ctx.into()).await.is_err(),
            "error on kitty when IME is not enabled"
        );

        let ctx = Context::builder()
            .with_env(Env::from_slice(&[
                ("XDG_CURRENT_DESKTOP", "ubuntu:GNOME"),
                ("GTK_IM_MODULE", "gtk-im-context-simple"),
                ("QT_IM_MODULE", "simple"),
                ("XMODIFIERS", "@im=null"),
            ]))
            .build_fake();
        assert!(
            IBusEnvCheck.check(&ctx.into()).await.is_err(),
            "fail when input method is disabled"
        );

        let ctx = Context::builder()
            .with_env(Env::from_slice(&[
                ("XDG_CURRENT_DESKTOP", "fedora:KDE"),
                ("QT_IM_MODULE", "ibus"),
                ("XMODIFIERS", "@im=ibus"),
            ]))
            .build_fake();
        assert!(
            IBusEnvCheck.check(&ctx.into()).await.is_err(),
            "fail when missing GTK_IM_MODULE on non-gnome desktops"
        );

        let ctx = Context::builder()
            .with_env(Env::from_slice(&[("XDG_CURRENT_DESKTOP", "fedora:KDE")]))
            .build_fake();
        let err = IBusEnvCheck.check(&ctx.into()).await.unwrap_err();
        #[allow(clippy::match_wildcard_for_single_variants)]
        match err {
            DoctorError::Error { info, .. } => {
                let info = info.join("\n");
                for var in &["GTK_IM_MODULE", "QT_IM_MODULE", "XMODIFIERS"] {
                    assert!(
                        info.contains(var),
                        "error info should contain all env vars. Actual info: {}",
                        info
                    );
                }
            },
            _ => panic!("missing env vars should error"),
        }
    }

    #[tokio::test]
    async fn test_gnome_extension_check() {
        let ctx = Context::builder()
            .with_env(Env::from_slice(&[
                ("XDG_SESSION_TYPE", "x11"),
                ("XDG_CURRENT_DESKTOP", "ubuntu:GNOME"),
            ]))
            .build_fake();
        let check = GnomeExtensionCheck.check(&ctx.into()).await;
        assert!(
            check.is_ok(),
            "x11 on GNOME shouldn't require the extension. Error: {:?}",
            check
        );

        let ctx = gnome_wayland_ctx().await;
        let shell_extensions = ShellExtensions::new_fake(Arc::downgrade(&ctx));
        let check = GnomeExtensionCheck
            .check(&LinuxContext::new(ctx, shell_extensions.into()))
            .await;
        assert!(
            matches!(
                &check,
                Err(DoctorError::Error { reason, fix: None, .. })
                    if reason.contains("was not found at /usr/share/gnome-shell/extensions/")
            ),
            "extension not installed should error. Got: {:?}",
            check
        );

        let ctx = gnome_wayland_ctx().await;
        let shell_extensions = ShellExtensions::new_fake(Arc::downgrade(&ctx));
        shell_extensions
            .install_for_fake(Some(ExtensionState::Disabled))
            .await
            .unwrap();
        shell_extensions.enable_extension().await.unwrap();
        let check = GnomeExtensionCheck
            .check(&LinuxContext::new(ctx, shell_extensions.into()))
            .await;
        assert!(
            check.is_ok(),
            "extension installed, loaded, and enabled should not error. Error: {:?}",
            check
        );

        let ctx = Context::builder()
            .with_test_home()
            .await
            .unwrap()
            .with_env_var("XDG_SESSION_TYPE", "wayland")
            .with_env_var("XDG_CURRENT_DESKTOP", "ubuntu:GNOME")
            .build_fake();
        let shell_extensions = ShellExtensions::new_fake(Arc::downgrade(&ctx));
        let check = GnomeExtensionCheck
            .check(&LinuxContext::new(ctx, shell_extensions.into()))
            .await;
        assert!(
            matches!(
                &check,
                Err(DoctorError::Error { reason, .. }) if reason.contains("gnome-shell process")
            ),
            "GNOME Shell not running should error. Got: {:?}",
            check
        );
    }

    async fn gnome_wayland_ctx() -> Arc<Context> {
        Context::builder()
            .with_test_home()
            .await
            .unwrap()
            .with_env_var("XDG_SESSION_TYPE", "wayland")
            .with_env_var("XDG_CURRENT_DESKTOP", "ubuntu:GNOME")
            .with_running_processes(&[GNOME_SHELL_PROCESS_NAME])
            .build_fake()
    }

    #[tokio::test]
    async fn test_gnome_extension_check_statuses() {
        let cases: [(Option<ExtensionState>, &str); 5] = [
            (None, "GNOME Shell has not loaded it. Run `shellcue integrations install gnome-shell-extension`"),
            (Some(ExtensionState::OutOfDate), "does not support this version of GNOME Shell (45.0)."),
            (Some(ExtensionState::Errored), "journalctl -b /usr/bin/gnome-shell"),
            (Some(ExtensionState::Disabled), "is switched off. To switch it on, run"),
            (Some(ExtensionState::Initialized), "is switched off. To switch it on, run"),
        ];
        for (state, expected) in cases {
            let ctx = gnome_wayland_ctx().await;
            let shell_extensions = ShellExtensions::new_fake(Arc::downgrade(&ctx));
            shell_extensions.install_for_fake(state).await.unwrap();
            let check = GnomeExtensionCheck
                .check(&LinuxContext::new(ctx, shell_extensions.into()))
                .await;
            assert!(
                matches!(
                    &check,
                    Err(DoctorError::Error { reason, fix: None, .. }) if reason.contains(expected)
                ),
                "state {:?} should error with {:?} and no fix. Got: {:?}",
                state,
                expected,
                check
            );
        }
    }

    #[tokio::test]
    async fn test_gnome_extension_check_loaded_from_elsewhere() {
        let elsewhere = std::path::Path::new(
            "/home/testuser/.local/share/gnome-shell/extensions/shellcue-gnome-integration@bandisch-apps.com",
        );
        for state in [
            ExtensionState::Enabled,
            ExtensionState::Disabled,
            ExtensionState::Errored,
            ExtensionState::OutOfDate,
        ] {
            let ctx = gnome_wayland_ctx().await;
            let shell_extensions = ShellExtensions::new_fake(Arc::downgrade(&ctx));
            shell_extensions.install_for_fake(None).await.unwrap();
            shell_extensions.load_for_fake(state, elsewhere).await;
            let check = GnomeExtensionCheck
                .check(&LinuxContext::new(ctx, shell_extensions.into()))
                .await;
            assert!(
                matches!(
                    &check,
                    Err(DoctorError::Error { reason, .. })
                        if reason.contains(&*elsewhere.to_string_lossy()) && reason.contains("does not put it there")
                ),
                "state {:?} loaded from outside the package should error. Got: {:?}",
                state,
                check
            );
        }
    }

    #[tokio::test]
    async fn test_gnome_extension_check_user_folder_while_not_loaded() {
        let ctx = gnome_wayland_ctx().await;
        let shell_extensions = ShellExtensions::new_fake(Arc::downgrade(&ctx));
        shell_extensions.install_for_fake(None).await.unwrap();
        let user_dir = user_extension_directory(&ctx).unwrap();
        ctx.fs().create_dir_all(&user_dir).await.unwrap();
        let check = GnomeExtensionCheck
            .check(&LinuxContext::new(ctx, shell_extensions.into()))
            .await;
        assert!(
            matches!(
                &check,
                Err(DoctorError::Error { reason, .. })
                    if reason.contains(&*user_dir.to_string_lossy()) && reason.contains("Remove that folder")
            ),
            "a user copy of the extension should error. Got: {:?}",
            check
        );
    }

    #[tokio::test]
    async fn test_gnome_extension_check_not_loaded_wording() {
        for listed in [false, true] {
            let ctx = gnome_wayland_ctx().await;
            let shell_extensions = ShellExtensions::new_fake(Arc::downgrade(&ctx));
            shell_extensions.install_for_fake(None).await.unwrap();
            if listed {
                shell_extensions.set_listed_enabled(true).await.unwrap();
            }
            let check = GnomeExtensionCheck
                .check(&LinuxContext::new(ctx, shell_extensions.into()))
                .await;
            let expected = if listed {
                "The ShellCue GNOME Shell extension is switched on but GNOME Shell has not loaded it yet. \
                 Log out and log in again."
            } else {
                "The ShellCue GNOME Shell extension is installed but GNOME Shell has not loaded it. \
                 Run `shellcue integrations install gnome-shell-extension`, then log out and log in again."
            };
            assert!(
                matches!(&check, Err(DoctorError::Error { reason, .. }) if reason == expected),
                "listed {listed}: expected {expected:?}. Got: {check:?}"
            );
        }
    }

    #[tokio::test]
    async fn test_gnome_extension_check_switches_nothing_on() {
        let ctx = gnome_wayland_ctx().await;
        let shell_extensions = Arc::new(ShellExtensions::new_fake(Arc::downgrade(&ctx)));
        shell_extensions
            .install_for_fake(Some(ExtensionState::Disabled))
            .await
            .unwrap();
        let check = GnomeExtensionCheck
            .check(&LinuxContext::new(Arc::clone(&ctx), Arc::clone(&shell_extensions)))
            .await;
        assert!(check.is_err(), "switched off should error. Got: {check:?}");
        assert_eq!(
            get_extension_status(&ctx, shell_extensions.as_ref()).await.unwrap(),
            ExtensionInstallationStatus::NotEnabled
        );
        assert!(!shell_extensions.is_listed_enabled().await.unwrap());
    }

    #[tokio::test]
    async fn test_gnome_extension_check_unknown_session_type() {
        let ctx = Context::builder()
            .with_test_home()
            .await
            .unwrap()
            .with_env_var("XDG_SESSION_TYPE", "mir")
            .with_env_var("XDG_CURRENT_DESKTOP", "ubuntu:GNOME")
            .with_running_processes(&[GNOME_SHELL_PROCESS_NAME])
            .build_fake();
        let shell_extensions = ShellExtensions::new_fake(Arc::downgrade(&ctx));
        let check = GnomeExtensionCheck
            .check(&LinuxContext::new(ctx, shell_extensions.into()))
            .await;
        assert!(check.is_err(), "an unknown session type should error. Got: {check:?}");
    }

    #[tokio::test]
    async fn test_ibus_env_check_gtk_im_module_wrong_on_gnome() {
        let ctx = Context::builder()
            .with_env(Env::from_slice(&[
                ("XDG_CURRENT_DESKTOP", "ubuntu:GNOME"),
                ("GTK_IM_MODULE", "xim"),
                ("QT_IM_MODULE", "ibus"),
                ("XMODIFIERS", "@im=ibus"),
            ]))
            .build_fake();
        let err = IBusEnvCheck.check(&ctx.into()).await.unwrap_err();
        #[allow(clippy::match_wildcard_for_single_variants)]
        match err {
            DoctorError::Error { info, .. } => {
                let info = info.join("\n");
                assert!(info.contains("GTK_IM_MODULE"), "{info}");
                assert!(!info.contains("QT_IM_MODULE") && !info.contains("XMODIFIERS"), "{info}");
            },
            _ => panic!("GTK_IM_MODULE=xim on GNOME should error"),
        }
    }

    mod e2e {
        use super::*;

        #[tokio::test]
        #[ignore = "not in ci"]
        async fn test_gnome_extension_check() {
            let ctx = get_linux_context().await.unwrap();
            GnomeExtensionCheck {}.check(&ctx).await.unwrap();
        }

        #[tokio::test]
        #[ignore = "not in ci"]
        async fn test_ibus_check() {
            let ctx = get_linux_context().await.unwrap();
            IBusRunningCheck {}.check(&ctx).await.unwrap();
        }

        #[tokio::test]
        #[ignore = "not in ci"]
        async fn test_ibus_connection_check() {
            let ctx = get_linux_context().await.unwrap();
            IBusConnectionCheck {}.check(&ctx).await.unwrap();
        }
    }
}
