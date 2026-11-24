use std::sync::Mutex;

use fig_proto::local::command_response::Response as CommandResponseTypes;
use fig_proto::local::dump_state_command::Type as DumpStateType;
use fig_proto::local::{
    DebugModeCommand,
    DiagnosticsCommand,
    DiagnosticsResponse,
    DumpStateCommand,
    DumpStateResponse,
    LogLevelCommand,
    LogLevelResponse,
    QuitCommand,
};
use fig_remote_ipc::figterm::FigtermState;
use tao::event_loop::ControlFlow;

use super::{
    LocalResponse,
    LocalResult,
};
use crate::event::{
    Event,
    WindowEvent,
};
use crate::platform::PlatformState;
use crate::webview::notification::WebviewNotificationsState;
use crate::{
    AUTOCOMPLETE_ID,
    EventLoopProxy,
    platform,
};

pub async fn debug(command: DebugModeCommand, proxy: &EventLoopProxy) -> LocalResult {
    static DEBUG_MODE: Mutex<bool> = Mutex::new(false);

    let debug_mode = match command.set_debug_mode {
        Some(b) => {
            *DEBUG_MODE.lock().unwrap() = b;
            b
        },
        None => match command.toggle_debug_mode {
            Some(true) => {
                let mut locked_debug = DEBUG_MODE.lock().unwrap();
                *locked_debug = !*locked_debug;
                *locked_debug
            },
            _ => *DEBUG_MODE.lock().unwrap(),
        },
    };

    proxy
        .send_event(Event::WindowEvent {
            window_id: AUTOCOMPLETE_ID.clone(),
            window_event: WindowEvent::DebugMode(debug_mode),
        })
        .unwrap();

    Ok(LocalResponse::Success(None))
}

pub async fn quit(_: QuitCommand, proxy: &EventLoopProxy) -> LocalResult {
    proxy
        .send_event(Event::ControlFlow(ControlFlow::Exit))
        .map(|_| LocalResponse::Success(None))
        .map_err(|_err| {
            #[allow(clippy::exit)]
            std::process::exit(0)
        })
}

pub async fn diagnostic(_: DiagnosticsCommand, figterm_state: &FigtermState) -> LocalResult {
    let (edit_buffer_string, edit_buffer_cursor, shell_context, intercept_enabled, intercept_global_enabled) = {
        match figterm_state.most_recent() {
            Some(session) => (
                Some(session.edit_buffer.text.clone()),
                Some(session.edit_buffer.cursor),
                session.context.clone(),
                Some(session.intercept.into()),
                Some(session.intercept_global.into()),
            ),
            None => (None, None, None, None, None),
        }
    };

    let response = DiagnosticsResponse {
        autocomplete_active: Some(platform::autocomplete_active()),
        #[cfg(target_os = "macos")]
        path_to_bundle: macos_utils::bundle::get_bundle_path()
            .and_then(|path| path.to_str().map(|s| s.to_owned()))
            .unwrap_or_default(),
        #[cfg(target_os = "macos")]
        accessibility: if macos_utils::accessibility::accessibility_is_enabled() {
            "true".into()
        } else {
            "false".into()
        },

        edit_buffer_string,
        edit_buffer_cursor,
        shell_context,
        intercept_enabled,
        intercept_global_enabled,

        ..Default::default()
    };

    Ok(LocalResponse::Message(Box::new(CommandResponseTypes::Diagnostics(
        response,
    ))))
}

pub async fn prompt_for_accessibility_permission() -> LocalResult {
    cfg_if::cfg_if! {
        if #[cfg(target_os = "macos")] {
            use std::sync::atomic::{
                AtomicBool,
                Ordering,
            };

            use macos_utils::accessibility::{
                accessibility_is_enabled,
                open_accessibility,
                prompt_accessibility,
            };

            // macOS may show its dialog only once, so later requests open the pane.
            static PROMPTED: AtomicBool = AtomicBool::new(false);

            if !accessibility_is_enabled() {
                if PROMPTED.swap(true, Ordering::Relaxed) {
                    open_accessibility();
                } else {
                    prompt_accessibility();
                }
            }
            Ok(LocalResponse::Success(None))
        } else {
            Err(LocalResponse::Error {
                code: None,
                message: Some("Accessibility API not supported on this platform".to_owned()),
            })
        }
    }
}

pub fn log_level(LogLevelCommand { level }: LogLevelCommand) -> LocalResult {
    let old_level = fig_log::set_log_level(level).map_err(|err| LocalResponse::Error {
        code: None,
        message: Some(format!("Error setting log level: {err}")),
    })?;

    Ok(LocalResponse::Message(Box::new(CommandResponseTypes::LogLevel(
        LogLevelResponse {
            old_level: Some(old_level),
        },
    ))))
}

pub fn dump_state(
    command: DumpStateCommand,
    figterm_state: &FigtermState,
    webview_notifications_state: &WebviewNotificationsState,
    platform_state: &PlatformState,
) -> LocalResult {
    let json = match command.r#type() {
        DumpStateType::DumpStateFigterm => {
            serde_json::to_string_pretty(&figterm_state).unwrap_or_else(|err| format!("unable to dump: {err}"))
        },
        DumpStateType::DumpStateWebNotifications => serde_json::to_string_pretty(&webview_notifications_state)
            .unwrap_or_else(|err| format!("unable to dump: {err}")),
        DumpStateType::DumpStatePlatform => {
            serde_json::to_string_pretty(&platform_state).unwrap_or_else(|err| format!("unable to dump: {err}"))
        },
    };

    LocalResult::Ok(LocalResponse::Message(Box::new(CommandResponseTypes::DumpState(
        DumpStateResponse { json },
    ))))
}

#[allow(unused_variables)]
pub async fn connect_to_ibus(proxy: EventLoopProxy, platform_state: &PlatformState) -> LocalResult {
    cfg_if::cfg_if! {
        if #[cfg(target_os = "linux")] {
            use crate::platform::ibus::launch_ibus_connection;
            match launch_ibus_connection(proxy, platform_state.inner()).await {
                Ok(_) => Ok(LocalResponse::Success(None)),
                Err(err) => {
                    Err(LocalResponse::Error {
                        code: None,
                        message: Some(format!("Failed connecting to ibus: {:?}", err)),
                    })
                },
            }
        } else {
            Err(LocalResponse::Error {
                code: None,
                message: Some("Connecting to IBus is only supported on Linux".to_owned()),
            })
        }
    }
}
