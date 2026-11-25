use std::io::ErrorKind;
use std::path::{
    Path,
    PathBuf,
};

use fig_util::consts::{
    APP_BUNDLE_ID,
    CLI_BINARY_NAME,
    PTY_BINARY_NAME,
};
use fig_util::directories;
use tokio::fs;
use tokio::io::{
    AsyncReadExt,
    AsyncWriteExt,
};
use tracing::warn;

use crate::Error;

/// Where the commands are linked from: the folder of the canonicalized running executable.
pub fn commands_dir() -> Result<PathBuf, Error> {
    let exe = fig_util::current_exe_origin()?;
    Ok(exe.parent().unwrap_or(Path::new("/")).to_path_buf())
}

/// Links the CLI and the pty that sit next to the running executable into `~/.local/bin`.
pub async fn link_commands() -> Result<(), Error> {
    link_commands_from(&commands_dir()?, &directories::home_local_bin()?).await
}

/// Changes nothing unless both binaries are in `dir`; one link alone breaks the integration.
async fn link_commands_from(dir: &Path, bin_dir: &Path) -> Result<(), Error> {
    let names = [CLI_BINARY_NAME, PTY_BINARY_NAME];
    for name in names {
        let source = dir.join(name);
        if !source.is_file() {
            return Err(fig_integrations::Error::FileDoesNotExist(source.into()).into());
        }
    }
    fs::create_dir_all(bin_dir).await.map_err(|err| io_error(err, bin_dir))?;
    for name in names {
        let link = bin_dir.join(name);
        symlink(dir.join(name), &link).await.map_err(|err| io_error(err, &link))?;
    }
    Ok(())
}

fn io_error(err: std::io::Error, path: &Path) -> Error {
    fig_integrations::Error::Context {
        error: Box::new(err.into()),
        context: path.display().to_string().into(),
    }
    .into()
}

/// Symlink, and overwrite if it already exists and is invalid or not a symlink
async fn symlink(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> Result<(), std::io::Error> {
    let src = src.as_ref();
    let dst = dst.as_ref();

    // Check if the link already exists
    match fs::symlink_metadata(dst).await {
        Ok(metadata) => {
            // If it's a symlink, check if it points to the right place
            if metadata.file_type().is_symlink() {
                if let Ok(read_link) = fs::read_link(dst).await {
                    if read_link == src {
                        return Ok(());
                    }
                }
            }

            // If it's not a symlink or it points to the wrong place, delete it
            fs::remove_file(dst).await?;
        },
        Err(err) if err.kind() == ErrorKind::NotFound => {},
        Err(err) => return Err(err),
    }

    // Create the symlink
    fs::symlink(src, dst).await
}

async fn remove_in_dir_with_prefix_unless(dir: &Path, prefix: &str, unless: impl Fn(&str) -> bool) {
    if let Ok(mut entries) = fs::read_dir(dir).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            if let Some(name) = entry.file_name().to_str() {
                if name.starts_with(prefix) && !unless(name) {
                    fs::remove_file(entry.path()).await.ok();
                    fs::remove_dir_all(entry.path()).await.ok();
                }
            }
        }
    }
}

#[allow(unused_variables)]
pub(crate) async fn uninstall_desktop(ctx: &fig_os_shim::Context) -> Result<(), Error> {
    // TODO:
    // 1. Set title of running ttys "Restart this terminal to finish uninstalling Q..."
    // 2. Delete webview cache

    // Remove launch agents
    if let Ok(home) = directories::home_dir() {
        let launch_agents = home.join("Library").join("LaunchAgents");
        remove_in_dir_with_prefix_unless(&launch_agents, "com.bandisch-apps.shellcue.", |p| p.contains("daemon")).await;
    } else {
        warn!("Could not find home directory");
    }

    // Delete Fig defaults on macOS
    tokio::process::Command::new("defaults")
        .args(["delete", APP_BUNDLE_ID])
        .output()
        .await
        .map_err(|err| warn!("Failed to delete defaults: {err}"))
        .ok();

    uninstall_terminal_integrations().await;

    // Delete data dir
    if let Ok(fig_data_dir) = directories::fig_data_dir() {
        let state = fig_settings::state::get_string("anonymousId").unwrap_or_default();

        for file in std::fs::read_dir(fig_data_dir).ok().into_iter().flatten().flatten() {
            if let Some(file_name) = file.file_name().to_str() {
                if file_name == "credentials.json" {
                } else if file_name == "state.json" {
                    std::fs::write(file.path(), serde_json::json!({ "anonymousId": state }).to_string())
                        .map_err(|err| warn!("Failed to write state.json: {err}"))
                        .ok();
                } else if let Ok(metadata) = file.metadata() {
                    if metadata.is_dir() {
                        fs::remove_dir_all(file.path())
                            .await
                            .map_err(|err| warn!("Failed to remove data dir: {err}"))
                            .ok();
                    } else {
                        fs::remove_file(file.path())
                            .await
                            .map_err(|err| warn!("Failed to remove data dir: {err}"))
                            .ok();
                    }
                }
            }
        }
    }

    // Delete settings dir
    if let Ok(settings_dir) = directories::settings_dir() {
        if settings_dir.exists() {
            fs::remove_dir_all(&settings_dir)
                .await
                .map_err(|err| warn!("Failed to remove settings dir: {err}"))
                .ok();
        }
    }

    let app_path = fig_util::app_bundle_path();
    if app_path.exists() {
        fs::remove_dir_all(&app_path)
            .await
            .map_err(|err| warn!("Failed to remove {app_path:?}: {err}"))
            .ok();
    }

    Ok(())
}

pub async fn uninstall_terminal_integrations() {
    // Delete integrations
    if let Ok(home) = directories::home_dir() {
        // Remove Hyper integration
        let hyper_path = home.join(".hyper.js");
        if hyper_path.exists() {
            // Read the config file
            match fs::File::open(&hyper_path).await {
                Ok(mut file) => {
                    let mut contents = String::new();
                    match file.read_to_string(&mut contents).await {
                        Ok(_) => {
                            contents = contents.replace("\"fig-hyper-integration\",", "");
                            contents = contents.replace("\"fig-hyper-integration\"", "");

                            // Write the config file
                            match fs::File::create(&hyper_path).await {
                                Ok(mut file) => {
                                    file.write_all(contents.as_bytes())
                                        .await
                                        .map_err(|err| warn!("Could not write to Hyper config: {err}"))
                                        .ok();
                                },
                                Err(err) => {
                                    warn!("Could not create Hyper config: {err}");
                                },
                            }
                        },
                        Err(err) => {
                            warn!("Could not read Hyper config: {err}");
                        },
                    }
                },
                Err(err) => {
                    warn!("Could not open Hyper config: {err}");
                },
            }
        }

        // Remove Kitty integration
        let kitty_path = home.join(".config").join("kitty").join("kitty.conf");
        if kitty_path.exists() {
            // Read the config file
            match fs::File::open(&kitty_path).await {
                Ok(mut file) => {
                    let mut contents = String::new();
                    match file.read_to_string(&mut contents).await {
                        Ok(_) => {
                            contents = contents.replace("watcher ${HOME}/.fig/tools/kitty-integration.py", "");
                            // Write the config file
                            match fs::File::create(&kitty_path).await {
                                Ok(mut file) => {
                                    file.write_all(contents.as_bytes())
                                        .await
                                        .map_err(|err| warn!("Could not write to Kitty config: {err}"))
                                        .ok();
                                },
                                Err(err) => {
                                    warn!("Could not create Kitty config: {err}");
                                },
                            }
                        },
                        Err(err) => {
                            warn!("Could not read Kitty config: {err}");
                        },
                    }
                },
                Err(err) => {
                    warn!("Could not open Kitty config: {err}");
                },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh folder under the system temp dir, removed on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!("fig_install-{name}-{}", std::process::id()));
            std::fs::remove_dir_all(&path).ok();
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }

    #[tokio::test]
    async fn test_symlink() {
        let tmp_dir = TempDir::new("symlink");
        let tmp_dir = tmp_dir.0.as_path();

        // folders
        let src_dir_1 = tmp_dir.join("dir_1");
        let src_dir_2 = tmp_dir.join("dir_2");
        let dst_dir = tmp_dir.join("dst");

        std::fs::create_dir_all(&src_dir_1).unwrap();
        std::fs::create_dir_all(&src_dir_2).unwrap();

        // Check that a new symlink is created
        assert!(!dst_dir.exists());
        symlink(&src_dir_1, &dst_dir).await.unwrap();
        assert!(dst_dir.exists());
        assert_eq!(dst_dir.read_link().unwrap(), src_dir_1);

        // Check that the symlink is updated
        symlink(&src_dir_2, &dst_dir).await.unwrap();
        assert!(dst_dir.exists());
        assert_eq!(dst_dir.read_link().unwrap(), src_dir_2);

        // files
        let src_file_1 = src_dir_1.join("file_1");
        let src_file_2 = src_dir_2.join("file_2");
        let dst_file = dst_dir.join("file");

        std::fs::write(&src_file_1, "content 1").unwrap();
        std::fs::write(&src_file_2, "content 2").unwrap();

        // Check that a new symlink is created
        assert!(!dst_file.exists());
        symlink(&src_file_1, &dst_file).await.unwrap();
        assert!(dst_file.exists());
        assert_eq!(std::fs::read_to_string(&dst_file).unwrap(), "content 1");

        // Check that the symlink is updated
        symlink(&src_file_2, &dst_file).await.unwrap();
        assert!(dst_file.exists());
        assert_eq!(std::fs::read_to_string(&dst_file).unwrap(), "content 2");
    }

    #[tokio::test]
    async fn links_only_when_both_binaries_are_present() {
        let tmp_dir = TempDir::new("link-commands");
        let app_dir = tmp_dir.0.join("MacOS");
        let bin_dir = tmp_dir.0.join("bin");
        std::fs::create_dir_all(&app_dir).unwrap();

        std::fs::write(app_dir.join(CLI_BINARY_NAME), "cli").unwrap();
        assert!(link_commands_from(&app_dir, &bin_dir).await.is_err());
        assert!(!bin_dir.exists());

        std::fs::remove_file(app_dir.join(CLI_BINARY_NAME)).unwrap();
        std::fs::write(app_dir.join(PTY_BINARY_NAME), "pty").unwrap();
        assert!(link_commands_from(&app_dir, &bin_dir).await.is_err());
        assert!(!bin_dir.exists());

        std::fs::create_dir_all(app_dir.join(CLI_BINARY_NAME)).unwrap();
        assert!(link_commands_from(&app_dir, &bin_dir).await.is_err());
        assert!(!bin_dir.exists());

        std::fs::remove_dir(app_dir.join(CLI_BINARY_NAME)).unwrap();
        std::fs::write(app_dir.join(CLI_BINARY_NAME), "cli").unwrap();
        std::fs::create_dir_all(&bin_dir).unwrap();
        std::fs::write(bin_dir.join(CLI_BINARY_NAME), "something else").unwrap();
        link_commands_from(&app_dir, &bin_dir).await.unwrap();
        for name in [CLI_BINARY_NAME, PTY_BINARY_NAME] {
            assert_eq!(bin_dir.join(name).read_link().unwrap(), app_dir.join(name));
        }
    }
}
