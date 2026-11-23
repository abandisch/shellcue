use std::path::Path;

use fig_util::consts::APP_BUNDLE_ID;
use fig_util::directories;
use tokio::fs;
use tokio::io::{
    AsyncReadExt,
    AsyncWriteExt,
};
use tracing::warn;

use crate::Error;

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
        remove_in_dir_with_prefix_unless(&launch_agents, "com.amazon.codewhisperer.", |p| p.contains("daemon")).await;
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
