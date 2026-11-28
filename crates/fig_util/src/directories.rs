use std::convert::TryInto;
use std::fmt::Display;
use std::path::PathBuf;

use camino::Utf8PathBuf;
use fig_os_shim::{
    Context,
    EnvProvider,
    FsProvider,
    Os,
    PlatformProvider,
    Shim,
};
use thiserror::Error;
use time::OffsetDateTime;

#[cfg(unix)]
use crate::RUNTIME_DIR_NAME;
use crate::linux::PACKAGE_NAME;
use crate::{
    BACKUP_DIR_NAME,
    DATA_DIR_NAME,
    TAURI_PRODUCT_NAME,
};

macro_rules! utf8_dir {
    ($name:ident, $($arg:ident: $type:ty),*) => {
        paste::paste! {
            pub fn [<$name _utf8>]($($arg: $type),*) -> Result<Utf8PathBuf> {
                Ok($name($($arg),*)?.try_into()?)
            }
        }
    };
    ($name:ident) => {
        utf8_dir!($name,);
    };
}

#[derive(Debug, Error)]
pub enum DirectoryError {
    #[error("home directory not found")]
    NoHomeDirectory,
    #[error("runtime directory not found: neither XDG_RUNTIME_DIR nor TMPDIR were found")]
    NoRuntimeDirectory,
    #[error("non absolute path: {0:?}")]
    NonAbsolutePath(PathBuf),
    #[error("unsupported platform: {0:?}")]
    UnsupportedOs(Os),
    #[error("IO Error: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    TimeFormat(#[from] time::error::Format),
    #[error(transparent)]
    Utf8FromPath(#[from] camino::FromPathError),
    #[error(transparent)]
    Utf8FromPathBuf(#[from] camino::FromPathBufError),
    #[error(transparent)]
    FromVecWithNul(#[from] std::ffi::FromVecWithNulError),
    #[error(transparent)]
    IntoString(#[from] std::ffi::IntoStringError),
    #[error("must be ran from an appimage executable")]
    NotAppImage,
}

type Result<T, E = DirectoryError> = std::result::Result<T, E>;

/// The directory of the users home
///
/// - Linux: /home/Alice
/// - MacOS: /Users/Alice
pub fn home_dir() -> Result<PathBuf> {
    dirs::home_dir().ok_or(DirectoryError::NoHomeDirectory)
}

pub fn home_dir_ctx<Ctx: FsProvider + EnvProvider>(ctx: &Ctx) -> Result<PathBuf> {
    if ctx.env().is_real() {
        home_dir()
    } else {
        ctx.env()
            .get("HOME")
            .map_err(|_err| DirectoryError::NoHomeDirectory)
            .and_then(|h| {
                if h.is_empty() {
                    Err(DirectoryError::NoHomeDirectory)
                } else {
                    Ok(h)
                }
            })
            .map(PathBuf::from)
            .map(|p| ctx.fs().chroot_path(p))
    }
}

/// The directory of the users `$HOME/.local/bin` directory
///
/// MacOS and Linux path: `$HOME/.local/bin``
#[cfg(unix)]
pub fn home_local_bin() -> Result<PathBuf> {
    let mut path = home_dir()?;
    path.push(".local/bin");
    Ok(path)
}

#[cfg(unix)]
pub fn home_local_bin_ctx(ctx: &Context) -> Result<PathBuf> {
    let mut path = home_dir_ctx(ctx)?;
    path.push(".local/bin");
    Ok(path)
}

/// The ShellCue data directory
///
/// - Linux: `$XDG_DATA_HOME/{data_dir}` or `$HOME/.local/share/{data_dir}`
/// - MacOS: `$HOME/Library/Application Support/{data_dir}`
pub fn fig_data_dir() -> Result<PathBuf> {
    Ok(dirs::data_local_dir()
        .ok_or(DirectoryError::NoHomeDirectory)?
        .join(DATA_DIR_NAME))
}

pub fn fig_data_dir_ctx(fs: &impl FsProvider) -> Result<PathBuf> {
    Ok(fs.fs().chroot_path(fig_data_dir()?))
}

/// The user's local data directory.
///
/// - Linux: `$XDG_DATA_HOME` or `$HOME/.local/share`
/// - MacOS: `$HOME/Library/Application Support`
pub fn local_data_dir<Ctx: FsProvider + EnvProvider + PlatformProvider>(ctx: &Ctx) -> Result<PathBuf> {
    let env = ctx.env();
    match ctx.platform().os() {
        Os::Linux => {
            if let Some(path) = env.get_os("XDG_DATA_HOME") {
                return Ok(path.into());
            }
            Ok(home_dir_ctx(ctx)?.join(".local/share"))
        },
        Os::Mac => Ok(home_dir_ctx(ctx)?.join("Library/Application Support")),
        os => Err(DirectoryError::UnsupportedOs(os)),
    }
}

/// The ShellCue cache directory
///
/// - Linux: `$XDG_CACHE_HOME/{data_dir}` or `$HOME/.cache/{data_dir}`
/// - MacOS: `$HOME/Library/Caches/{data_dir}`
pub fn cache_dir() -> Result<PathBuf> {
    Ok(dirs::cache_dir()
        .ok_or(DirectoryError::NoHomeDirectory)?
        .join(DATA_DIR_NAME))
}

/// Get the macos tempdir from the `confstr` function
///
/// See: <https://man7.org/linux/man-pages/man3/confstr.3.html>
#[cfg(target_os = "macos")]
fn macos_tempdir() -> Result<PathBuf> {
    let len = unsafe { libc::confstr(libc::_CS_DARWIN_USER_TEMP_DIR, std::ptr::null::<i8>().cast_mut(), 0) };
    let mut buf: Vec<u8> = vec![0; len];
    unsafe { libc::confstr(libc::_CS_DARWIN_USER_TEMP_DIR, buf.as_mut_ptr().cast(), buf.len()) };
    let c_string = std::ffi::CString::from_vec_with_nul(buf)?;
    let str = c_string.into_string()?;
    Ok(PathBuf::from(str))
}

/// Runtime dir is used for runtime data that should not be persisted for a long time, e.g. socket
/// files and logs
///
/// The XDG_RUNTIME_DIR is set by systemd <https://www.freedesktop.org/software/systemd/man/latest/file-hierarchy.html#/run/user/>,
/// if this is not set such as on macOS it will fallback to TMPDIR which is secure on macOS
pub fn runtime_dir() -> Result<PathBuf> {
    let mut dir = dirs::runtime_dir();
    dir = dir.or_else(|| std::env::var_os("TMPDIR").map(PathBuf::from));

    cfg_if::cfg_if! {
        if #[cfg(target_os = "macos")] {
            let macos_tempdir = macos_tempdir()?;
            dir = dir.or(Some(macos_tempdir));
        } else {
            dir = dir.or_else(|| Some(std::env::temp_dir()));
        }
    }

    dir.ok_or(DirectoryError::NoRuntimeDirectory)
}

/// The sockets directory of the local ShellCue installation
///
/// - Linux: $XDG_RUNTIME_DIR/shellcue
/// - MacOS: $TMPDIR/shellcue
pub fn sockets_dir() -> Result<PathBuf> {
    Ok(runtime_dir()?.join(RUNTIME_DIR_NAME))
}

/// The directory on the host machine where socket files are stored
///
/// Returns the same as `sockets_dir`
///
/// - Linux: $XDG_RUNTIME_DIR/shellcue
/// - MacOS: $TMPDIR/shellcue
pub fn host_sockets_dir() -> Result<PathBuf> {
    sockets_dir()
}

/// The path to all of the themes
pub fn themes_dir(ctx: &Context) -> Result<PathBuf> {
    Ok(resources_path_ctx(ctx)?.join("themes"))
}

/// The autocomplete directory
pub fn autocomplete_dir() -> Result<PathBuf> {
    Ok(fig_data_dir()?.join("autocomplete"))
}

/// The autocomplete specs directory
pub fn autocomplete_specs_dir() -> Result<PathBuf> {
    Ok(autocomplete_dir()?.join("specs"))
}

/// The directory to all the ShellCue logs
///
/// - Linux: `$XDG_RUNTIME_DIR/shellcue/logs`
/// - MacOS: `$TMPDIR/shellcue/logs`
pub fn logs_dir() -> Result<PathBuf> {
    Ok(sockets_dir()?.join("logs"))
}

/// The directory where ShellCue places all data-sensitive backups
///
/// - Linux/MacOS: `$HOME/{backup_dir}`
pub fn backups_dir() -> Result<PathBuf> {
    Ok(home_dir()?.join(BACKUP_DIR_NAME))
}

/// The directory for time based data-sensitive backups
///
/// NOTE: This changes every second and cannot be cached
pub fn utc_backup_dir() -> Result<PathBuf> {
    let now = OffsetDateTime::now_utc().format(time::macros::format_description!(
        "[year]-[month]-[day]_[hour]-[minute]-[second]"
    ))?;

    Ok(backups_dir()?.join(now))
}

/// The desktop app socket path
///
/// - MacOS: `$TMPDIR/shellcue/desktop.sock`
/// - Linux: `$XDG_RUNTIME_DIR/shellcue/desktop.sock`
pub fn desktop_socket_path() -> Result<PathBuf> {
    Ok(host_sockets_dir()?.join("desktop.sock"))
}

/// The path to local remote socket
///
/// - MacOS: `$TMPDIR/shellcue/remote.sock`
/// - Linux: `$XDG_RUNTIME_DIR/shellcue/remote.sock`
pub fn local_remote_socket_path() -> Result<PathBuf> {
    Ok(host_sockets_dir()?.join("remote.sock"))
}

/// Get path to a figterm socket
///
/// - MacOS: `$TMPDIR/shellcue/t/$SESSION_ID.sock`
/// - Linux: `$XDG_RUNTIME_DIR/shellcue/t/$SESSION_ID.sock`
pub fn figterm_socket_path(session_id: impl Display) -> Result<PathBuf> {
    Ok(sockets_dir()?.join("t").join(format!("{session_id}.sock")))
}

/// The path to the resources directory
///
/// - MacOS: "/Applications/{app_name}.app/Contents/Resources"
/// - Linux: "/usr/share/{package_name}"
pub fn resources_path() -> Result<PathBuf> {
    cfg_if::cfg_if! {
        if #[cfg(all(unix, not(target_os = "macos")))] {
            Ok(std::path::Path::new("/usr/share").join(PACKAGE_NAME))
        } else if #[cfg(target_os = "macos")] {
            Ok(crate::app_bundle_path().join(crate::macos::BUNDLE_CONTENTS_RESOURCE_PATH))
        }
    }
}

pub fn resources_path_ctx<Ctx: EnvProvider + PlatformProvider>(ctx: &Ctx) -> Result<PathBuf> {
    let os = ctx.platform().os();
    match os {
        fig_os_shim::Os::Mac => Ok(crate::app_bundle_path().join(crate::macos::BUNDLE_CONTENTS_RESOURCE_PATH)),
        fig_os_shim::Os::Linux => {
            if ctx.env().in_appimage() {
                Ok(ctx
                    .env()
                    .current_dir()?
                    .join(format!("lib/{}", TAURI_PRODUCT_NAME.replace("_", "-"))))
            } else {
                Ok(format!("/usr/share/{}", PACKAGE_NAME).into())
            }
        },
        _ => Err(DirectoryError::UnsupportedOs(os)),
    }
}

/// The path to the ShellCue install manifest
///
/// - MacOS: "/Applications/{app_name}.app/Contents/Resources/manifest.json"
/// - Linux: "/usr/share/{package_name}/manifest.json"
pub fn manifest_path() -> Result<PathBuf> {
    Ok(resources_path()?.join("manifest.json"))
}

/// The directory that holds the ShellCue settings file
///
/// - Linux/MacOS: `$HOME/.config/{data_dir}` (`XDG_CONFIG_HOME` is not consulted)
pub fn settings_dir() -> Result<PathBuf> {
    Ok(home_dir()?.join(".config").join(DATA_DIR_NAME))
}

/// The path to the ShellCue settings file
///
/// - Linux/MacOS: `$HOME/.config/{data_dir}/settings.json` (`XDG_CONFIG_HOME` is not consulted)
pub fn settings_path() -> Result<PathBuf> {
    Ok(settings_dir()?.join("settings.json"))
}

/// The path to the desktop entry bundled with the AppImage.
///
/// Only applicable to the desktop app binary when ran as an AppImage.
pub fn appimage_desktop_entry_path<Ctx: EnvProvider>(ctx: &Ctx) -> Result<PathBuf> {
    if !ctx.env().in_appimage() {
        return Err(DirectoryError::NotAppImage);
    }
    Ok(ctx.env().current_dir()?.join("share/applications/shellcue-desktop.desktop"))
}

/// The path to the icon bundled with the AppImage to be used for the desktop entry file.
///
/// Only applicable to the desktop app binary when ran as an AppImage.
pub fn appimage_desktop_entry_icon_path<Ctx: EnvProvider>(ctx: &Ctx) -> Result<PathBuf> {
    if !ctx.env().in_appimage() {
        return Err(DirectoryError::NotAppImage);
    }
    Ok(ctx
        .env()
        .current_dir()?
        .join("share/icons/hicolor/128x128/apps/shellcue-desktop.png"))
}

/// The path to the data directory auto-created by the Linux windowing application.
pub fn local_webview_data_dir<Ctx: FsProvider + EnvProvider + PlatformProvider>(ctx: &Ctx) -> Result<PathBuf> {
    let os = ctx.platform().os();
    if os != Os::Linux {
        return Err(DirectoryError::UnsupportedOs(os));
    }
    Ok(local_data_dir(ctx)?.join(crate::consts::linux::DESKTOP_APP_WM_CLASS))
}

utf8_dir!(home_dir);
#[cfg(unix)]
utf8_dir!(home_local_bin);
utf8_dir!(fig_data_dir);
utf8_dir!(sockets_dir);
utf8_dir!(figterm_socket_path, session_id: impl Display);
utf8_dir!(manifest_path);
utf8_dir!(backups_dir);
utf8_dir!(logs_dir);
utf8_dir!(settings_path);

#[cfg(test)]
mod linux_tests {
    use super::*;

    #[test]
    fn all_paths() {
        assert!(home_dir().is_ok());
        #[cfg(unix)]
        assert!(home_local_bin().is_ok());
        assert!(fig_data_dir().is_ok());
        assert!(sockets_dir().is_ok());
        assert!(local_remote_socket_path().is_ok());
        assert!(figterm_socket_path("test").is_ok());
        assert!(resources_path().is_ok());
        assert!(manifest_path().is_ok());
        assert!(backups_dir().is_ok());
        assert!(logs_dir().is_ok());
        assert!(settings_path().is_ok());
    }

    #[test]
    fn shellcue_path_names() {
        assert!(settings_path().unwrap().ends_with(".config/shellcue/settings.json"));
        assert!(settings_dir().unwrap().ends_with(".config/shellcue"));
        assert!(logs_dir().unwrap().ends_with("shellcue/logs"));
        assert_eq!(sockets_dir().unwrap().file_name().unwrap(), "shellcue");
        assert_eq!(fig_data_dir().unwrap().file_name().unwrap(), "shellcue");
        assert_eq!(backups_dir().unwrap().file_name().unwrap(), ".shellcue.dotfiles.bak");
    }
}

// TODO(grant): Add back path tests on linux
#[cfg(all(test, not(target_os = "linux")))]
mod tests {
    use insta;

    use super::*;

    /// If this test fails then either of these paths were changed.
    ///
    /// Since we set the permissions of the parent of these paths, make sure they're in folders we
    /// own otherwise we will set permissions of directories we shouldn't
    #[test]
    fn test_socket_paths() {
        #[cfg(unix)]
        assert_eq!(
            host_sockets_dir().unwrap().file_name().unwrap().to_str().unwrap(),
            format!("shellcue")
        );

        #[cfg(unix)]
        assert_eq!(
            figterm_socket_path("").unwrap().parent().unwrap().file_name().unwrap(),
            "t"
        );
    }

    macro_rules! assert_directory {
        ($value:expr, @$snapshot:literal) => {
            insta::assert_snapshot!(
                sanitized_directory_path($value),
                @$snapshot,
            )
        };
    }

    macro_rules! macos {
        ($value:expr, @$snapshot:literal) => {
            #[cfg(target_os = "macos")]
            assert_directory!($value, @$snapshot)
        };
    }

    macro_rules! linux {
        ($value:expr, @$snapshot:literal) => {
            #[cfg(target_os = "linux")]
            assert_directory!($value, @$snapshot)
        };
    }

    fn sanitized_directory_path(path: Result<PathBuf>) -> String {
        let mut path = path.unwrap().into_os_string().into_string().unwrap();

        if let Ok(home) = std::env::var("HOME") {
            let home = home.strip_suffix('/').unwrap_or(&home);
            path = path.replace(home, "$HOME");
        }

        let user = whoami::username();
        path = path.replace(&user, "$USER");

        if let Ok(tmpdir) = std::env::var("TMPDIR") {
            let tmpdir = tmpdir.strip_suffix('/').unwrap_or(&tmpdir);
            path = path.replace(tmpdir, "$TMPDIR");
        }

        #[cfg(target_os = "macos")]
        {
            if let Ok(tmpdir) = macos_tempdir() {
                let tmpdir = tmpdir.to_str().unwrap();
                let tmpdir = tmpdir.strip_suffix('/').unwrap_or(tmpdir);
                path = path.replace(tmpdir, "$TMPDIR");
            };
        }

        if let Ok(xdg_runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
            let xdg_runtime_dir = xdg_runtime_dir.strip_suffix('/').unwrap_or(&xdg_runtime_dir);
            path = path.replace(xdg_runtime_dir, "$XDG_RUNTIME_DIR");
        }

        #[cfg(target_os = "linux")]
        {
            path = path.replace("/tmp", "$TMPDIR");
        }

        path
    }

    #[cfg(unix)]
    #[test]
    fn snapshot_home_local_bin() {
        #[cfg(target_os = "linux")]
        linux!(home_local_bin(), @"$HOME/.local/bin");
        #[cfg(target_os = "macos")]
        macos!(home_local_bin(), @"$HOME/.local/bin");
    }

    #[test]
    fn snapshot_fig_data_dir() {
        linux!(fig_data_dir(), @"$HOME/.local/share/shellcue");
        macos!(fig_data_dir(), @"$HOME/Library/Application Support/shellcue");
    }

    #[test]
    fn snapshot_sockets_dir() {
        linux!(sockets_dir(), @"$XDG_RUNTIME_DIR/shellcue");
        macos!(sockets_dir(), @"$TMPDIR/shellcue");
    }

    #[test]
    fn snapshot_themes_dir() {
        linux!(themes_dir(&Context::new()), @"/usr/share/shellcue/themes");
        macos!(themes_dir(&Context::new()), @"/Applications/ShellCue.app/Contents/Resources/themes");
    }

    #[test]
    fn snapshot_backups_dir() {
        linux!(backups_dir(), @"$HOME/.shellcue.dotfiles.bak");
        macos!(backups_dir(), @"$HOME/.shellcue.dotfiles.bak");
    }

    #[test]
    fn snapshot_fig_socket_path() {
        linux!(desktop_socket_path(), @"$XDG_RUNTIME_DIR/shellcue/desktop.sock");
        macos!(desktop_socket_path(), @"$TMPDIR/shellcue/desktop.sock");
    }

    #[test]
    fn snapshot_local_remote_socket_path() {
        linux!(local_remote_socket_path(), @"$XDG_RUNTIME_DIR/shellcue/remote.sock");
        macos!(local_remote_socket_path(), @"$TMPDIR/shellcue/remote.sock");
    }

    #[test]
    fn snapshot_figterm_socket_path() {
        linux!(figterm_socket_path("$SESSION_ID"), @"$XDG_RUNTIME_DIR/shellcue/t/$SESSION_ID.sock");
        macos!(figterm_socket_path("$SESSION_ID"), @"$TMPDIR/shellcue/t/$SESSION_ID.sock");
    }

    #[test]
    fn snapshot_settings_path() {
        linux!(settings_path(), @"$HOME/.config/shellcue/settings.json");
        macos!(settings_path(), @"$HOME/.config/shellcue/settings.json");
    }

    #[test]
    #[cfg(unix)]
    fn socket_path_length() {
        use std::os::unix::ffi::OsStrExt;
        /// Sockets are bounded at 100 bytes, why, because legacy compat
        const MAX_SOCKET_LEN: usize = 100;

        let uuid = uuid::Uuid::new_v4().simple().to_string();
        let qterm_socket = figterm_socket_path(uuid.clone()).unwrap();
        let qterm_socket_bytes = qterm_socket.as_os_str().as_bytes().len();
        assert!(qterm_socket_bytes <= MAX_SOCKET_LEN);

        let fig_socket = desktop_socket_path().unwrap();
        let fig_socket_bytes = fig_socket.as_os_str().as_bytes().len();
        assert!(fig_socket_bytes <= MAX_SOCKET_LEN);

        let secure_socket = local_remote_socket_path().unwrap();
        let secure_socket_bytes = secure_socket.as_os_str().as_bytes().len();
        assert!(secure_socket_bytes <= MAX_SOCKET_LEN);
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn macos_tempdir_test() {
        let tmpdir = macos_tempdir().unwrap();
        println!("{:?}", tmpdir);
    }
}
