//! What the setup page shows, and which commands each state honours.

use serde::Serialize;

use super::bridge::{
    SetupAction,
    SetupCommand,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ShellStatus {
    Needed,
    Working,
    Done,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessibilityStatus {
    Needed,
    Waiting,
    Granted,
    TimedOut,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InputMethodStatus {
    NotEnabled,
    Working,
    Enabled,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ShellStep {
    pub status: ShellStatus,
    /// The integration files not yet set up, with the home directory shown as `~/`.
    pub files: Vec<String>,
    pub scripts_dir: String,
    pub error: Option<String>,
    pub command: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AccessibilityStep {
    pub status: AccessibilityStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InputMethodStep {
    pub status: InputMethodStatus,
    pub directory: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SetupView {
    pub shell: ShellStep,
    pub accessibility: AccessibilityStep,
    pub input_method: InputMethodStep,
    pub menu_bar_icon_hidden: bool,
    pub busy: bool,
}

impl SetupView {
    fn is_done(&self, action: SetupAction) -> bool {
        match action {
            SetupAction::Shell => self.shell.status == ShellStatus::Done,
            SetupAction::Accessibility => self.accessibility.status == AccessibilityStatus::Granted,
            SetupAction::InputMethod => self.input_method.status == InputMethodStatus::Enabled,
        }
    }
}

/// One action at a time, and never one whose step is done; "Not now" and close always work.
pub fn accepts(command: SetupCommand, view: &SetupView) -> bool {
    match command {
        SetupCommand::Ready | SetupCommand::NotNow | SetupCommand::Close => true,
        SetupCommand::Action(action) => !view.busy && !view.is_done(action),
    }
}

pub fn begin(view: &mut SetupView, action: SetupAction) {
    view.busy = true;
    match action {
        SetupAction::Shell => {
            view.shell.status = ShellStatus::Working;
            view.shell.error = None;
        },
        SetupAction::Accessibility => view.accessibility.status = AccessibilityStatus::Waiting,
        SetupAction::InputMethod => {
            view.input_method.status = InputMethodStatus::Working;
            view.input_method.error = None;
        },
    }
}

/// A recheck of the system cannot see a failure or a timeout, so those stay until the step is done.
pub fn keep_outcomes(previous: Option<&SetupView>, mut fresh: SetupView) -> SetupView {
    let Some(previous) = previous else {
        return fresh;
    };
    if fresh.shell.status == ShellStatus::Needed && previous.shell.status == ShellStatus::Failed {
        fresh.shell.status = ShellStatus::Failed;
        fresh.shell.error.clone_from(&previous.shell.error);
    }
    if fresh.accessibility.status == AccessibilityStatus::Needed
        && previous.accessibility.status == AccessibilityStatus::TimedOut
    {
        fresh.accessibility.status = AccessibilityStatus::TimedOut;
    }
    if fresh.input_method.status == InputMethodStatus::NotEnabled
        && previous.input_method.status == InputMethodStatus::Failed
    {
        fresh.input_method.status = InputMethodStatus::Failed;
        fresh.input_method.error.clone_from(&previous.input_method.error);
    }
    fresh
}

/// The view a finished recheck may store, or `None` when an action began or finished while it ran.
pub fn recheck_result(
    previous: Option<&SetupView>,
    fresh: SetupView,
    started: u64,
    generation: u64,
) -> Option<SetupView> {
    if started != generation || previous.is_some_and(|view| view.busy) {
        return None;
    }
    Some(keep_outcomes(previous, fresh))
}

/// At most one recheck runs; requests made while it runs collapse into one more.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recheck {
    Idle,
    Running,
    Again,
}

impl Recheck {
    /// Whether the caller should start a recheck now.
    pub fn request(&mut self) -> bool {
        let start = *self == Recheck::Idle;
        *self = if start { Recheck::Running } else { Recheck::Again };
        start
    }

    /// Whether the recheck that just ended must run once more.
    pub fn finish(&mut self) -> bool {
        let again = *self == Recheck::Again;
        *self = if again { Recheck::Running } else { Recheck::Idle };
        again
    }
}

#[cfg(test)]
mod tests {
    use super::super::bridge::update_script;
    use super::*;

    fn needed() -> SetupView {
        SetupView {
            shell: ShellStep {
                status: ShellStatus::Needed,
                files: vec!["~/.zshrc".to_owned(), "~/.zprofile".to_owned()],
                scripts_dir: "~/Library/Application Support/shellcue/shell".to_owned(),
                error: None,
                command: "shellcue setup".to_owned(),
            },
            accessibility: AccessibilityStep {
                status: AccessibilityStatus::Needed,
            },
            input_method: InputMethodStep {
                status: InputMethodStatus::NotEnabled,
                directory: "~/Library/Input Methods".to_owned(),
                error: None,
            },
            menu_bar_icon_hidden: false,
            busy: false,
        }
    }

    fn done() -> SetupView {
        let mut view = needed();
        view.shell.status = ShellStatus::Done;
        view.accessibility.status = AccessibilityStatus::Granted;
        view.input_method.status = InputMethodStatus::Enabled;
        view
    }

    const ACTIONS: [SetupAction; 3] = [SetupAction::Shell, SetupAction::Accessibility, SetupAction::InputMethod];
    const ALWAYS: [SetupCommand; 3] = [SetupCommand::Ready, SetupCommand::NotNow, SetupCommand::Close];

    #[test]
    fn view_serialises_to_pinned_json() {
        let view = SetupView {
            shell: ShellStep {
                status: ShellStatus::Failed,
                files: vec!["~/.zshrc".to_owned()],
                scripts_dir: "~/Library/Application Support/shellcue/shell".to_owned(),
                error: Some("Permission denied".to_owned()),
                command: "/Applications/ShellCue.app/Contents/MacOS/shellcue setup".to_owned(),
            },
            accessibility: AccessibilityStep {
                status: AccessibilityStatus::TimedOut,
            },
            input_method: InputMethodStep {
                status: InputMethodStatus::NotEnabled,
                directory: "~/Library/Input Methods".to_owned(),
                error: None,
            },
            menu_bar_icon_hidden: true,
            busy: false,
        };
        assert_eq!(
            serde_json::to_string(&view).unwrap(),
            concat!(
                r#"{"shell":{"status":"failed","files":["~/.zshrc"],"#,
                r#""scripts_dir":"~/Library/Application Support/shellcue/shell","#,
                r#""error":"Permission denied","#,
                r#""command":"/Applications/ShellCue.app/Contents/MacOS/shellcue setup"},"#,
                r#""accessibility":{"status":"timed_out"},"#,
                r#""input_method":{"status":"not_enabled","directory":"~/Library/Input Methods","error":null},"#,
                r#""menu_bar_icon_hidden":true,"busy":false}"#,
            )
        );
    }

    #[test]
    fn every_status_has_a_snake_case_name() {
        assert_eq!(serde_json::to_string(&ShellStatus::Needed).unwrap(), r#""needed""#);
        assert_eq!(serde_json::to_string(&ShellStatus::Working).unwrap(), r#""working""#);
        assert_eq!(serde_json::to_string(&ShellStatus::Done).unwrap(), r#""done""#);
        assert_eq!(serde_json::to_string(&AccessibilityStatus::Needed).unwrap(), r#""needed""#);
        assert_eq!(serde_json::to_string(&AccessibilityStatus::Waiting).unwrap(), r#""waiting""#);
        assert_eq!(serde_json::to_string(&AccessibilityStatus::Granted).unwrap(), r#""granted""#);
        assert_eq!(serde_json::to_string(&InputMethodStatus::Working).unwrap(), r#""working""#);
        assert_eq!(serde_json::to_string(&InputMethodStatus::Enabled).unwrap(), r#""enabled""#);
        assert_eq!(serde_json::to_string(&InputMethodStatus::Failed).unwrap(), r#""failed""#);
    }

    #[test]
    fn update_script_escapes_untrusted_text() {
        let mut view = needed();
        view.shell.error = Some("</script>\"quoted\" C:\\path\nnext".to_owned());
        let script = update_script(&view).unwrap();
        assert!(script.starts_with("window.shellcueSetup.update({"));
        assert!(script.ends_with("});"));
        assert!(script.contains(r#""error":"</script>\"quoted\" C:\\path\nnext""#));
        assert!(!script.contains('\n'));
    }

    #[test]
    fn a_fresh_view_accepts_every_command() {
        let view = needed();
        for action in ACTIONS {
            assert!(accepts(SetupCommand::Action(action), &view), "{action:?}");
        }
        for command in ALWAYS {
            assert!(accepts(command, &view), "{command:?}");
        }
    }

    #[test]
    fn done_steps_are_ignored() {
        let view = done();
        for action in ACTIONS {
            assert!(!accepts(SetupCommand::Action(action), &view), "{action:?}");
        }
        for command in ALWAYS {
            assert!(accepts(command, &view), "{command:?}");
        }

        let mut view = needed();
        view.shell.status = ShellStatus::Done;
        assert!(!accepts(SetupCommand::Action(SetupAction::Shell), &view));
        assert!(accepts(SetupCommand::Action(SetupAction::Accessibility), &view));
        assert!(accepts(SetupCommand::Action(SetupAction::InputMethod), &view));

        let mut view = needed();
        view.accessibility.status = AccessibilityStatus::Granted;
        assert!(accepts(SetupCommand::Action(SetupAction::Shell), &view));
        assert!(!accepts(SetupCommand::Action(SetupAction::Accessibility), &view));
        assert!(accepts(SetupCommand::Action(SetupAction::InputMethod), &view));

        let mut view = needed();
        view.input_method.status = InputMethodStatus::Enabled;
        assert!(accepts(SetupCommand::Action(SetupAction::Shell), &view));
        assert!(accepts(SetupCommand::Action(SetupAction::Accessibility), &view));
        assert!(!accepts(SetupCommand::Action(SetupAction::InputMethod), &view));
    }

    #[test]
    fn busy_ignores_actions_but_honours_leaving() {
        let mut view = needed();
        view.busy = true;
        for action in ACTIONS {
            assert!(!accepts(SetupCommand::Action(action), &view), "{action:?}");
        }
        for command in ALWAYS {
            assert!(accepts(command, &view), "{command:?}");
        }
    }

    #[test]
    fn failed_and_timed_out_steps_can_be_retried() {
        let mut view = needed();
        view.shell.status = ShellStatus::Failed;
        view.accessibility.status = AccessibilityStatus::TimedOut;
        view.input_method.status = InputMethodStatus::Failed;
        for action in ACTIONS {
            assert!(accepts(SetupCommand::Action(action), &view), "{action:?}");
        }
    }

    #[test]
    fn begin_marks_the_step_in_flight() {
        let mut view = needed();
        view.shell.error = Some("old".to_owned());
        begin(&mut view, SetupAction::Shell);
        assert!(view.busy);
        assert_eq!(view.shell.status, ShellStatus::Working);
        assert_eq!(view.shell.error, None);
        assert!(!accepts(SetupCommand::Action(SetupAction::Accessibility), &view));

        let mut view = needed();
        begin(&mut view, SetupAction::Accessibility);
        assert!(view.busy);
        assert_eq!(view.accessibility.status, AccessibilityStatus::Waiting);

        let mut view = needed();
        view.input_method.error = Some("old".to_owned());
        begin(&mut view, SetupAction::InputMethod);
        assert!(view.busy);
        assert_eq!(view.input_method.status, InputMethodStatus::Working);
        assert_eq!(view.input_method.error, None);
    }

    #[test]
    fn a_recheck_keeps_failures_until_done() {
        let mut previous = needed();
        previous.shell.status = ShellStatus::Failed;
        previous.shell.error = Some("Permission denied".to_owned());
        previous.accessibility.status = AccessibilityStatus::TimedOut;
        previous.input_method.status = InputMethodStatus::Failed;
        previous.input_method.error = Some("OSStatus error code: -50".to_owned());

        let kept = keep_outcomes(Some(&previous), needed());
        assert_eq!(kept.shell.status, ShellStatus::Failed);
        assert_eq!(kept.shell.error.as_deref(), Some("Permission denied"));
        assert_eq!(kept.accessibility.status, AccessibilityStatus::TimedOut);
        assert_eq!(kept.input_method.status, InputMethodStatus::Failed);
        assert_eq!(kept.input_method.error.as_deref(), Some("OSStatus error code: -50"));

        assert_eq!(keep_outcomes(Some(&previous), done()), done());
        assert_eq!(keep_outcomes(None, needed()), needed());
        assert_eq!(keep_outcomes(Some(&done()), needed()), needed());
    }

    #[test]
    fn a_stale_recheck_is_discarded() {
        assert_eq!(recheck_result(None, needed(), 0, 0), Some(needed()));
        assert_eq!(recheck_result(Some(&needed()), done(), 3, 3), Some(done()));
        assert_eq!(recheck_result(Some(&needed()), done(), 3, 4), None);
        assert_eq!(recheck_result(None, needed(), 0, 1), None);

        let mut busy = needed();
        busy.busy = true;
        assert_eq!(recheck_result(Some(&busy), done(), 3, 3), None);

        let mut failed = needed();
        failed.shell.status = ShellStatus::Failed;
        assert_eq!(
            recheck_result(Some(&failed), needed(), 1, 1).map(|view| view.shell.status),
            Some(ShellStatus::Failed)
        );
    }

    #[test]
    fn rechecks_coalesce() {
        let mut recheck = Recheck::Idle;
        assert!(recheck.request());
        assert_eq!(recheck, Recheck::Running);
        assert!(!recheck.request());
        assert!(!recheck.request());
        assert!(!recheck.request());
        assert_eq!(recheck, Recheck::Again);
        assert!(recheck.finish());
        assert_eq!(recheck, Recheck::Running);
        assert!(!recheck.finish());
        assert_eq!(recheck, Recheck::Idle);
        assert!(recheck.request());
        assert!(!recheck.finish());
        assert_eq!(recheck, Recheck::Idle);
    }
}
