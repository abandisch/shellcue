//! Launch at login: the setting records the choice; on macOS a LaunchAgent file carries it out.
//!
//! No `launchctl`: macOS reads the file at the next login, and unloading a job loaded this session
//! could quit the app that job started.

use fig_util::consts::{
    LAUNCH_ON_STARTUP_DEFAULT,
    LAUNCH_ON_STARTUP_KEY,
};

#[cfg(target_os = "macos")]
mod agent {
    use std::io::ErrorKind;
    use std::path::{
        Path,
        PathBuf,
    };

    use anyhow::bail;
    use fig_util::consts::{
        APP_BUNDLE_ID,
        APP_PROCESS_NAME,
        PRODUCT_NAME,
    };
    use fig_util::launchd_plist::{
        LaunchdPlist,
        create_launch_agent,
    };
    use macos_utils::bundle::get_bundle_path;
    use tracing::warn;

    const LABEL: &str = "com.bandisch-apps.shellcue.launcher";

    /// The only place the LaunchAgent is described, so launch and the toggle cannot drift.
    pub(super) fn launch_agent(bundle: &Path) -> LaunchdPlist {
        let exe = bundle.join("Contents").join("MacOS").join(APP_PROCESS_NAME);
        LaunchdPlist::new(LABEL)
            .program_arguments([&exe.to_string_lossy(), "--is-startup"])
            .associated_bundle_identifiers([APP_BUNDLE_ID])
            .run_at_load(true)
    }

    pub fn path() -> Result<PathBuf, fig_util::Error> {
        LaunchdPlist::new(LABEL).get_file_path().map(Into::into)
    }

    /// A bare binary has no bundle to start, and its folder would be written into the file.
    pub(super) fn is_app_bundle(path: &Path) -> bool {
        path.extension().is_some_and(|extension| extension == "app")
    }

    fn app_bundle_path() -> Option<PathBuf> {
        get_bundle_path().filter(|bundle| is_app_bundle(bundle))
    }

    pub(super) fn write() -> anyhow::Result<()> {
        let Some(bundle) = app_bundle_path() else {
            bail!("{} is not running from its app bundle", PRODUCT_NAME);
        };
        create_launch_agent(&launch_agent(&bundle))?;
        Ok(())
    }

    pub(super) fn remove() -> anyhow::Result<()> {
        match std::fs::remove_file(path()?) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == ErrorKind::NotFound => Ok(()),
            Err(err) => Err(err.into()),
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(super) enum Reconcile {
        Nothing,
        Write,
        Remove,
    }

    pub(super) fn reconcile_action(enabled: bool, present: bool, matches: bool) -> Reconcile {
        match (enabled, present, matches) {
            (true, true, true) | (false, false, _) => Reconcile::Nothing,
            (true, _, _) => Reconcile::Write,
            (false, true, _) => Reconcile::Remove,
        }
    }

    /// Makes the file match the setting, writing only when its content differs.
    pub fn reconcile() {
        let path = match path() {
            Ok(path) => path,
            Err(err) => {
                warn!(%err, "Failed to get the launch at login file path");
                return;
            },
        };
        let current = std::fs::read_to_string(&path).ok();
        let wanted = app_bundle_path().map(|bundle| launch_agent(&bundle).plist());
        let matches = current.is_some() && current == wanted;
        let result = match reconcile_action(super::setting(), current.is_some(), matches) {
            Reconcile::Nothing => Ok(()),
            Reconcile::Write => write(),
            Reconcile::Remove => remove(),
        };
        if let Err(err) = result {
            warn!("Failed to match the launch at login file to the setting: {err:#}");
        }
    }
}

#[cfg(target_os = "macos")]
pub use agent::{
    path as agent_path,
    reconcile,
};

fn setting() -> bool {
    fig_settings::settings::get_bool_or(LAUNCH_ON_STARTUP_KEY, LAUNCH_ON_STARTUP_DEFAULT)
}

pub fn launch_at_login_enabled() -> bool {
    #[cfg(target_os = "macos")]
    {
        setting() && agent::path().is_ok_and(|path| path.exists())
    }
    #[cfg(not(target_os = "macos"))]
    {
        setting()
    }
}

/// Serialises changes so the file and the setting cannot be left disagreeing.
static LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

/// On macOS the file changes first, so a failure leaves the setting as it was.
#[cfg(target_os = "macos")]
pub fn set_launch_at_login(enabled: bool) -> anyhow::Result<()> {
    let _guard = LOCK.lock();
    set_unlocked(enabled)
}

/// Returns the state read after the change.
pub fn toggle_launch_at_login() -> anyhow::Result<bool> {
    let _guard = LOCK.lock();
    set_unlocked(!launch_at_login_enabled())?;
    Ok(launch_at_login_enabled())
}

fn set_unlocked(enabled: bool) -> anyhow::Result<()> {
    #[cfg(target_os = "macos")]
    if enabled {
        agent::write()?;
    } else {
        agent::remove()?;
    }
    fig_settings::settings::set_value(LAUNCH_ON_STARTUP_KEY, enabled)?;
    Ok(())
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use std::path::Path;

    use super::agent::{
        Reconcile,
        is_app_bundle,
        launch_agent,
        reconcile_action,
    };

    #[test]
    fn only_an_app_bundle_is_written() {
        for (path, expected) in [
            ("/Applications/ShellCue.app", true),
            ("/Users/someone/Downloads/ShellCue.app", true),
            ("/Users/someone/shellcue/target/debug", false),
            ("/usr/local/bin", false),
            ("/Applications/ShellCue.app/Contents/MacOS", false),
            ("/", false),
        ] {
            assert_eq!(is_app_bundle(Path::new(path)), expected, "{path}");
        }
    }

    #[test]
    fn launch_agent_is_pinned() {
        let plist = launch_agent(Path::new("/Applications/ShellCue.app")).plist();
        assert_eq!(
            plist,
            concat!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n",
                "<!DOCTYPE plist PUBLIC \"-//Apple Computer//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n",
                "<plist version=\"1.0\">\n",
                "    <dict>\n",
                "        <key>Label</key>\n",
                "        <string>com.bandisch-apps.shellcue.launcher</string>\n",
                "        <key>ProgramArguments</key>\n",
                "        <array>\n",
                "            <string>/Applications/ShellCue.app/Contents/MacOS/shellcue-desktop</string>\n",
                "            <string>--is-startup</string>\n",
                "        </array>\n",
                "        <key>RunAtLoad</key>\n",
                "        <true/>\n",
                "        <key>AssociatedBundleIdentifiers</key>\n",
                "        <array>\n",
                "            <string>com.bandisch-apps.shellcue</string>\n",
                "        </array>\n",
                "    </dict>\n",
                "</plist>\n",
            )
        );
        assert!(!plist.contains("KeepAlive"));
    }

    #[test]
    fn reconcile_follows_the_setting() {
        for (enabled, present, matches, expected) in [
            (true, false, false, Reconcile::Write),
            (true, true, false, Reconcile::Write),
            (true, true, true, Reconcile::Nothing),
            (false, true, false, Reconcile::Remove),
            (false, true, true, Reconcile::Remove),
            (false, false, false, Reconcile::Nothing),
        ] {
            assert_eq!(
                reconcile_action(enabled, present, matches),
                expected,
                "enabled={enabled} present={present} matches={matches}"
            );
        }
    }
}
