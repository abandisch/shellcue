use async_trait::async_trait;
use dbus::gnome_shell::{
    ExtensionInstallationStatus,
    ForeignCopy,
    GnomeShellVersion,
    ShellExtensions,
    foreign_extension_copy,
    get_extension_status,
    system_extension_directory,
};
use fig_os_shim::{
    EnvProvider,
    FsProvider,
    SysInfoProvider,
};
use fig_util::PRODUCT_NAME;

use crate::Integration;
use crate::error::{
    Error,
    Result,
};

/// What switching the extension on achieved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwitchOn {
    /// The shell had already loaded it and it was on.
    AlreadyOn,
    /// The running shell switched it on.
    On,
    /// It is now listed as on; the shell loads it at the next login.
    OnAfterRelogin,
    /// It was already listed as on; the shell loads it at the next login.
    AlreadyOnAfterRelogin,
}

/// The CLI arguments, after the binary name, that switch the extension on.
pub const SWITCH_ON_COMMAND: &str = "integrations install gnome-shell-extension";

pub fn not_found_message() -> String {
    format!(
        "The {PRODUCT_NAME} GNOME Shell extension was not found at {}. It is installed with the {PRODUCT_NAME} package; reinstall the package.",
        system_extension_directory().display()
    )
}

pub fn errored_message() -> String {
    format!(
        "GNOME Shell reports an error for the {PRODUCT_NAME} extension. The error is in the journal: journalctl -b /usr/bin/gnome-shell"
    )
}

pub fn out_of_date_message(version: Option<&GnomeShellVersion>) -> String {
    match version {
        Some(version) if version.minor.is_empty() => format!(
            "The {PRODUCT_NAME} GNOME Shell extension does not support this version of GNOME Shell ({}).",
            version.major
        ),
        Some(version) => format!(
            "The {PRODUCT_NAME} GNOME Shell extension does not support this version of GNOME Shell ({}.{}).",
            version.major, version.minor
        ),
        None => format!("The {PRODUCT_NAME} GNOME Shell extension does not support this version of GNOME Shell."),
    }
}

pub fn foreign_copy_message(copy: &ForeignCopy) -> String {
    match copy {
        ForeignCopy::Loaded(path) => format!(
            "GNOME Shell uses a copy of the {PRODUCT_NAME} extension at {} instead of the one installed with the {PRODUCT_NAME} package ({}). {PRODUCT_NAME} does not put it there. Remove that folder, then log out and log in again.",
            path.display(),
            system_extension_directory().display()
        ),
        ForeignCopy::Present(path) => format!(
            "There is a copy of the {PRODUCT_NAME} extension at {}. GNOME Shell may load it instead of the one installed with the {PRODUCT_NAME} package ({}). {PRODUCT_NAME} does not put it there. Remove that folder, then try again.",
            path.display(),
            system_extension_directory().display()
        ),
    }
}

/// What [SWITCH_ON_COMMAND] prints.
pub fn switch_on_text(outcome: SwitchOn) -> String {
    let mut text = match outcome {
        SwitchOn::On | SwitchOn::OnAfterRelogin => format!("The {PRODUCT_NAME} GNOME Shell extension is switched on."),
        SwitchOn::AlreadyOn | SwitchOn::AlreadyOnAfterRelogin => {
            format!("The {PRODUCT_NAME} GNOME Shell extension is already switched on.")
        },
    };
    if matches!(outcome, SwitchOn::OnAfterRelogin | SwitchOn::AlreadyOnAfterRelogin) {
        text.push_str("\nLog out and log in again so GNOME Shell loads it.");
    }
    text
}

/// What `integrations uninstall gnome-shell-extension` prints.
pub fn switch_off_text(changed: bool) -> String {
    if changed {
        format!("The {PRODUCT_NAME} GNOME Shell extension is switched off.")
    } else {
        format!("The {PRODUCT_NAME} GNOME Shell extension was not switched on.")
    }
}

/// What `integrations status gnome-shell-extension` prints; `listed` is
/// [ShellExtensions::is_listed_enabled].
pub fn status_text(status: ExtensionInstallationStatus, listed: bool, version: Option<&GnomeShellVersion>) -> String {
    let on = format!("The {PRODUCT_NAME} GNOME Shell extension is switched on.");
    match status {
        ExtensionInstallationStatus::Enabled => on,
        ExtensionInstallationStatus::RequiresRelogin if listed => {
            format!("{on}\nGNOME Shell loads it at the next login.")
        },
        ExtensionInstallationStatus::RequiresRelogin | ExtensionInstallationStatus::NotEnabled => {
            format!("The {PRODUCT_NAME} GNOME Shell extension is switched off.")
        },
        ExtensionInstallationStatus::NotInstalled => not_found_message(),
        ExtensionInstallationStatus::OutOfDate => out_of_date_message(version),
        ExtensionInstallationStatus::Errored => errored_message(),
        ExtensionInstallationStatus::GnomeShellNotRunning => "GNOME Shell is not running.".to_string(),
    }
}

#[derive(Debug, Clone)]
pub struct GnomeExtensionIntegration<'a, Ctx, ExtensionsCtx> {
    ctx: &'a Ctx,
    shell_extensions: &'a ShellExtensions<ExtensionsCtx>,
}

impl<'a, Ctx, ExtensionsCtx> GnomeExtensionIntegration<'a, Ctx, ExtensionsCtx>
where
    Ctx: FsProvider + EnvProvider + SysInfoProvider,
    ExtensionsCtx: FsProvider + EnvProvider + SysInfoProvider + Send + Sync,
{
    pub fn new(ctx: &'a Ctx, shell_extensions: &'a ShellExtensions<ExtensionsCtx>) -> Self {
        Self { ctx, shell_extensions }
    }

    /// Switches the extension on for the current user: over D-Bus when the running shell has
    /// loaded it, otherwise in gsettings so the shell switches it on at the next login.
    pub async fn switch_on(&self) -> Result<SwitchOn> {
        let status = get_extension_status(self.ctx, self.shell_extensions).await?;
        if matches!(
            status,
            ExtensionInstallationStatus::Enabled
                | ExtensionInstallationStatus::NotEnabled
                | ExtensionInstallationStatus::RequiresRelogin
        ) {
            if let Some(copy) = foreign_extension_copy(self.ctx, self.shell_extensions).await? {
                return Err(Error::Custom(foreign_copy_message(&copy).into()));
            }
        }
        match status {
            ExtensionInstallationStatus::Enabled => Ok(SwitchOn::AlreadyOn),
            ExtensionInstallationStatus::NotEnabled => {
                if self.shell_extensions.enable_extension().await? {
                    Ok(SwitchOn::On)
                } else {
                    Err(Error::Custom("GNOME Shell did not switch the extension on.".into()))
                }
            },
            ExtensionInstallationStatus::RequiresRelogin => {
                if self.shell_extensions.is_listed_enabled().await? {
                    Ok(SwitchOn::AlreadyOnAfterRelogin)
                } else {
                    self.shell_extensions.set_listed_enabled(true).await?;
                    Ok(SwitchOn::OnAfterRelogin)
                }
            },
            ExtensionInstallationStatus::NotInstalled => Err(Error::Custom(not_found_message().into())),
            ExtensionInstallationStatus::OutOfDate => Err(Error::Custom(
                out_of_date_message(self.shell_version().await.as_ref()).into(),
            )),
            ExtensionInstallationStatus::Errored => Err(Error::Custom(errored_message().into())),
            ExtensionInstallationStatus::GnomeShellNotRunning => Err(Error::Custom(
                "GNOME Shell is not running. The extension is only for GNOME Shell.".into(),
            )),
        }
    }

    /// Switches the extension off for the current user. Returns whether anything changed.
    pub async fn switch_off(&self) -> Result<bool> {
        let status = get_extension_status(self.ctx, self.shell_extensions).await?;
        if status == ExtensionInstallationStatus::Enabled {
            return Ok(self.shell_extensions.disable_extension().await?);
        }
        let listed = match self.shell_extensions.is_listed_enabled().await {
            Ok(listed) => listed,
            // Without GNOME Shell there may be no org.gnome.shell schema, so nothing to switch off.
            Err(_) if status == ExtensionInstallationStatus::GnomeShellNotRunning => return Ok(false),
            Err(err) => return Err(err.into()),
        };
        if listed {
            self.shell_extensions.set_listed_enabled(false).await?;
        }
        Ok(listed)
    }

    async fn shell_version(&self) -> Option<GnomeShellVersion> {
        self.shell_extensions.gnome_shell_version().await.ok()
    }
}

#[async_trait]
impl<Ctx, ExtensionsCtx> Integration for GnomeExtensionIntegration<'_, Ctx, ExtensionsCtx>
where
    Ctx: FsProvider + EnvProvider + SysInfoProvider + Sync,
    ExtensionsCtx: FsProvider + EnvProvider + SysInfoProvider + Send + Sync,
{
    fn describe(&self) -> String {
        "GNOME Extension Integration".to_owned()
    }

    async fn install(&self) -> Result<()> {
        self.switch_on().await?;
        Ok(())
    }

    async fn uninstall(&self) -> Result<()> {
        self.switch_off().await?;
        Ok(())
    }

    async fn is_installed(&self) -> Result<()> {
        match get_extension_status(self.ctx, self.shell_extensions).await? {
            ExtensionInstallationStatus::GnomeShellNotRunning => Err(Error::Custom(
                "GNOME Shell is not running, cannot determine installation status".into(),
            )),
            ExtensionInstallationStatus::Enabled => Ok(()),
            ExtensionInstallationStatus::RequiresRelogin => {
                if self.shell_extensions.is_listed_enabled().await? {
                    Ok(())
                } else {
                    Err(Error::NotInstalled(self.describe().into()))
                }
            },
            ExtensionInstallationStatus::NotEnabled | ExtensionInstallationStatus::NotInstalled => {
                Err(Error::NotInstalled(self.describe().into()))
            },
            ExtensionInstallationStatus::Errored => Err(Error::ImproperInstallation(errored_message().into())),
            ExtensionInstallationStatus::OutOfDate => {
                Err(Error::ImproperInstallation(
                    out_of_date_message(self.shell_version().await.as_ref()).into(),
                ))
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use dbus::gnome_shell::{
        EXTENSION_UUID,
        ExtensionState,
        GNOME_SHELL_PROCESS_NAME,
        user_extension_directory,
    };
    use fig_os_shim::Context;

    use super::*;

    async fn make_ctx() -> Arc<Context> {
        Context::builder()
            .with_test_home()
            .await
            .unwrap()
            .with_running_processes(&[GNOME_SHELL_PROCESS_NAME])
            .build_fake()
    }

    #[tokio::test]
    async fn test_switch_on_when_enabled() {
        let ctx = make_ctx().await;
        let shell_extensions = ShellExtensions::new_fake(Arc::downgrade(&ctx));
        shell_extensions
            .install_for_fake(Some(ExtensionState::Enabled))
            .await
            .unwrap();
        let integration = GnomeExtensionIntegration::new(&*ctx, &shell_extensions);

        assert_eq!(integration.switch_on().await.unwrap(), SwitchOn::AlreadyOn);
        assert!(integration.is_installed().await.is_ok());
    }

    #[tokio::test]
    async fn test_switch_on_when_not_enabled() {
        let ctx = make_ctx().await;
        let shell_extensions = ShellExtensions::new_fake(Arc::downgrade(&ctx));
        shell_extensions
            .install_for_fake(Some(ExtensionState::Disabled))
            .await
            .unwrap();
        let integration = GnomeExtensionIntegration::new(&*ctx, &shell_extensions);
        assert!(matches!(integration.is_installed().await, Err(Error::NotInstalled(_))));

        assert_eq!(integration.switch_on().await.unwrap(), SwitchOn::On);
        assert_eq!(
            get_extension_status(&*ctx, &shell_extensions).await.unwrap(),
            ExtensionInstallationStatus::Enabled
        );
        assert!(integration.is_installed().await.is_ok());
        assert!(shell_extensions.is_listed_enabled().await.unwrap());
    }

    #[tokio::test]
    async fn test_switch_on_when_requires_relogin() {
        let ctx = make_ctx().await;
        let shell_extensions = ShellExtensions::new_fake(Arc::downgrade(&ctx));
        shell_extensions.install_for_fake(None).await.unwrap();
        let integration = GnomeExtensionIntegration::new(&*ctx, &shell_extensions);
        assert!(matches!(integration.is_installed().await, Err(Error::NotInstalled(_))));

        assert_eq!(integration.switch_on().await.unwrap(), SwitchOn::OnAfterRelogin);
        assert!(shell_extensions.is_listed_enabled().await.unwrap());
        assert!(integration.is_installed().await.is_ok());

        assert_eq!(integration.switch_on().await.unwrap(), SwitchOn::AlreadyOnAfterRelogin);
    }

    #[tokio::test]
    async fn test_switch_on_errors() {
        // Not installed
        let ctx = make_ctx().await;
        let shell_extensions = ShellExtensions::new_fake(Arc::downgrade(&ctx));
        let integration = GnomeExtensionIntegration::new(&*ctx, &shell_extensions);
        let err = integration.switch_on().await.unwrap_err();
        assert_eq!(err.to_string(), not_found_message());
        assert!(matches!(integration.is_installed().await, Err(Error::NotInstalled(_))));
        assert!(!shell_extensions.is_listed_enabled().await.unwrap());

        // Out of date
        let ctx = make_ctx().await;
        let shell_extensions = ShellExtensions::new_fake(Arc::downgrade(&ctx));
        shell_extensions
            .install_for_fake(Some(ExtensionState::OutOfDate))
            .await
            .unwrap();
        let integration = GnomeExtensionIntegration::new(&*ctx, &shell_extensions);
        let err = integration.switch_on().await.unwrap_err();
        assert_eq!(
            err.to_string(),
            format!("The {PRODUCT_NAME} GNOME Shell extension does not support this version of GNOME Shell (45.0).")
        );
        assert!(matches!(integration.is_installed().await, Err(Error::ImproperInstallation(_))));

        // Errored
        let ctx = make_ctx().await;
        let shell_extensions = ShellExtensions::new_fake(Arc::downgrade(&ctx));
        shell_extensions
            .install_for_fake(Some(ExtensionState::Errored))
            .await
            .unwrap();
        let integration = GnomeExtensionIntegration::new(&*ctx, &shell_extensions);
        let err = integration.switch_on().await.unwrap_err();
        assert_eq!(err.to_string(), errored_message());
        assert!(matches!(integration.is_installed().await, Err(Error::ImproperInstallation(_))));

        // GNOME Shell not running
        let ctx = Context::builder().with_test_home().await.unwrap().build_fake();
        let shell_extensions = ShellExtensions::new_fake(Arc::downgrade(&ctx));
        let integration = GnomeExtensionIntegration::new(&*ctx, &shell_extensions);
        let err = integration.switch_on().await.unwrap_err();
        assert_eq!(
            err.to_string(),
            "GNOME Shell is not running. The extension is only for GNOME Shell."
        );
        assert!(matches!(integration.is_installed().await, Err(Error::Custom(_))));
    }

    #[tokio::test]
    async fn test_switch_off_when_loaded() {
        let ctx = make_ctx().await;
        let shell_extensions = ShellExtensions::new_fake(Arc::downgrade(&ctx));
        shell_extensions
            .install_for_fake(Some(ExtensionState::Enabled))
            .await
            .unwrap();
        let integration = GnomeExtensionIntegration::new(&*ctx, &shell_extensions);

        assert!(integration.switch_off().await.unwrap());
        assert_eq!(
            get_extension_status(&*ctx, &shell_extensions).await.unwrap(),
            ExtensionInstallationStatus::NotEnabled
        );
        assert!(!integration.switch_off().await.unwrap());
    }

    #[tokio::test]
    async fn test_switch_off_when_not_loaded() {
        let ctx = make_ctx().await;
        let shell_extensions = ShellExtensions::new_fake(Arc::downgrade(&ctx));
        shell_extensions.install_for_fake(None).await.unwrap();
        let integration = GnomeExtensionIntegration::new(&*ctx, &shell_extensions);
        assert!(!integration.switch_off().await.unwrap());

        shell_extensions.set_listed_enabled(true).await.unwrap();
        assert!(integration.switch_off().await.unwrap());
        assert!(!shell_extensions.is_listed_enabled().await.unwrap());
        assert!(matches!(integration.is_installed().await, Err(Error::NotInstalled(_))));
    }

    #[tokio::test]
    async fn test_switch_off_in_gsettings_for_every_other_status() {
        let cases = [
            (true, Some(ExtensionState::Disabled), ExtensionInstallationStatus::NotEnabled),
            (true, Some(ExtensionState::OutOfDate), ExtensionInstallationStatus::OutOfDate),
            (true, Some(ExtensionState::Errored), ExtensionInstallationStatus::Errored),
            (true, None, ExtensionInstallationStatus::NotInstalled),
            (false, None, ExtensionInstallationStatus::GnomeShellNotRunning),
        ];
        for (running, state, expected) in cases {
            for listed in [false, true] {
                let ctx = if running {
                    make_ctx().await
                } else {
                    Context::builder().with_test_home().await.unwrap().build_fake()
                };
                let shell_extensions = ShellExtensions::new_fake(Arc::downgrade(&ctx));
                if let Some(state) = state {
                    shell_extensions.install_for_fake(Some(state)).await.unwrap();
                }
                assert_eq!(get_extension_status(&*ctx, &shell_extensions).await.unwrap(), expected);
                if listed {
                    shell_extensions.set_listed_enabled(true).await.unwrap();
                }
                let integration = GnomeExtensionIntegration::new(&*ctx, &shell_extensions);

                assert_eq!(integration.switch_off().await.unwrap(), listed, "{expected:?}");
                assert!(!shell_extensions.is_listed_enabled().await.unwrap(), "{expected:?}");
            }
        }
    }

    #[tokio::test]
    async fn test_switch_on_refuses_copy_loaded_elsewhere() {
        let ctx = make_ctx().await;
        let shell_extensions = ShellExtensions::new_fake(Arc::downgrade(&ctx));
        shell_extensions.install_for_fake(None).await.unwrap();
        let elsewhere = PathBuf::from("/home/testuser/.local/share/gnome-shell/extensions").join(EXTENSION_UUID);
        shell_extensions.load_for_fake(ExtensionState::Disabled, &elsewhere).await;
        let integration = GnomeExtensionIntegration::new(&*ctx, &shell_extensions);

        let err = integration.switch_on().await.unwrap_err();
        assert_eq!(err.to_string(), foreign_copy_message(&ForeignCopy::Loaded(elsewhere.clone())));
        assert!(err.to_string().contains(&*elsewhere.to_string_lossy()));
        assert!(!shell_extensions.is_listed_enabled().await.unwrap());
        assert_eq!(
            get_extension_status(&*ctx, &shell_extensions).await.unwrap(),
            ExtensionInstallationStatus::NotEnabled
        );
    }

    #[tokio::test]
    async fn test_switch_on_refuses_user_folder() {
        let ctx = make_ctx().await;
        let shell_extensions = ShellExtensions::new_fake(Arc::downgrade(&ctx));
        shell_extensions.install_for_fake(None).await.unwrap();
        let user_dir = user_extension_directory(&*ctx).unwrap();
        ctx.fs().create_dir_all(&user_dir).await.unwrap();
        let integration = GnomeExtensionIntegration::new(&*ctx, &shell_extensions);

        let err = integration.switch_on().await.unwrap_err();
        assert_eq!(err.to_string(), foreign_copy_message(&ForeignCopy::Present(user_dir.clone())));
        assert!(err.to_string().contains(&*user_dir.to_string_lossy()), "{err}");
        assert!(!shell_extensions.is_listed_enabled().await.unwrap());
        assert_eq!(
            get_extension_status(&*ctx, &shell_extensions).await.unwrap(),
            ExtensionInstallationStatus::RequiresRelogin
        );
    }

    #[test]
    fn test_foreign_copy_message() {
        let path = PathBuf::from("/home/testuser/.local/share/gnome-shell/extensions").join(EXTENSION_UUID);
        assert_eq!(
            foreign_copy_message(&ForeignCopy::Loaded(path.clone())),
            "GNOME Shell uses a copy of the ShellCue extension at \
             /home/testuser/.local/share/gnome-shell/extensions/shellcue-gnome-integration@bandisch-apps.com \
             instead of the one installed with the ShellCue package \
             (/usr/share/gnome-shell/extensions/shellcue-gnome-integration@bandisch-apps.com). \
             ShellCue does not put it there. Remove that folder, then log out and log in again."
        );
        assert_eq!(
            foreign_copy_message(&ForeignCopy::Present(path)),
            "There is a copy of the ShellCue extension at \
             /home/testuser/.local/share/gnome-shell/extensions/shellcue-gnome-integration@bandisch-apps.com. \
             GNOME Shell may load it instead of the one installed with the ShellCue package \
             (/usr/share/gnome-shell/extensions/shellcue-gnome-integration@bandisch-apps.com). \
             ShellCue does not put it there. Remove that folder, then try again."
        );
    }

    #[test]
    fn test_out_of_date_message() {
        let version = |minor: &str| GnomeShellVersion {
            major: 48,
            minor: minor.to_string(),
        };
        assert_eq!(
            out_of_date_message(Some(&version("1"))),
            "The ShellCue GNOME Shell extension does not support this version of GNOME Shell (48.1)."
        );
        assert_eq!(
            out_of_date_message(Some(&version(""))),
            "The ShellCue GNOME Shell extension does not support this version of GNOME Shell (48)."
        );
        assert_eq!(
            out_of_date_message(None),
            "The ShellCue GNOME Shell extension does not support this version of GNOME Shell."
        );
    }

    #[test]
    fn test_switch_on_text() {
        let cases = [
            (SwitchOn::On, "The ShellCue GNOME Shell extension is switched on."),
            (SwitchOn::AlreadyOn, "The ShellCue GNOME Shell extension is already switched on."),
            (
                SwitchOn::OnAfterRelogin,
                "The ShellCue GNOME Shell extension is switched on.\n\
                 Log out and log in again so GNOME Shell loads it.",
            ),
            (
                SwitchOn::AlreadyOnAfterRelogin,
                "The ShellCue GNOME Shell extension is already switched on.\n\
                 Log out and log in again so GNOME Shell loads it.",
            ),
        ];
        for (outcome, expected) in cases {
            assert_eq!(switch_on_text(outcome), expected);
        }
    }

    #[test]
    fn test_switch_off_text() {
        assert_eq!(switch_off_text(true), "The ShellCue GNOME Shell extension is switched off.");
        assert_eq!(switch_off_text(false), "The ShellCue GNOME Shell extension was not switched on.");
    }

    #[test]
    fn test_status_text() {
        let on = "The ShellCue GNOME Shell extension is switched on.";
        let off = "The ShellCue GNOME Shell extension is switched off.";
        let relogin = "The ShellCue GNOME Shell extension is switched on.\nGNOME Shell loads it at the next login.";
        let not_found = "The ShellCue GNOME Shell extension was not found at \
                         /usr/share/gnome-shell/extensions/shellcue-gnome-integration@bandisch-apps.com. \
                         It is installed with the ShellCue package; reinstall the package.";
        let out_of_date = "The ShellCue GNOME Shell extension does not support this version of GNOME Shell (47.2).";
        let errored = "GNOME Shell reports an error for the ShellCue extension. \
                       The error is in the journal: journalctl -b /usr/bin/gnome-shell";
        let version = GnomeShellVersion {
            major: 47,
            minor: "2".to_string(),
        };
        for listed in [false, true] {
            let cases = [
                (ExtensionInstallationStatus::Enabled, on),
                (ExtensionInstallationStatus::RequiresRelogin, if listed { relogin } else { off }),
                (ExtensionInstallationStatus::NotEnabled, off),
                (ExtensionInstallationStatus::NotInstalled, not_found),
                (ExtensionInstallationStatus::OutOfDate, out_of_date),
                (ExtensionInstallationStatus::Errored, errored),
                (ExtensionInstallationStatus::GnomeShellNotRunning, "GNOME Shell is not running."),
            ];
            for (status, expected) in cases {
                assert_eq!(status_text(status, listed, Some(&version)), expected, "{status:?} {listed}");
            }
        }
        assert_eq!(not_found_message(), not_found);
        assert_eq!(errored_message(), errored);
    }
}
