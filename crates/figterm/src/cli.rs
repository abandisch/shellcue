use clap::Parser;
use fig_util::PTY_BINARY_NAME;

#[derive(Debug, Parser, PartialEq, Eq)]
#[command(version, about, name = PTY_BINARY_NAME)]
pub struct Cli {
    #[arg(last = true)]
    pub command: Option<Vec<String>>,
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
        assert_eq!(Cli::command().get_name(), "shellcue-term");
    }

    #[test]
    fn test_command() {
        let cli = Cli::parse_from(["figterm"]);
        assert_eq!(cli, Cli { command: None });

        let cli = Cli::parse_from(["figterm", "--", "exe", "arg1", "arg2"]);
        assert_eq!(cli, Cli {
            command: Some(vec!["exe".into(), "arg1".into(), "arg2".into()])
        });
    }
}
