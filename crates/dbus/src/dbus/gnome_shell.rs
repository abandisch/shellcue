use std::borrow::Cow;
use std::collections::HashMap;
use std::path::{
    Path,
    PathBuf,
};
use std::str::FromStr;
use std::sync::{
    Arc,
    Weak,
};

use fig_os_shim::{
    EnvProvider,
    FsProvider,
    SysInfoProvider,
};
use fig_util::directories::{
    DirectoryError,
    home_dir_ctx,
};
use thiserror::Error;
use tokio::process::Command;
use tokio::sync::Mutex;
use zbus::proxy;
use zbus::zvariant::{
    OwnedValue,
    Value,
};

use super::session_bus;
use crate::CrateError;

pub const GNOME_SHELL_PROCESS_NAME: &str = "gnome-shell";

/// Extension uuid of the ShellCue GNOME Shell extension (GNOME Shell 45 to 47).
pub const EXTENSION_UUID: &str = "shellcue-gnome-integration@bandisch-apps.com";

const GSETTINGS_PATH: &str = "/usr/bin/gsettings";

const GNOME_SHELL_SCHEMA: &str = "org.gnome.shell";

/// Represents the installation status for the ShellCue GNOME Shell extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtensionInstallationStatus {
    /// The GNOME Shell process is not running, so the status cannot be asked over D-Bus.
    GnomeShellNotRunning,

    /// The packaged extension folder is absent and the running shell does not know the extension.
    NotInstalled,

    /// The packaged extension folder with a `metadata.json` exists, but the running shell has not
    /// loaded it. GNOME Shell only scans for extensions when a session starts.
    RequiresRelogin,

    /// The running shell refuses the extension because its `shell-version` does not match.
    OutOfDate,

    /// The running shell reports the extension in an error state.
    Errored,

    /// The extension is loaded but switched off.
    NotEnabled,

    /// The extension is loaded and switched on.
    Enabled,
}

/// Represents the result of getting the "state" parameter from the D-Bus `get_extension_info` API.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtensionState {
    /// The extension is not loaded.
    NotLoaded,

    /// The extension is loaded and enabled.
    Enabled,

    /// The extension is loaded and disabled.
    Disabled,

    /// The extension is loaded and in an error state.
    Errored,

    /// The extension does not support the running GNOME Shell version.
    OutOfDate,

    /// Transitional; not expected at rest.
    Downloading,

    /// Loaded by the shell, not switched on in this session.
    Initialized,

    /// Transitional; not expected at rest.
    Deactivating,

    /// Transitional; not expected at rest.
    Activating,
}

impl ExtensionState {
    fn from_u32(state: u32) -> Result<Self, ExtensionsError> {
        // This mapping is done just from observation, not sure if this is entirely correct in
        // practice.
        // Reference: https://gitlab.gnome.org/GNOME/gnome-shell/-/blob/main/js/misc/extensionUtils.js?ref_type=heads#L15
        match state {
            1 => Ok(ExtensionState::Enabled),
            2 => Ok(ExtensionState::Disabled),
            3 => Ok(ExtensionState::Errored),
            4 => Ok(ExtensionState::OutOfDate),
            5 => Ok(ExtensionState::Downloading),
            6 => Ok(ExtensionState::Initialized),
            7 => Ok(ExtensionState::Deactivating),
            8 => Ok(ExtensionState::Activating),
            _ => Err(ExtensionsError::UnknownState(state)),
        }
    }

    fn as_u32(&self) -> Option<u32> {
        match self {
            ExtensionState::NotLoaded => None,
            ExtensionState::Enabled => Some(1),
            ExtensionState::Disabled => Some(2),
            ExtensionState::Errored => Some(3),
            ExtensionState::OutOfDate => Some(4),
            ExtensionState::Downloading => Some(5),
            ExtensionState::Initialized => Some(6),
            ExtensionState::Deactivating => Some(7),
            ExtensionState::Activating => Some(8),
        }
    }
}

/// One of the two gsettings lists in `org.gnome.shell` that decide whether an extension is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExtensionList {
    Enabled,
    Disabled,
}

impl ExtensionList {
    fn key(&self) -> &'static str {
        match self {
            ExtensionList::Enabled => "enabled-extensions",
            ExtensionList::Disabled => "disabled-extensions",
        }
    }

    fn read_error(&self, reason: &str) -> ExtensionsError {
        ExtensionsError::Settings(format!("Could not read {GNOME_SHELL_SCHEMA} {}: {reason}", self.key()).into())
    }

    fn write_error(&self, reason: &str) -> ExtensionsError {
        ExtensionsError::Settings(format!("Could not write {GNOME_SHELL_SCHEMA} {}: {reason}", self.key()).into())
    }

    fn parse(&self, text: &str) -> Result<Vec<String>, ExtensionsError> {
        parse_strv(text).ok_or_else(|| self.read_error(&format!("unexpected value {}", text.trim())))
    }
}

async fn new_proxy() -> Result<ShellExtensionsProxy<'static>, CrateError> {
    Ok(ShellExtensionsProxy::new(session_bus().await?).await?)
}

/// Path to the extension folder installed, root-owned, by the ShellCue package.
pub fn system_extension_directory() -> PathBuf {
    PathBuf::from("/usr/share/gnome-shell/extensions").join(EXTENSION_UUID)
}

/// Per-user copy of the extension, which GNOME Shell loads in place of the packaged one.
pub fn user_extension_directory<Ctx: FsProvider + EnvProvider>(ctx: &Ctx) -> Result<PathBuf, ExtensionsError> {
    Ok(home_dir_ctx(ctx)?
        .join(".local/share/gnome-shell/extensions")
        .join(EXTENSION_UUID))
}

/// A copy of the extension other than the packaged one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForeignCopy {
    /// The running shell loaded the extension from this folder.
    Loaded(PathBuf),
    /// The shell has not loaded the extension and this [user_extension_directory] exists.
    Present(PathBuf),
}

/// Finds a [ForeignCopy]; the home folder counts only if the shell loaded no copy.
pub async fn foreign_extension_copy<Ctx, ExtensionsCtx>(
    ctx: &Ctx,
    shell_extensions: &ShellExtensions<ExtensionsCtx>,
) -> Result<Option<ForeignCopy>, ExtensionsError>
where
    Ctx: FsProvider + EnvProvider + SysInfoProvider,
    ExtensionsCtx: FsProvider + EnvProvider + SysInfoProvider,
{
    match shell_extensions.loaded_path().await? {
        Some(path) if path != system_extension_directory() => Ok(Some(ForeignCopy::Loaded(path))),
        Some(_) => Ok(None),
        None => Ok(user_extension_directory(ctx)
            .ok()
            .filter(|dir| ctx.fs().exists(dir))
            .map(ForeignCopy::Present)),
    }
}

/// Parses gsettings' GVariant text for a string array. Escapes are not supported, so any item
/// containing a quote or a backslash makes the whole text unparseable.
fn parse_strv(text: &str) -> Option<Vec<String>> {
    let text = text.trim();
    if text == "@as []" {
        return Some(Vec::new());
    }
    let inner = text.strip_prefix('[')?.strip_suffix(']')?.trim();
    if inner.is_empty() {
        return Some(Vec::new());
    }
    inner
        .split(',')
        .map(|item| {
            let item = item.trim();
            let quote = item.chars().next().filter(|c| *c == '\'' || *c == '"')?;
            let value = item.strip_prefix(quote)?.strip_suffix(quote)?;
            if value.contains(['\'', '"', '\\']) {
                return None;
            }
            Some(value.to_string())
        })
        .collect()
}

/// Formats a string array as GVariant text for gsettings; [Option::None] if an item contains a
/// quote or a backslash.
fn format_strv(items: &[String]) -> Option<String> {
    if items.iter().any(|item| item.contains(['\'', '"', '\\'])) {
        return None;
    }
    if items.is_empty() {
        return Some("@as []".to_string());
    }
    let quoted = items.iter().map(|item| format!("'{item}'")).collect::<Vec<_>>();
    Some(format!("[{}]", quoted.join(", ")))
}

/// Gets the installation status for the ShellCue GNOME Shell extension.
pub async fn get_extension_status<Ctx, ExtensionsCtx>(
    ctx: &Ctx,
    shell_extensions: &ShellExtensions<ExtensionsCtx>,
) -> Result<ExtensionInstallationStatus, ExtensionsError>
where
    Ctx: FsProvider + EnvProvider + SysInfoProvider,
    ExtensionsCtx: FsProvider + EnvProvider + SysInfoProvider,
{
    if !shell_extensions.is_gnome_shell_running().await? {
        return Ok(ExtensionInstallationStatus::GnomeShellNotRunning);
    }

    match shell_extensions.get_extension_state().await? {
        ExtensionState::Enabled => Ok(ExtensionInstallationStatus::Enabled),
        ExtensionState::Disabled | ExtensionState::Initialized => Ok(ExtensionInstallationStatus::NotEnabled),
        ExtensionState::OutOfDate => Ok(ExtensionInstallationStatus::OutOfDate),
        ExtensionState::NotLoaded => {
            if ctx.fs().exists(system_extension_directory().join("metadata.json")) {
                Ok(ExtensionInstallationStatus::RequiresRelogin)
            } else {
                Ok(ExtensionInstallationStatus::NotInstalled)
            }
        },
        ExtensionState::Errored => Ok(ExtensionInstallationStatus::Errored),
        // Transitional states are not expected at rest.
        _ => Ok(ExtensionInstallationStatus::Errored),
    }
}

/// Provides an accessible interface to retrieving info about the ShellCue GNOME Shell Extension.
///
/// Note that the real implementation uses the D-Bus API and `gsettings` for retrieving info and
/// performing actions. Therefore, in contexts where the D-Bus session bus cannot be accessed (e.g.
/// in Debian maintainer scripts), this struct should not be used.
#[derive(Debug)]
pub struct ShellExtensions<Ctx> {
    inner: inner::Inner<Ctx>,
}

mod inner {
    use std::sync::{
        Arc,
        Weak,
    };

    use tokio::sync::Mutex;

    use super::*;

    #[derive(Debug)]
    pub enum Inner<Ctx> {
        Real(Weak<Ctx>),
        Fake(Arc<Mutex<Fake<Ctx>>>),
    }

    #[derive(Debug)]
    pub struct Fake<Ctx> {
        pub(super) ctx: Weak<Ctx>,
        pub version: GnomeShellVersion,
        pub extension_info: HashMap<String, OwnedValue>,
        /// Raw gsettings text, as `gsettings get` prints it.
        pub enabled_extensions: String,
        /// Raw gsettings text, as `gsettings get` prints it.
        pub disabled_extensions: String,
    }

    impl<Ctx> Fake<Ctx> {
        pub fn extension_info(&self) -> HashMap<String, OwnedValue> {
            self.extension_info
                .iter()
                .map(|(k, v)| (k.clone(), v.try_clone().unwrap()))
                .collect::<_>()
        }
    }
}

impl<Ctx> ShellExtensions<Ctx>
where
    Ctx: FsProvider + EnvProvider + SysInfoProvider,
{
    /// Creates a new real GNOME Shell extension client.
    ///
    /// Takes a [`Weak`] pointer since this enables [`ShellExtensions`] to be embedded with a
    /// [`fig_os_shim::Context`].
    pub fn new(ctx: Weak<Ctx>) -> Self {
        Self {
            inner: inner::Inner::Real(ctx),
        }
    }

    /// Creates a new fake shell extension client, returning GNOME Shell v45, the extension as not
    /// known to the shell, and both gsettings lists empty.
    pub fn new_fake(ctx: Weak<Ctx>) -> Self {
        Self {
            inner: inner::Inner::Fake(Arc::new(Mutex::new(inner::Fake {
                ctx,
                version: GnomeShellVersion {
                    major: 45,
                    minor: "0".to_string(),
                },
                extension_info: HashMap::new(),
                enabled_extensions: "@as []".to_string(),
                disabled_extensions: "@as []".to_string(),
            }))),
        }
    }

    /// Returns the version of the system's GNOME Shell.
    pub async fn gnome_shell_version(&self) -> Result<GnomeShellVersion, ExtensionsError> {
        use inner::Inner;
        match &self.inner {
            Inner::Real(_) => new_proxy().await?.shell_version().await?.parse(),
            Inner::Fake(fake) => Ok(fake.lock().await.version.clone()),
        }
    }

    /// The D-Bus `GetExtensionInfo` map; empty when the running shell does not know the extension.
    pub async fn get_extension_info(&self) -> Result<HashMap<String, OwnedValue>, ExtensionsError> {
        use inner::Inner;
        match &self.inner {
            Inner::Real(_) => Ok(new_proxy().await?.get_extension_info(EXTENSION_UUID).await?),
            Inner::Fake(fake) => Ok(fake.lock().await.extension_info()),
        }
    }

    /// The folder the running shell loaded the extension from, if it loaded it.
    pub async fn loaded_path(&self) -> Result<Option<PathBuf>, ExtensionsError> {
        let mut info = self.get_extension_info().await?;
        Ok(info
            .remove("path")
            .and_then(|path| String::try_from(path).ok())
            .filter(|path| !path.is_empty())
            .map(PathBuf::from))
    }

    async fn is_gnome_shell_running(&self) -> Result<bool, ExtensionsError> {
        Ok(self.ctx().await?.sysinfo().is_process_running(GNOME_SHELL_PROCESS_NAME))
    }

    /// Gets the [ExtensionState] of the extension according to what we can determine from the
    /// D-Bus `get_extension_info` API.
    async fn get_extension_state(&self) -> Result<ExtensionState, ExtensionsError> {
        let mut info = self.get_extension_info().await?;
        if info.keys().count() == 0 {
            return Ok(ExtensionState::NotLoaded);
        }
        let state = f64::try_from(
            info.remove("state")
                .ok_or(ExtensionsError::Other("missing extension state".into()))?,
        )? as u32;
        ExtensionState::from_u32(state)
    }

    /// Asks the running shell over D-Bus to switch the extension on. Returns whether the shell
    /// reports that it did; `Ok(false)` when the shell does not know the extension yet.
    pub async fn enable_extension(&self) -> Result<bool, ExtensionsError> {
        use inner::Inner;
        match &self.inner {
            Inner::Real(_) => Ok(new_proxy().await?.enable_extension(EXTENSION_UUID).await?),
            Inner::Fake(fake) => match self.get_extension_state().await? {
                ExtensionState::Disabled | ExtensionState::Initialized => {
                    self.set_listed_enabled(true).await?;
                    fake.lock()
                        .await
                        .extension_info
                        .insert("state".to_string(), OwnedValue::from(1f64));
                    Ok(true)
                },
                _ => Ok(false),
            },
        }
    }

    /// Asks the running shell over D-Bus to switch the extension off. Returns whether the shell
    /// reports that it did; `Ok(false)` when the shell does not know the extension yet.
    pub async fn disable_extension(&self) -> Result<bool, ExtensionsError> {
        use inner::Inner;
        match &self.inner {
            Inner::Real(_) => Ok(new_proxy().await?.disable_extension(EXTENSION_UUID).await?),
            Inner::Fake(fake) => match self.get_extension_state().await? {
                ExtensionState::Enabled => {
                    self.set_listed_enabled(false).await?;
                    fake.lock()
                        .await
                        .extension_info
                        .insert("state".to_string(), OwnedValue::from(2f64));
                    Ok(true)
                },
                _ => Ok(false),
            },
        }
    }

    /// Whether the extension is in `enabled-extensions` and not in `disabled-extensions`, which
    /// takes precedence. This is what the shell reads when a session starts.
    pub async fn is_listed_enabled(&self) -> Result<bool, ExtensionsError> {
        let enabled = self.read_list(ExtensionList::Enabled).await?;
        if !enabled.iter().any(|uuid| uuid == EXTENSION_UUID) {
            return Ok(false);
        }
        let disabled = self.read_list(ExtensionList::Disabled).await?;
        Ok(!disabled.iter().any(|uuid| uuid == EXTENSION_UUID))
    }

    /// Adds the extension to `enabled-extensions` and removes it from `disabled-extensions` when
    /// `on`; removes it from `enabled-extensions` otherwise. Only a list that changes is written.
    pub async fn set_listed_enabled(&self, on: bool) -> Result<(), ExtensionsError> {
        let mut enabled = self.read_list(ExtensionList::Enabled).await?;
        let is_listed = enabled.iter().any(|uuid| uuid == EXTENSION_UUID);
        if on {
            let mut disabled = self.read_list(ExtensionList::Disabled).await?;
            if !is_listed {
                enabled.push(EXTENSION_UUID.to_string());
                self.write_list(ExtensionList::Enabled, &enabled).await?;
            }
            if disabled.iter().any(|uuid| uuid == EXTENSION_UUID) {
                disabled.retain(|uuid| uuid != EXTENSION_UUID);
                self.write_list(ExtensionList::Disabled, &disabled).await?;
            }
        } else if is_listed {
            enabled.retain(|uuid| uuid != EXTENSION_UUID);
            self.write_list(ExtensionList::Enabled, &enabled).await?;
        }
        Ok(())
    }

    async fn read_list(&self, list: ExtensionList) -> Result<Vec<String>, ExtensionsError> {
        use inner::Inner;
        match &self.inner {
            Inner::Real(_) => {
                let output = Command::new(GSETTINGS_PATH)
                    .args(["get", GNOME_SHELL_SCHEMA, list.key()])
                    .output()
                    .await
                    .map_err(|err| list.read_error(&err.to_string()))?;
                if !output.status.success() {
                    return Err(list.read_error(&format!("gsettings failed ({})", output.status)));
                }
                let text = String::from_utf8(output.stdout).map_err(|_| list.read_error("the value is not UTF-8"))?;
                list.parse(&text)
            },
            Inner::Fake(fake) => {
                let fake = fake.lock().await;
                list.parse(match list {
                    ExtensionList::Enabled => &fake.enabled_extensions,
                    ExtensionList::Disabled => &fake.disabled_extensions,
                })
            },
        }
    }

    async fn write_list(&self, list: ExtensionList, items: &[String]) -> Result<(), ExtensionsError> {
        use inner::Inner;
        let text = format_strv(items).ok_or_else(|| list.write_error("an item contains a quote or a backslash"))?;
        match &self.inner {
            Inner::Real(_) => {
                let output = Command::new(GSETTINGS_PATH)
                    .args(["set", GNOME_SHELL_SCHEMA, list.key(), text.as_str()])
                    .output()
                    .await
                    .map_err(|err| list.write_error(&err.to_string()))?;
                if !output.status.success() {
                    return Err(list.write_error(&format!("gsettings failed ({})", output.status)));
                }
                Ok(())
            },
            Inner::Fake(fake) => {
                let mut fake = fake.lock().await;
                match list {
                    ExtensionList::Enabled => fake.enabled_extensions = text,
                    ExtensionList::Disabled => fake.disabled_extensions = text,
                }
                Ok(())
            },
        }
    }

    async fn ctx(&self) -> Result<Arc<Ctx>, ExtensionsError> {
        use inner::Inner;
        match &self.inner {
            Inner::Real(ctx) => Ok(ctx.upgrade().ok_or(ExtensionsError::InvalidContext)?),
            Inner::Fake(fake) => Ok(fake.lock().await.ctx.upgrade().ok_or(ExtensionsError::InvalidContext)?),
        }
    }

    /// Test helper for the fake impl that writes the packaged extension files and, when `state` is
    /// [Option::Some], marks the extension as loaded from [system_extension_directory] in `state`.
    #[allow(dead_code)]
    pub async fn install_for_fake(&self, state: Option<ExtensionState>) -> Result<(), ExtensionsError> {
        if let inner::Inner::Fake(_) = &self.inner {
            self.write_fake_extension_to_fs().await?;
            if let Some(state) = state {
                self.load_for_fake(state, &system_extension_directory()).await;
            }
        }
        Ok(())
    }

    /// Test helper for the fake impl that marks the extension as loaded from `path` in `state`.
    #[allow(dead_code)]
    pub async fn load_for_fake(&self, state: ExtensionState, path: &Path) {
        if let inner::Inner::Fake(fake) = &self.inner {
            let state = state.as_u32().expect("a loaded extension has a state") as f64;
            let path = path.to_string_lossy().into_owned();
            fake.lock().await.extension_info = [
                ("state".to_string(), OwnedValue::from(state)),
                ("path".to_string(), OwnedValue::try_from(Value::from(path.as_str())).unwrap()),
            ]
            .into_iter()
            .collect();
        }
    }

    /// Test helper that writes `metadata.json` under [system_extension_directory].
    #[allow(dead_code)]
    async fn write_fake_extension_to_fs(&self) -> Result<(), ExtensionsError> {
        if let inner::Inner::Fake(_) = &self.inner {
            let ctx = self.ctx().await?;
            let extension_dir_path = system_extension_directory();
            ctx.fs().create_dir_all(&extension_dir_path).await.ok();
            ctx.fs()
                .write(
                    extension_dir_path.join("metadata.json"),
                    format!("{{\"uuid\": \"{EXTENSION_UUID}\"}}"),
                )
                .await
                .unwrap();
        }
        Ok(())
    }
}

#[derive(Debug, Error)]
pub enum ExtensionsError {
    #[error(transparent)]
    CrateError(#[from] CrateError),
    #[error(transparent)]
    StdIo(#[from] std::io::Error),
    #[error(transparent)]
    Zbus(#[from] zbus::Error),
    #[error(transparent)]
    ZVariant(#[from] zbus::zvariant::Error),
    #[error("Invalid major version: {0:?}")]
    InvalidMajorVersion(#[from] std::num::ParseIntError),
    #[error(transparent)]
    DirectoryError(#[from] DirectoryError),
    #[error("Invalid Context reference")]
    InvalidContext,
    #[error("Unknown state: {0}")]
    UnknownState(u32),
    #[error("{0}")]
    Settings(Cow<'static, str>),
    #[error("Error: {0:?}")]
    Other(Cow<'static, str>),
}

#[proxy(
    default_service = "org.gnome.Shell.Extensions",
    interface = "org.gnome.Shell.Extensions",
    default_path = "/org/gnome/Shell/Extensions"
)]
trait ShellExtensions {
    #[zbus(property)]
    fn shell_version(&self) -> zbus::Result<String>;

    /// GetExtensionInfo method
    fn get_extension_info(&self, uuid: &str) -> zbus::Result<HashMap<String, OwnedValue>>;

    /// EnableExtension method
    fn enable_extension(&self, uuid: &str) -> zbus::Result<bool>;

    /// DisableExtension method
    fn disable_extension(&self, uuid: &str) -> zbus::Result<bool>;
}

/// Represents a version of the system's GNOME Shell.
///
/// GNOME Shell versioning scheme taken from this post: <https://discourse.gnome.org/t/new-gnome-versioning-scheme/4235>
#[derive(Debug, Clone)]
pub struct GnomeShellVersion {
    pub major: u32,
    pub minor: String,
}

impl FromStr for GnomeShellVersion {
    type Err = ExtensionsError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut split = s.split(".");
        let major = split.next().unwrap();
        let minor = split.next().unwrap_or_default();
        Ok(Self {
            major: major
                .to_string()
                .parse()
                .map_err(ExtensionsError::InvalidMajorVersion)?,
            minor: minor.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
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

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| item.to_string()).collect()
    }

    async fn fake_list_texts(shell_extensions: &ShellExtensions<Context>) -> (String, String) {
        match &shell_extensions.inner {
            inner::Inner::Fake(fake) => {
                let fake = fake.lock().await;
                (fake.enabled_extensions.clone(), fake.disabled_extensions.clone())
            },
            inner::Inner::Real(_) => panic!("expected the fake"),
        }
    }

    async fn fake_lists(shell_extensions: &ShellExtensions<Context>) -> (Vec<String>, Vec<String>) {
        let (enabled, disabled) = fake_list_texts(shell_extensions).await;
        (parse_strv(&enabled).unwrap(), parse_strv(&disabled).unwrap())
    }

    async fn set_fake_list_text(shell_extensions: &ShellExtensions<Context>, list: ExtensionList, text: &str) {
        if let inner::Inner::Fake(fake) = &shell_extensions.inner {
            let mut fake = fake.lock().await;
            match list {
                ExtensionList::Enabled => fake.enabled_extensions = text.to_string(),
                ExtensionList::Disabled => fake.disabled_extensions = text.to_string(),
            }
        }
    }

    async fn set_fake_lists(shell_extensions: &ShellExtensions<Context>, enabled: &[&str], disabled: &[&str]) {
        let enabled = format_strv(&strings(enabled)).unwrap();
        let disabled = format_strv(&strings(disabled)).unwrap();
        set_fake_list_text(shell_extensions, ExtensionList::Enabled, &enabled).await;
        set_fake_list_text(shell_extensions, ExtensionList::Disabled, &disabled).await;
    }

    mod extension_status_tests {
        use super::*;

        #[tokio::test]
        async fn test_extension_status_when_gnome_shell_not_running() {
            let ctx = Context::builder().with_test_home().await.unwrap().build_fake();
            let shell_extensions = ShellExtensions::new_fake(Arc::downgrade(&ctx));

            let status = get_extension_status(&ctx, &shell_extensions).await.unwrap();

            assert_eq!(status, ExtensionInstallationStatus::GnomeShellNotRunning);
        }

        #[tokio::test]
        async fn test_extension_status_when_not_packaged_and_not_loaded() {
            let ctx = make_ctx().await;
            let shell_extensions = ShellExtensions::new_fake(Arc::downgrade(&ctx));

            let status = get_extension_status(&ctx, &shell_extensions).await.unwrap();

            assert_eq!(status, ExtensionInstallationStatus::NotInstalled);
        }

        #[tokio::test]
        async fn test_extension_status_when_packaged_but_not_loaded() {
            let ctx = make_ctx().await;
            let shell_extensions = ShellExtensions::new_fake(Arc::downgrade(&ctx));
            shell_extensions.install_for_fake(None).await.unwrap();

            let status = get_extension_status(&ctx, &shell_extensions).await.unwrap();

            assert_eq!(status, ExtensionInstallationStatus::RequiresRelogin);
        }

        #[tokio::test]
        async fn test_extension_status_when_out_of_date() {
            let ctx = make_ctx().await;
            let shell_extensions = ShellExtensions::new_fake(Arc::downgrade(&ctx));
            shell_extensions
                .install_for_fake(Some(ExtensionState::OutOfDate))
                .await
                .unwrap();

            let status = get_extension_status(&ctx, &shell_extensions).await.unwrap();

            assert_eq!(status, ExtensionInstallationStatus::OutOfDate);
        }

        #[tokio::test]
        async fn test_extension_status_when_errored() {
            for state in &[ExtensionState::Errored, ExtensionState::Downloading, ExtensionState::Activating] {
                let ctx = make_ctx().await;
                let shell_extensions = ShellExtensions::new_fake(Arc::downgrade(&ctx));
                shell_extensions.install_for_fake(Some(*state)).await.unwrap();

                let status = get_extension_status(&ctx, &shell_extensions).await.unwrap();

                assert_eq!(
                    status,
                    ExtensionInstallationStatus::Errored,
                    "Extension with state {:?} should return Errored",
                    *state
                );
            }
        }

        #[tokio::test]
        async fn test_extension_status_when_not_enabled() {
            for disabled_state in &[ExtensionState::Disabled, ExtensionState::Initialized] {
                let ctx = make_ctx().await;
                let shell_extensions = ShellExtensions::new_fake(Arc::downgrade(&ctx));
                shell_extensions.install_for_fake(Some(*disabled_state)).await.unwrap();

                let status = get_extension_status(&ctx, &shell_extensions).await.unwrap();

                assert_eq!(
                    status,
                    ExtensionInstallationStatus::NotEnabled,
                    "Extension with state {:?} should return NotEnabled",
                    *disabled_state
                );
            }
        }

        #[tokio::test]
        async fn test_extension_status_when_enabled() {
            let ctx = make_ctx().await;
            let shell_extensions = ShellExtensions::new_fake(Arc::downgrade(&ctx));
            shell_extensions
                .install_for_fake(Some(ExtensionState::Enabled))
                .await
                .unwrap();

            let status = get_extension_status(&ctx, &shell_extensions).await.unwrap();

            assert_eq!(status, ExtensionInstallationStatus::Enabled);
        }
    }

    mod strv_tests {
        use super::*;

        #[test]
        fn test_parse_strv_empty_forms() {
            assert_eq!(parse_strv("@as []"), Some(vec![]));
            assert_eq!(parse_strv("@as []\n"), Some(vec![]));
            assert_eq!(parse_strv("[]"), Some(vec![]));
            assert_eq!(parse_strv(" [ ] "), Some(vec![]));
        }

        #[test]
        fn test_parse_strv_items() {
            assert_eq!(parse_strv("['a@b']\n"), Some(strings(&["a@b"])));
            assert_eq!(parse_strv("['a@b', 'c@d']"), Some(strings(&["a@b", "c@d"])));
            assert_eq!(parse_strv(r#"["a@b", 'c@d']"#), Some(strings(&["a@b", "c@d"])));
        }

        #[test]
        fn test_parse_strv_rejects() {
            assert_eq!(parse_strv("'a@b', 'c@d']"), None);
            assert_eq!(parse_strv("['a@b', 'c@d'"), None);
            assert_eq!(parse_strv("['a@b, 'c@d']"), None);
            assert_eq!(parse_strv("['a@b\"]"), None);
            assert_eq!(parse_strv(r"['a\b']"), None);
            assert_eq!(parse_strv(r#"['a"b']"#), None);
            assert_eq!(parse_strv("['a@b'] extra"), None);
            assert_eq!(parse_strv("['a@b',]"), None);
            assert_eq!(parse_strv("[a@b]"), None);
            assert_eq!(parse_strv(""), None);
        }

        #[test]
        fn test_format_strv() {
            assert_eq!(format_strv(&[]), Some("@as []".to_string()));
            assert_eq!(format_strv(&strings(&["a@b"])), Some("['a@b']".to_string()));
            assert_eq!(format_strv(&strings(&["a@b", "c@d"])), Some("['a@b', 'c@d']".to_string()));
            assert_eq!(format_strv(&strings(&["a'b"])), None);
            assert_eq!(format_strv(&strings(&["a\"b"])), None);
            assert_eq!(format_strv(&strings(&[r"a\b"])), None);
        }

        #[test]
        fn test_strv_round_trip() {
            for items in [strings(&[]), strings(&["a@b"]), strings(&["a@b", "c@d", EXTENSION_UUID])] {
                assert_eq!(parse_strv(&format_strv(&items).unwrap()), Some(items));
            }
        }
    }

    mod listed_tests {
        use super::*;

        #[tokio::test]
        async fn test_set_listed_enabled_on_adds_once() {
            let ctx = make_ctx().await;
            let shell_extensions = ShellExtensions::new_fake(Arc::downgrade(&ctx));
            set_fake_lists(&shell_extensions, &["other@x"], &[]).await;

            shell_extensions.set_listed_enabled(true).await.unwrap();
            shell_extensions.set_listed_enabled(true).await.unwrap();

            let (enabled, disabled) = fake_lists(&shell_extensions).await;
            assert_eq!(enabled, strings(&["other@x", EXTENSION_UUID]));
            assert!(disabled.is_empty());
            assert!(shell_extensions.is_listed_enabled().await.unwrap());
        }

        #[tokio::test]
        async fn test_set_listed_enabled_on_removes_from_disabled() {
            let ctx = make_ctx().await;
            let shell_extensions = ShellExtensions::new_fake(Arc::downgrade(&ctx));
            set_fake_lists(&shell_extensions, &[EXTENSION_UUID], &["other@x", EXTENSION_UUID]).await;

            shell_extensions.set_listed_enabled(true).await.unwrap();

            let (enabled, disabled) = fake_lists(&shell_extensions).await;
            assert_eq!(enabled, strings(&[EXTENSION_UUID]));
            assert_eq!(disabled, strings(&["other@x"]));
        }

        #[tokio::test]
        async fn test_set_listed_enabled_off_removes() {
            let ctx = make_ctx().await;
            let shell_extensions = ShellExtensions::new_fake(Arc::downgrade(&ctx));
            set_fake_lists(&shell_extensions, &["other@x", EXTENSION_UUID], &[]).await;

            shell_extensions.set_listed_enabled(false).await.unwrap();

            let (enabled, disabled) = fake_lists(&shell_extensions).await;
            assert_eq!(enabled, strings(&["other@x"]));
            assert!(disabled.is_empty());
            assert!(!shell_extensions.is_listed_enabled().await.unwrap());
        }

        #[tokio::test]
        async fn test_is_listed_enabled_false_when_also_disabled() {
            let ctx = make_ctx().await;
            let shell_extensions = ShellExtensions::new_fake(Arc::downgrade(&ctx));
            assert!(!shell_extensions.is_listed_enabled().await.unwrap());

            set_fake_lists(&shell_extensions, &[EXTENSION_UUID], &[EXTENSION_UUID]).await;

            assert!(!shell_extensions.is_listed_enabled().await.unwrap());
        }

        #[tokio::test]
        async fn test_unparseable_enabled_list_changes_nothing() {
            let disabled = format!("['{EXTENSION_UUID}']");
            for text in ["['a', b]", r"['a\b']"] {
                let ctx = make_ctx().await;
                let shell_extensions = ShellExtensions::new_fake(Arc::downgrade(&ctx));
                set_fake_list_text(&shell_extensions, ExtensionList::Enabled, text).await;
                set_fake_list_text(&shell_extensions, ExtensionList::Disabled, &disabled).await;
                let before = fake_list_texts(&shell_extensions).await;

                assert!(shell_extensions.is_listed_enabled().await.is_err(), "{text}");
                assert!(shell_extensions.set_listed_enabled(true).await.is_err(), "{text}");
                assert_eq!(fake_list_texts(&shell_extensions).await, before);
                assert!(shell_extensions.set_listed_enabled(false).await.is_err(), "{text}");
                assert_eq!(fake_list_texts(&shell_extensions).await, before);
            }
        }

        #[tokio::test]
        async fn test_unparseable_disabled_list_blocks_switching_on() {
            let ctx = make_ctx().await;
            let shell_extensions = ShellExtensions::new_fake(Arc::downgrade(&ctx));
            set_fake_list_text(&shell_extensions, ExtensionList::Enabled, "['other@x']").await;
            set_fake_list_text(&shell_extensions, ExtensionList::Disabled, "['a', b]").await;

            assert!(shell_extensions.set_listed_enabled(true).await.is_err());
            assert_eq!(fake_list_texts(&shell_extensions).await.0, "['other@x']");
        }
    }

    #[tokio::test]
    async fn test_fake_enable_and_disable_update_lists() {
        let ctx = make_ctx().await;
        let shell_extensions = ShellExtensions::new_fake(Arc::downgrade(&ctx));
        shell_extensions
            .install_for_fake(Some(ExtensionState::Disabled))
            .await
            .unwrap();
        set_fake_lists(&shell_extensions, &["other@x"], &[EXTENSION_UUID]).await;

        assert!(shell_extensions.enable_extension().await.unwrap());
        let (enabled, disabled) = fake_lists(&shell_extensions).await;
        assert_eq!(enabled, strings(&["other@x", EXTENSION_UUID]));
        assert!(disabled.is_empty());

        assert!(shell_extensions.disable_extension().await.unwrap());
        let (enabled, disabled) = fake_lists(&shell_extensions).await;
        assert_eq!(enabled, strings(&["other@x"]));
        assert!(disabled.is_empty());
    }

    #[test]
    fn test_extension_state_from_u32() {
        assert_eq!(ExtensionState::from_u32(1).unwrap(), ExtensionState::Enabled);
        assert_eq!(ExtensionState::from_u32(2).unwrap(), ExtensionState::Disabled);
        assert_eq!(ExtensionState::from_u32(3).unwrap(), ExtensionState::Errored);
        assert_eq!(ExtensionState::from_u32(4).unwrap(), ExtensionState::OutOfDate);
        assert_eq!(ExtensionState::from_u32(6).unwrap(), ExtensionState::Initialized);
        assert!(matches!(ExtensionState::from_u32(99), Err(ExtensionsError::UnknownState(99))));
    }

    #[tokio::test]
    async fn test_fake_enable_and_disable() {
        let ctx = make_ctx().await;
        let shell_extensions = ShellExtensions::new_fake(Arc::downgrade(&ctx));
        assert!(!shell_extensions.enable_extension().await.unwrap());
        assert!(!shell_extensions.disable_extension().await.unwrap());

        shell_extensions
            .install_for_fake(Some(ExtensionState::Disabled))
            .await
            .unwrap();
        assert!(!shell_extensions.disable_extension().await.unwrap());
        assert!(shell_extensions.enable_extension().await.unwrap());
        assert_eq!(
            get_extension_status(&ctx, &shell_extensions).await.unwrap(),
            ExtensionInstallationStatus::Enabled
        );
        assert!(!shell_extensions.enable_extension().await.unwrap());
        assert!(shell_extensions.disable_extension().await.unwrap());
        assert_eq!(
            get_extension_status(&ctx, &shell_extensions).await.unwrap(),
            ExtensionInstallationStatus::NotEnabled
        );
    }

    #[tokio::test]
    async fn test_loaded_path() {
        let ctx = make_ctx().await;
        let shell_extensions = ShellExtensions::new_fake(Arc::downgrade(&ctx));
        assert_eq!(shell_extensions.loaded_path().await.unwrap(), None);

        shell_extensions
            .install_for_fake(Some(ExtensionState::Enabled))
            .await
            .unwrap();
        assert_eq!(
            shell_extensions.loaded_path().await.unwrap(),
            Some(system_extension_directory())
        );

        let elsewhere = PathBuf::from("/home/testuser/.local/share/gnome-shell/extensions").join(EXTENSION_UUID);
        shell_extensions.load_for_fake(ExtensionState::Enabled, &elsewhere).await;
        assert_eq!(shell_extensions.loaded_path().await.unwrap(), Some(elsewhere));
    }

    #[tokio::test]
    async fn test_foreign_extension_copy() {
        let ctx = make_ctx().await;
        let shell_extensions = ShellExtensions::new_fake(Arc::downgrade(&ctx));
        shell_extensions.install_for_fake(None).await.unwrap();
        assert_eq!(foreign_extension_copy(&ctx, &shell_extensions).await.unwrap(), None);

        let user_dir = user_extension_directory(&ctx).unwrap();
        ctx.fs().create_dir_all(&user_dir).await.unwrap();
        assert_eq!(
            foreign_extension_copy(&ctx, &shell_extensions).await.unwrap(),
            Some(ForeignCopy::Present(user_dir.clone()))
        );

        shell_extensions.load_for_fake(ExtensionState::Enabled, &user_dir).await;
        assert_eq!(
            foreign_extension_copy(&ctx, &shell_extensions).await.unwrap(),
            Some(ForeignCopy::Loaded(user_dir))
        );
    }

    #[tokio::test]
    async fn test_foreign_extension_copy_ignores_home_folder_when_packaged_one_is_loaded() {
        for state in [
            ExtensionState::Enabled,
            ExtensionState::Disabled,
            ExtensionState::Initialized,
            ExtensionState::Errored,
            ExtensionState::OutOfDate,
        ] {
            let ctx = make_ctx().await;
            let shell_extensions = ShellExtensions::new_fake(Arc::downgrade(&ctx));
            shell_extensions.install_for_fake(Some(state)).await.unwrap();
            ctx.fs()
                .create_dir_all(user_extension_directory(&ctx).unwrap())
                .await
                .unwrap();

            assert_eq!(
                foreign_extension_copy(&ctx, &shell_extensions).await.unwrap(),
                None,
                "{state:?}"
            );
        }
    }

    #[test]
    fn test_gnome_shell_version_parse() {
        let version: GnomeShellVersion = "46.2".parse().unwrap();
        assert_eq!(version.major, 46);
        assert_eq!(version.minor, "2");

        let version: GnomeShellVersion = "46".parse().unwrap();
        assert_eq!(version.major, 46);
        assert_eq!(version.minor, "");

        let version: GnomeShellVersion = "47.rc".parse().unwrap();
        assert_eq!(version.major, 47);
        assert_eq!(version.minor, "rc");

        assert!("".parse::<GnomeShellVersion>().is_err());
    }

    #[test]
    fn test_system_extension_directory() {
        assert_eq!(
            system_extension_directory(),
            PathBuf::from("/usr/share/gnome-shell/extensions/shellcue-gnome-integration@bandisch-apps.com")
        );
    }

    mod e2e {
        use super::*;

        #[tokio::test]
        #[ignore = "not in ci"]
        async fn test_gnome_shell_version() {
            let ctx = Context::new();
            println!(
                "{:?}",
                ShellExtensions::new(Arc::downgrade(&ctx))
                    .gnome_shell_version()
                    .await
                    .unwrap()
            );
        }

        #[tokio::test]
        #[ignore = "not in ci"]
        async fn test_get_extension_info() {
            let info = new_proxy()
                .await
                .unwrap()
                .get_extension_info(EXTENSION_UUID)
                .await
                .unwrap();
            println!("{:?}", info);
        }

        #[tokio::test]
        #[ignore = "not in ci"]
        async fn test_get_installed_extension_status() {
            let ctx = Context::new();
            let shell_extensions = ShellExtensions::new(Arc::downgrade(&ctx));
            let status = get_extension_status(&ctx, &shell_extensions).await.unwrap();
            println!("{:?}", status);
        }

        #[tokio::test]
        #[ignore = "not in ci"]
        async fn test_is_listed_enabled() {
            let ctx = Context::new();
            let res = ShellExtensions::new(Arc::downgrade(&ctx)).is_listed_enabled().await;
            println!("{:?}", res);
        }

        #[tokio::test]
        #[ignore = "not in ci"]
        async fn test_enable_extension() {
            let ctx = Context::new();
            let res = ShellExtensions::new(Arc::downgrade(&ctx)).enable_extension().await;
            println!("{:?}", res);
        }
    }
}
