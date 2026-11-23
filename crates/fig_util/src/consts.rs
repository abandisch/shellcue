pub const APP_BUNDLE_ID: &str = "com.amazon.codewhisperer";
pub const APP_BUNDLE_NAME: &str = "Amazon Q.app";

#[cfg(target_os = "macos")]
pub const APP_PROCESS_NAME: &str = "q_desktop";
#[cfg(target_os = "linux")]
pub const APP_PROCESS_NAME: &str = "q-desktop";

/// The name configured under `"package.productName"` in the tauri.conf.json file.
pub const TAURI_PRODUCT_NAME: &str = "q_desktop";

pub const CLI_BINARY_NAME: &str = "shellcue";
pub const PTY_BINARY_NAME: &str = "shellcue-term";

pub const CLI_CRATE_NAME: &str = "q_cli";

pub const PRODUCT_NAME: &str = "ShellCue";

pub const RUNTIME_DIR_NAME: &str = "shellcue";

/// Data directory name used in paths like ~/.local/share/{DATA_DIR_NAME}
#[cfg(unix)]
pub const DATA_DIR_NAME: &str = "shellcue";

/// Backup directory name
pub const BACKUP_DIR_NAME: &str = ".shellcue.dotfiles.bak";

/// Build time env vars
pub mod build {
    /// The target of the current build, e.g. "aarch64-unknown-linux-musl"
    pub const TARGET_TRIPLE: Option<&str> = option_env!("AMAZON_Q_BUILD_TARGET_TRIPLE");

    /// A git full sha hash of the current build
    pub const HASH: Option<&str> = option_env!("AMAZON_Q_BUILD_HASH");

    /// The datetime in rfc3339 format of the current build
    pub const DATETIME: Option<&str> = option_env!("AMAZON_Q_BUILD_DATETIME");

    /// If `shellcheck` tests should be skipped
    pub const SKIP_SHELLCHECK_TESTS: bool = option_env!("AMAZON_Q_BUILD_SKIP_SHELLCHECK_TESTS").is_some();
}

/// macOS specific constants
pub mod macos {
    pub const BUNDLE_CONTENTS_MACOS_PATH: &str = "Contents/MacOS";
    pub const BUNDLE_CONTENTS_RESOURCE_PATH: &str = "Contents/Resources";
    pub const BUNDLE_CONTENTS_HELPERS_PATH: &str = "Contents/Helpers";
    pub const BUNDLE_CONTENTS_INFO_PLIST_PATH: &str = "Contents/Info.plist";
}

pub mod linux {
    pub const DESKTOP_ENTRY_NAME: &str = "amazon-q.desktop";

    /// Name of the deb package.
    pub const PACKAGE_NAME: &str = "amazon-q";

    /// The wm_class used for the application windows.
    pub const DESKTOP_APP_WM_CLASS: &str = "Amazon-q";
}

pub mod env_var {
    macro_rules! define_env_vars {
        ($($(#[$meta:meta])* $ident:ident = $name:expr),*) => {
            $(
                $(#[$meta])*
                pub const $ident: &str = $name;
            )*

            pub const ALL: &[&str] = &[$($ident),*];
        }
    }

    define_env_vars! {
        /// The UUID of the current parent shellcue-term instance
        SHELLCUE_TERM_SESSION_ID = "SHELLCUE_TERM_SESSION_ID",

        /// Set if shellcue-term is running, contains the version
        SHELLCUE_TERM = "SHELLCUE_TERM",

        /// Sets the current log level
        SHELLCUE_LOG_LEVEL = "SHELLCUE_LOG_LEVEL",

        /// Overrides the ZDOTDIR environment variable
        SHELLCUE_ZDOTDIR = "SHELLCUE_ZDOTDIR",

        /// Indicates a process was launched by ShellCue
        PROCESS_LAUNCHED_BY_SHELLCUE = "PROCESS_LAUNCHED_BY_SHELLCUE",

        /// The shell to use in shellcue-term
        SHELLCUE_SHELL = "SHELLCUE_SHELL",

        /// Indicates the user is debugging the shell
        SHELLCUE_DEBUG_SHELL = "SHELLCUE_DEBUG_SHELL"
    }
}

pub mod system_paths {
    /// System installation paths
    pub const APPLICATIONS_DIR: &str = "/Applications";
    pub const USR_LOCAL_BIN: &str = "/usr/local/bin";
    pub const USR_SHARE: &str = "/usr/share";
    pub const OPT_HOMEBREW_BIN: &str = "/opt/homebrew/bin";
}

#[cfg(test)]
mod tests {
    use time::OffsetDateTime;
    use time::format_description::well_known::Rfc3339;

    use super::*;

    #[test]
    fn test_build_envs() {
        if let Some(build_hash) = build::HASH {
            println!("build_hash: {build_hash}");
            assert!(!build_hash.is_empty());
        }

        if let Some(build_datetime) = build::DATETIME {
            println!("build_datetime: {build_datetime}");
            println!("{}", OffsetDateTime::parse(build_datetime, &Rfc3339).unwrap());
        }
    }
}
