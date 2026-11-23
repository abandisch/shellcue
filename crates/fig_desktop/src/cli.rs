use clap::Parser;
use fig_util::consts::APP_PROCESS_NAME;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None, name = APP_PROCESS_NAME)]
pub struct Cli {
    /// Checks the `app.launchOnStartup` setting before launching
    #[arg(long)]
    pub is_startup: bool,
    /// Kill old instances of `fig_desktop`
    #[arg(long)]
    pub kill_old: bool,
    /// Allow launching multiple instances of `fig_desktop`
    #[arg(long)]
    pub allow_multiple: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verify_app() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }

    #[test]
    fn command_name() {
        use clap::CommandFactory;
        assert_eq!(Cli::command().get_name(), APP_PROCESS_NAME);
    }

    #[test]
    fn removed_arguments_are_rejected() {
        assert!(Cli::try_parse_from(["fig_desktop", "--no-dashboard"]).is_err());
        assert!(Cli::try_parse_from(["fig_desktop", "q://dashboard/"]).is_err());
    }
}
