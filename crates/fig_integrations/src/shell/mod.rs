use std::fs::File;
use std::io::{
    ErrorKind,
    Write,
};
use std::path::PathBuf;

use async_trait::async_trait;
use cfg_if::cfg_if;
use clap::ValueEnum;
use fig_os_shim::Env;
use fig_util::{
    CLI_BINARY_NAME,
    PRODUCT_NAME,
    PTY_BINARY_NAME,
    Shell,
    directories,
};
use regex::Regex;
use serde::{
    Deserialize,
    Serialize,
};

use crate::error::{
    ErrorExt,
    Result,
};
use crate::{
    Error,
    FileIntegration,
    Integration,
    backup_file,
};

#[derive(Debug, Copy, Clone, PartialEq, Eq, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum When {
    Pre,
    Post,
}

impl When {
    pub fn all() -> [When; 2] {
        [Self::Pre, Self::Post]
    }
}

impl std::fmt::Display for When {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            When::Pre => write!(f, "pre"),
            When::Post => write!(f, "post"),
        }
    }
}

fn integration_file_name(dotfile_name: &str, when: &When, shell: &Shell) -> String {
    format!(
        "{}.{when}.{shell}",
        Regex::new(r"^\.").unwrap().replace_all(dotfile_name, ""),
    )
}

pub trait ShellExt {
    fn get_shell_integrations(&self, env: &Env) -> Result<Vec<Box<dyn ShellIntegration>>>;
    /// Script integrations are installed into ~/.fig/shell
    fn get_script_integrations(&self) -> Result<Vec<ShellScriptShellIntegration>>;
    fn get_fig_integration_source(&self, when: &When) -> String;
}

impl ShellExt for Shell {
    fn get_script_integrations(&self) -> Result<Vec<ShellScriptShellIntegration>> {
        let mut integrations = vec![];

        for file in match self {
            Shell::Zsh => [".zshrc", ".zprofile"].iter(),
        } {
            for when in &When::all() {
                let path = directories::fig_data_dir()?
                    .join("shell")
                    .join(integration_file_name(file, when, self));

                integrations.push(ShellScriptShellIntegration {
                    shell: *self,
                    when: *when,
                    path,
                });
            }
        }

        Ok(integrations)
    }

    fn get_shell_integrations(&self, env: &Env) -> Result<Vec<Box<dyn ShellIntegration>>> {
        let config_dir = self.get_config_directory(env)?;

        let integrations: Vec<Box<dyn ShellIntegration>> = match self {
            Shell::Zsh => vec![".zshrc", ".zprofile"]
                .into_iter()
                .map(|filename| {
                    Box::new(DotfileShellIntegration {
                        pre: true,
                        post: true,
                        shell: *self,
                        dotfile_directory: config_dir.clone(),
                        dotfile_name: filename,
                    }) as Box<dyn ShellIntegration>
                })
                .collect(),
        };

        Ok(integrations)
    }

    fn get_fig_integration_source(&self, when: &When) -> String {
        let script = match (self, when) {
            (Shell::Zsh, When::Pre) => include_str!("scripts/pre.sh"),
            (Shell::Zsh, When::Post) => include_str!("scripts/post.zsh"),
        };

        script
            .replace("{{CLI_BINARY_NAME}}", CLI_BINARY_NAME)
            .replace("{{PTY_BINARY_NAME}}", PTY_BINARY_NAME)
    }
}

pub trait ShellIntegration: Send + Sync + Integration + ShellIntegrationClone {
    // The unique name of the integration file
    fn file_name(&self) -> &str;
    fn get_shell(&self) -> Shell;
    fn path(&self) -> PathBuf;
}

impl std::fmt::Display for dyn ShellIntegration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.get_shell(), self.path().display())
    }
}

pub trait ShellIntegrationClone {
    fn clone_box(&self) -> Box<dyn ShellIntegration>;
}

impl<T> ShellIntegrationClone for T
where
    T: 'static + ShellIntegration + Clone,
{
    fn clone_box(&self) -> Box<dyn ShellIntegration> {
        Box::new(self.clone())
    }
}

// We can now implement Clone manually by forwarding to clone_box.
impl Clone for Box<dyn ShellIntegration> {
    fn clone(&self) -> Box<dyn ShellIntegration> {
        self.clone_box()
    }
}

#[derive(Debug, Clone)]
pub struct ShellScriptShellIntegration {
    pub shell: Shell,
    pub when: When,
    pub path: PathBuf,
}

fn get_prefix(s: &str) -> &str {
    match s.find('.') {
        Some(i) => &s[..i],
        None => s,
    }
}

impl ShellScriptShellIntegration {
    fn get_file_integration(&self) -> FileIntegration {
        FileIntegration {
            path: self.path.clone(),
            contents: self.get_contents(),
            #[cfg(unix)]
            mode: None,
        }
    }

    fn get_name(&self) -> Option<&str> {
        self.path.file_name().and_then(|s| s.to_str())
    }

    #[allow(clippy::needless_return)]
    fn get_contents(&self) -> String {
        let Self { shell, when, path } = self;
        let rcfile = match path.file_name().and_then(|x| x.to_str()) {
            Some(name) => format!(" --rcfile {}", get_prefix(name)),
            None => "".into(),
        };
        cfg_if!(
            if #[cfg(target_os = "macos")] {
                return match self.shell {
                    // Check if ~/.local/bin/{CLI_BINARY_NAME} is executable before eval
                    Shell::Zsh => format!("[ -x ~/.local/bin/{CLI_BINARY_NAME} ] && eval \"$(~/.local/bin/{CLI_BINARY_NAME} init {shell} {when}{rcfile})\""),
                }
            } else {
                let add_to_path_line = match self.shell {
                    Shell::Zsh => indoc::indoc! {r#"
                        _Q_LOCAL_BIN="$HOME/.local/bin"
                        [[ ":$PATH:" != *":$_Q_LOCAL_BIN:"* ]] && PATH="${PATH:+"$PATH:"}$_Q_LOCAL_BIN"
                        unset _Q_LOCAL_BIN
                    "#},
                };

                let source_line = match self.shell {
                    Shell::Zsh => format!("command -v {CLI_BINARY_NAME} >/dev/null 2>&1 && eval \"$({CLI_BINARY_NAME} init {shell} {when}{rcfile})\""),
                };

                return format!("{add_to_path_line}\n{source_line}\n");
            }
        );
    }
}

#[async_trait]
impl Integration for ShellScriptShellIntegration {
    async fn is_installed(&self) -> Result<()> {
        self.get_file_integration().is_installed().await
    }

    async fn install(&self) -> Result<()> {
        self.get_file_integration().install().await
    }

    async fn uninstall(&self) -> Result<()> {
        self.get_file_integration().uninstall().await
    }

    fn describe(&self) -> String {
        format!("{} {}", self.shell, self.when)
    }

    async fn migrate(&self) -> Result<()> {
        self.get_file_integration().install().await
    }
}

impl ShellIntegration for ShellScriptShellIntegration {
    fn file_name(&self) -> &str {
        self.get_name().unwrap_or("unknown_script")
    }

    fn get_shell(&self) -> Shell {
        self.shell
    }

    fn path(&self) -> PathBuf {
        self.path.clone()
    }
}

/// zsh integration where we modify a dotfile with pre/post hooks that reference script
/// files.
#[derive(Debug, Clone)]
pub struct DotfileShellIntegration {
    pub shell: Shell,
    pub pre: bool,
    pub post: bool,
    pub dotfile_directory: PathBuf,
    pub dotfile_name: &'static str,
}

impl DotfileShellIntegration {
    fn dotfile_path(&self) -> PathBuf {
        self.dotfile_directory.join(self.dotfile_name)
    }

    fn script_integration(&self, when: When) -> Result<ShellScriptShellIntegration> {
        let integration_file_name = format!(
            "{}.{}.{}",
            Regex::new(r"^\.").unwrap().replace_all(self.dotfile_name, ""),
            when,
            self.shell
        );
        Ok(ShellScriptShellIntegration {
            shell: self.shell,
            when,
            path: directories::fig_data_dir()?.join("shell").join(integration_file_name),
        })
    }

    #[allow(clippy::unused_self)]
    fn description(&self, when: When) -> String {
        match when {
            When::Pre => format!("# {PRODUCT_NAME} pre block. Keep at the top of this file."),
            When::Post => format!("# {PRODUCT_NAME} post block. Keep at the bottom of this file."),
        }
    }

    fn source_text(&self, when: When) -> Result<String> {
        let home = directories::home_dir()?;
        let integration_path = self.script_integration(when)?.path;
        let path = format!("\"${{HOME}}/{}\"", integration_path.strip_prefix(home)?.display());

        Ok(format!("[[ -f {path} ]] && builtin source {path}"))
    }

    fn source_regex(&self, when: When, constrain_position: bool) -> Result<Regex> {
        let regex = format!(
            r#"{}(?:{}\n)?{}\n{{0,2}}{}"#,
            if constrain_position && when == When::Pre {
                "^"
            } else {
                ""
            },
            regex::escape(&self.description(when)),
            regex::escape(&self.source_text(when)?),
            if constrain_position && when == When::Post {
                "$"
            } else {
                ""
            },
        );
        Ok(Regex::new(&regex)?)
    }

    fn remove_from_text(&self, text: impl Into<String>, when: When) -> Result<String> {
        let text: String = text.into();
        Ok(self.source_regex(when, false)?.replace_all(&text, "").into())
    }

    fn matches_text(&self, text: &str, when: When) -> Result<()> {
        let dotfile = self.dotfile_path();
        if !self.source_regex(when, false)?.is_match(text) {
            let message = format!("{} does not source {} integration", dotfile.display(), when);
            return Err(Error::NotInstalled(message.into()));
        }
        if !self.source_regex(when, true)?.is_match(text) {
            let position = match when {
                When::Pre => "first",
                When::Post => "last",
            };
            let message = format!(
                "{} does not source {} integration {}",
                dotfile.display(),
                when,
                position
            );
            return Err(Error::ImproperInstallation(message.into()));
        }
        Ok(())
    }

    async fn install_inner(&self) -> Result<()> {
        let dotfile = self.dotfile_path();
        let mut contents = if dotfile.exists() {
            backup_file(&dotfile, fig_util::directories::utc_backup_dir().ok())?;
            self.uninstall().await?;
            std::fs::read_to_string(&dotfile)?
        } else {
            String::new()
        };

        let original_contents = contents.clone();

        if self.pre {
            self.script_integration(When::Pre)?.install().await?;
            let (shebang, post_shebang) = split_shebang(&contents);
            contents = format!(
                "{}{}\n{}\n{}",
                shebang,
                self.description(When::Pre),
                self.source_text(When::Pre)?,
                post_shebang,
            );
        }

        if self.post {
            self.script_integration(When::Post)?.install().await?;
            contents = format!(
                "{}\n{}\n{}\n",
                contents,
                self.description(When::Post),
                self.source_text(When::Post)?,
            );
        }

        if contents.ne(&original_contents) {
            let mut file = File::create(&dotfile).with_path(self.path())?;
            file.write_all(contents.as_bytes())?;
        }
        Ok(())
    }
}

#[async_trait]
impl Integration for DotfileShellIntegration {
    fn describe(&self) -> String {
        format!(
            "{}{}{} into {}",
            self.shell,
            if self.pre { " pre" } else { "" },
            if self.post { " post" } else { "" },
            self.dotfile_name,
        )
    }

    async fn install(&self) -> Result<()> {
        if self.is_installed().await.is_ok() {
            return Ok(());
        }
        self.install_inner().await?;
        Ok(())
    }

    async fn uninstall(&self) -> Result<()> {
        let dotfile = self.dotfile_path();
        if dotfile.exists() {
            let mut contents = std::fs::read_to_string(&dotfile)?;

            if self.pre {
                contents = self.remove_from_text(&contents, When::Pre)?;
            }

            if self.post {
                contents = self.remove_from_text(&contents, When::Post)?;
            }

            contents = contents.trim().to_string();
            contents.push('\n');

            std::fs::write(&dotfile, contents.as_bytes()).with_path(self.path())?;
        }

        if self.pre {
            self.script_integration(When::Pre)?.uninstall().await?;
        }

        if self.post {
            self.script_integration(When::Post)?.uninstall().await?;
        }

        Ok(())
    }

    async fn is_installed(&self) -> Result<()> {
        let dotfile = self.dotfile_path();

        let filtered_contents: String = match std::fs::read_to_string(&dotfile).with_path(&dotfile) {
            // Remove comments and empty lines.
            Ok(contents) => {
                // Check for existence of ignore flag
                if Regex::new(r"(?mi)^\s*#\s*fig ignore\s?.*$")
                    .unwrap()
                    .is_match(&contents)
                {
                    return Ok(());
                }

                Regex::new(r"(?m)^\s*(#.*)?\n")
                    .unwrap()
                    .replace_all(&contents, "")
                    .into()
            },
            Err(Error::Io(err)) if err.kind() == ErrorKind::NotFound => {
                return Err(Error::FileDoesNotExist(dotfile.into()));
            },
            Err(err) => return Err(err),
        };

        let filtered_contents = filtered_contents.trim();

        if self.pre {
            self.matches_text(filtered_contents, When::Pre)?;
            self.script_integration(When::Pre)?.is_installed().await?;
        }

        if self.post {
            self.matches_text(filtered_contents, When::Post)?;
            self.script_integration(When::Post)?.is_installed().await?;
        }

        Ok(())
    }
}

impl ShellIntegration for DotfileShellIntegration {
    fn get_shell(&self) -> Shell {
        self.shell
    }

    fn path(&self) -> PathBuf {
        self.dotfile_path()
    }

    fn file_name(&self) -> &str {
        self.dotfile_name
    }
}

/// Splits the line containing the shebang (if any) with the rest of the string.
/// If the shebang exists, the newline is included. Otherwise, an empty slice is returned.
fn split_shebang(contents: &str) -> (&str, &str) {
    if contents.starts_with("#!") {
        match contents.find('\n') {
            Some(i) => (&contents[..i + 1], &contents[i + 1..]),
            None => ("", contents),
        }
    } else {
        ("", contents)
    }
}

#[cfg(test)]
mod test {
    #[cfg(target_os = "linux")]
    use std::io::Write;
    #[cfg(target_os = "linux")]
    use std::process::{
        Command,
        Stdio,
    };

    #[cfg(target_os = "linux")]
    use fig_util::build::SKIP_SHELLCHECK_TESTS;

    use super::*;

    #[cfg(target_os = "linux")]
    fn run_shellcheck(source: String) {
        if SKIP_SHELLCHECK_TESTS {
            return;
        }

        let shell_arg = "--shell=bash";
        let mut child = Command::new("shellcheck")
            .args([shell_arg, "--color=always", "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();

        let mut stdin = child.stdin.take().unwrap();
        std::thread::spawn(move || {
            stdin.write_all(source.as_bytes()).unwrap();
        });

        let output = child.wait_with_output().unwrap();
        if !output.status.success() {
            let stdout = String::from_utf8(output.stdout).unwrap();
            let stderr = String::from_utf8(output.stderr).unwrap();

            if !stdout.is_empty() {
                println!("{stdout}");
            }

            if !stderr.is_empty() {
                eprintln!("{stderr}");
            }

            if stdout.contains("error") {
                panic!();
            }
        }
    }

    #[test]
    fn test_split_shebang() {
        let shebang = "#!/usr/bin/env sh";
        let contents = "echo hello world";
        let with_shebang = format!("{}\n{}", shebang, contents);
        let with_shebang_no_lf = format!("{}{}", shebang, contents);
        let without_shebang = contents;
        assert_eq!(
            (format!("{shebang}\n").as_str(), contents),
            split_shebang(&with_shebang),
            "split with shebang and linefeed"
        );
        assert_eq!(
            ("", format!("{shebang}{contents}").as_str()),
            split_shebang(&with_shebang_no_lf),
            "split with shebang and no linefeed"
        );
        assert_eq!(("", contents), split_shebang(without_shebang), "split with no shebang");
    }

    // Only the path matters: these tests never read or write the dotfile.
    fn zshrc_integration() -> DotfileShellIntegration {
        DotfileShellIntegration {
            shell: Shell::Zsh,
            pre: true,
            post: true,
            dotfile_directory: PathBuf::from("/nonexistent"),
            dotfile_name: ".zshrc",
        }
    }

    fn block(integration: &DotfileShellIntegration, when: When) -> String {
        let source = integration.source_text(when).unwrap();
        format!("{}\n{source}\n", integration.description(when))
    }

    #[test]
    fn remove_from_text_strips_pre_block() {
        let integration = zshrc_integration();
        let text = format!("echo before\n{}echo after\n", block(&integration, When::Pre));
        let removed = integration.remove_from_text(text.as_str(), When::Pre).unwrap();
        assert_eq!(removed, "echo before\necho after\n");
    }

    #[test]
    fn remove_from_text_strips_post_block() {
        let integration = zshrc_integration();
        let text = format!("echo before\n{}echo after\n", block(&integration, When::Post));
        let removed = integration.remove_from_text(text.as_str(), When::Post).unwrap();
        assert_eq!(removed, "echo before\necho after\n");
    }

    #[test]
    fn matches_text_accepts_blocks_in_place() {
        let integration = zshrc_integration();
        let text = format!(
            "{}echo user\n{}",
            block(&integration, When::Pre),
            block(&integration, When::Post)
        );
        assert!(matches!(integration.matches_text(&text, When::Pre), Ok(())));
        assert!(matches!(integration.matches_text(&text, When::Post), Ok(())));
    }

    #[test]
    fn matches_text_reports_missing_block() {
        let integration = zshrc_integration();
        let text = "echo user\n";
        let pre = integration.matches_text(text, When::Pre);
        let post = integration.matches_text(text, When::Post);
        assert!(matches!(pre, Err(Error::NotInstalled(_))));
        assert!(matches!(post, Err(Error::NotInstalled(_))));
    }

    #[test]
    fn matches_text_reports_misplaced_pre_block() {
        let integration = zshrc_integration();
        let text = format!("echo user\n{}", block(&integration, When::Pre));
        let result = integration.matches_text(&text, When::Pre);
        assert!(matches!(result, Err(Error::ImproperInstallation(_))));
    }

    #[test]
    fn codewhisperer_block_is_not_recognised() {
        let integration = zshrc_integration();
        let current_source = integration.source_text(When::Pre).unwrap();
        let current_dir = format!("/{}/", fig_util::DATA_DIR_NAME);
        let old_source = current_source.replace(&current_dir, "/codewhisperer/");
        assert_ne!(old_source, current_source);
        let old_marker = "# CodeWhisperer pre block. Keep at the top of this file.";
        let text = format!("{old_marker}\n{old_source}\necho user\n");
        let result = integration.matches_text(&text, When::Pre);
        assert!(matches!(result, Err(Error::NotInstalled(_))));
        assert_eq!(integration.remove_from_text(text.as_str(), When::Pre).unwrap(), text);
    }

    #[cfg(target_os = "linux")]
    fn all_dotfile_shell_integrations() -> Vec<ShellScriptShellIntegration> {
        Shell::all()
            .iter()
            .flat_map(|shell| shell.get_script_integrations().unwrap())
            .collect()
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn dotfile_shell_integrations_snapshot() {
        for integration in all_dotfile_shell_integrations() {
            let integration_name = format!(
                "{} {}",
                integration.describe(),
                integration.path.file_name().unwrap().to_str().unwrap()
            )
            .replace(' ', "_");
            insta::assert_snapshot!(integration_name, integration.get_contents());
        }
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn dotfile_shell_integrations_shellcheck() {
        for integration in all_dotfile_shell_integrations() {
            run_shellcheck(integration.get_contents());
        }
    }
}
