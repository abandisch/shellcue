//! The setup page's only channel to the app: a fixed set of tokens in, a typed view out.

use url::Url;

use super::view::SetupView;

pub const SCHEME: &str = "shellcuesetup";
pub const HOST: &str = "localhost";
pub const PAGE_URL: &str = "shellcuesetup://localhost/";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupAction {
    Shell,
    Accessibility,
    InputMethod,
    LaunchAtLogin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupCommand {
    Ready,
    Action(SetupAction),
    NotNow,
    Close,
}

const TOKENS: [(&str, SetupCommand); 7] = [
    ("ready", SetupCommand::Ready),
    ("set-up-shell", SetupCommand::Action(SetupAction::Shell)),
    (
        "request-accessibility",
        SetupCommand::Action(SetupAction::Accessibility),
    ),
    ("enable-input-method", SetupCommand::Action(SetupAction::InputMethod)),
    (
        "enable-launch-at-login",
        SetupCommand::Action(SetupAction::LaunchAtLogin),
    ),
    ("not-now", SetupCommand::NotNow),
    ("close", SetupCommand::Close),
];

/// Exact, case-sensitive matches only.
pub fn parse_command(message: &str) -> Option<SetupCommand> {
    TOKENS
        .iter()
        .find(|(token, _)| *token == message)
        .map(|(_, command)| *command)
}

/// Used for both the sender of a message and every navigation.
pub fn is_setup_origin(url: &str) -> bool {
    Url::parse(url).is_ok_and(|url| {
        url.scheme() == SCHEME
            && url.username().is_empty()
            && url.password().is_none()
            && url.host_str() == Some(HOST)
            && url.port().is_none()
    })
}

/// The only script the app runs in the setup page; the view is data, never script text.
pub fn update_script(view: &SetupView) -> serde_json::Result<String> {
    serde_json::to_string(view).map(|json| format!("window.shellcueSetup.update({json});"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_command_accepts_every_token() {
        assert_eq!(parse_command("ready"), Some(SetupCommand::Ready));
        assert_eq!(
            parse_command("set-up-shell"),
            Some(SetupCommand::Action(SetupAction::Shell))
        );
        assert_eq!(
            parse_command("request-accessibility"),
            Some(SetupCommand::Action(SetupAction::Accessibility))
        );
        assert_eq!(
            parse_command("enable-input-method"),
            Some(SetupCommand::Action(SetupAction::InputMethod))
        );
        assert_eq!(
            parse_command("enable-launch-at-login"),
            Some(SetupCommand::Action(SetupAction::LaunchAtLogin))
        );
        assert_eq!(parse_command("not-now"), Some(SetupCommand::NotNow));
        assert_eq!(parse_command("close"), Some(SetupCommand::Close));
    }

    #[test]
    fn parse_command_rejects_near_misses() {
        for message in [
            "",
            "Ready",
            "READY",
            "Set-Up-Shell",
            " ready",
            "ready ",
            "\tclose",
            "ready\n",
            "close\0",
            "not_now",
            "notnow",
            "\"ready\"",
            "{\"command\":\"ready\"}",
            "[\"close\"]",
            "ready;close",
        ] {
            assert_eq!(parse_command(message), None, "{message:?}");
        }
    }

    #[test]
    fn launch_at_login_token_is_exact() {
        assert_eq!(
            parse_command("enable-launch-at-login"),
            Some(SetupCommand::Action(SetupAction::LaunchAtLogin))
        );
        for message in [
            "launch-at-login",
            "enable-launch_at_login",
            "Enable-Launch-At-Login",
            "enable-launch-at-login ",
            "disable-launch-at-login",
        ] {
            assert_eq!(parse_command(message), None, "{message:?}");
        }
    }

    #[test]
    fn parse_command_rejects_long_input() {
        assert_eq!(parse_command(&"ready".repeat(100_000)), None);
        assert_eq!(parse_command(&format!("close{}", " ".repeat(100_000))), None);
    }

    #[test]
    fn setup_origin_accepts_the_page() {
        assert!(is_setup_origin("shellcuesetup://localhost/"));
        assert!(is_setup_origin("shellcuesetup://localhost/index.html"));
        assert!(is_setup_origin(PAGE_URL));
    }

    #[test]
    fn setup_origin_rejects_everything_else() {
        for url in [
            "http://localhost/",
            "https://localhost/",
            "qcliresource://localhost/",
            "api://localhost/",
            "fig://localhost/",
            "shellcuesetup://evil/",
            "shellcuesetup://localhost.evil/",
            "shellcuesetup://localhost:8080/",
            "shellcuesetup://x@localhost/",
            "shellcuesetup://x:y@localhost/",
            "shellcuesetup://localhost@evil/",
            "shellcuesetup://a.localhost/",
            "shellcuesetup://localhost./",
            "shellcuesetup://LOCALHOST/",
            "shellcuesetup:/localhost/",
            "about:blank",
            "data:text/html,hello",
            "javascript:alert(1)",
            "file:///etc/passwd",
            "",
        ] {
            assert!(!is_setup_origin(url), "{url:?}");
        }
    }

    #[test]
    fn navigation_stays_on_the_page() {
        assert!(is_setup_origin("shellcuesetup://localhost/index.html#step-2"));
        assert!(!is_setup_origin("https://example.com/"));
        assert!(!is_setup_origin("shellcuesetup://example.com/index.html"));
        assert!(!is_setup_origin("blob:shellcuesetup://localhost/1234"));
    }
}
