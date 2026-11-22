use std::fmt::Display;
use std::path::{
    Path,
    PathBuf,
};
use std::str::FromStr;

use clap::ValueEnum;
use fig_os_shim::Env;
use regex::Regex;
use serde::{
    Deserialize,
    Serialize,
};
use tokio::process::Command;

use crate::env_var::Q_ZDOTDIR;
use crate::process_info::get_parent_process_exe;
use crate::{
    Error,
    directories,
};

/// All supported shells
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "camelCase")]
pub enum Shell {
    /// Zsh shell
    Zsh,
}

impl Display for Shell {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Shell {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, ()> {
        match s {
            "zsh" => Ok(Shell::Zsh),
            _ => Err(()),
        }
    }
}

impl Shell {
    pub fn all() -> &'static [Self] {
        &[Shell::Zsh]
    }

    /// All shells to run unit / integration tests with
    pub fn all_test() -> Vec<Self> {
        vec![Shell::Zsh]
    }

    /// Try to find the name of common shells in the input
    pub fn try_find_shell(input: impl AsRef<Path>) -> Option<Self> {
        let input = input.as_ref().file_name()?.to_str()?;
        input.contains("zsh").then_some(Shell::Zsh)
    }

    /// Gets the current shell of the parent process
    pub fn current_shell() -> Option<Self> {
        let parent_exe = get_parent_process_exe()?;
        let parent_exe_name = parent_exe.to_str()?;
        Self::try_find_shell(parent_exe_name)
    }

    pub async fn current_shell_version() -> Result<(Self, String), Error> {
        let parent_exe = get_parent_process_exe().ok_or(Error::NoParentProcess)?;
        let Some(shell) = Self::try_find_shell(&parent_exe) else {
            return Err(Error::UnknownShell(parent_exe.to_string_lossy().into()));
        };

        Ok((shell, shell_version(&shell, &parent_exe).await?))
    }

    /// Get the directory for the shell that contains the dotfiles
    pub fn get_config_directory(&self, env: &Env) -> Result<PathBuf, directories::DirectoryError> {
        match self {
            Shell::Zsh => match env
                .get_os("ZDOTDIR")
                .or_else(|| env.get_os(Q_ZDOTDIR))
                .map(PathBuf::from)
            {
                Some(dir) => Ok(dir),
                None => Ok(directories::home_dir()?),
            },
        }
    }

    pub fn get_data_path(&self) -> Result<PathBuf, directories::DirectoryError> {
        Ok(directories::fig_data_dir()?.join("shell").join(format!("{self}.json")))
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Shell::Zsh => "zsh",
        }
    }

    pub fn is_zsh(&self) -> bool {
        matches!(self, Shell::Zsh)
    }
}

const ZSH_RE: &str = r"(\d+\.\d+)";

async fn shell_version(shell: &Shell, exe_path: &Path) -> Result<String, Error> {
    let err = || Error::ShellVersion(*shell);
    match shell {
        Shell::Zsh => {
            let re = Regex::new(ZSH_RE).unwrap();
            let version_output = Command::new(exe_path).arg("--version").output().await?;
            let version_capture = re.captures(std::str::from_utf8(&version_output.stdout)?);
            Ok(version_capture.ok_or_else(err)?.get(1).ok_or_else(err)?.as_str().into())
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(windows))]
    #[tokio::test]
    async fn test_shell_version() {
        let exe_path_str = "/bin/zsh";
        let exe_path = Path::new(exe_path_str);
        let version: String = shell_version(&Shell::Zsh, exe_path)
            .await
            .unwrap_or_else(|err| panic!("exe {} failed. Error: {:?}", exe_path_str, err));
        println!("{}: {version:?}\n", exe_path.display());
    }

    #[test]
    fn test_zsh_re() {
        let re = Regex::new(ZSH_RE).unwrap();
        let zsh_version = "zsh 5.9 (arm-apple-darwin22.1.0)";
        assert_eq!(re.captures(zsh_version).unwrap().get(1).unwrap().as_str(), "5.9");
    }

    #[test]
    fn test_all_is_zsh_only() {
        assert_eq!(Shell::all(), &[Shell::Zsh]);
    }

    #[test]
    fn test_removed_shells_fail_from_str() {
        for name in ["bash", "fish", "nu"] {
            assert!(Shell::from_str(name).is_err(), "{name} should not parse");
            assert!(Shell::try_find_shell(name).is_none(), "{name} should not be found");
        }
        assert_eq!(Shell::from_str("zsh"), Ok(Shell::Zsh));
    }
}
