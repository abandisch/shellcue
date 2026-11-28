use std::time::Duration;

use fig_proto::fig::server_originated_message::Submessage as ServerOriginatedSubMessage;
use fig_proto::fig::{
    Duration as ProtoDuration,
    RunProcessRequest,
    RunProcessResponse,
};
use fig_proto::fig_common::{
    RUN_PROCESS_TIMEOUT_LIMIT,
    capped_duration,
};
use fig_proto::remote::hostbound;
use fig_remote_ipc::figterm::{
    FigtermCommand,
    FigtermState,
};
use tokio::time::timeout;
use tracing::debug;
use uuid::Uuid;

use super::RequestResult;

const MIN_RESPONSE_WAIT: Duration = Duration::from_secs(10);

/// The timeout shellcue-term is given and how long to wait for its answer; both capped.
fn timeouts(requested: Option<&ProtoDuration>) -> (Option<Duration>, Duration) {
    let timeout = requested.map(|t| capped_duration(t, RUN_PROCESS_TIMEOUT_LIMIT));
    (timeout, timeout.unwrap_or_default().max(MIN_RESPONSE_WAIT))
}

/// Programs only ever run inside the named, linked terminal session, never in this process.
pub async fn run(request: RunProcessRequest, state: &FigtermState) -> RequestResult {
    debug!({
        term_session =? request.terminal_session_id,
        exe = request.executable,
        args =? request.arguments,
        cwd = request.working_directory(),
        env =? request.env,
        timeout =? request.timeout,
    }, "Running command");

    let session_id = request
        .terminal_session_id
        .as_deref()
        .ok_or("terminal_session_id is required")?;
    let uuid = Uuid::parse_str(session_id)
        .map_err(|err| format!("terminal_session_id is not a valid UUID: {err}"))?;
    let session_sender = state
        .with(&uuid, |session| session.sender.clone())
        .ok_or("terminal session is not linked")?;

    let (requested_timeout, timeout_duration) = timeouts(request.timeout.as_ref());

    let (message, rx) = FigtermCommand::run_process(
        request.executable,
        request.arguments,
        request.working_directory,
        request.env,
        requested_timeout,
    );
    session_sender
        .send(message)
        .map_err(|err| format!("failed sending command to shellcue-term: {err}"))?;
    drop(session_sender);

    let response = timeout(timeout_duration, rx)
        .await
        .map_err(|_err| "Timed out waiting for shellcue-term response")?
        .map_err(|err| format!("Failed to receive shellcue-term response: {err}"))?;

    if let hostbound::response::Response::RunProcess(response) = response {
        RequestResult::Ok(Box::new(ServerOriginatedSubMessage::RunProcessResponse(
            RunProcessResponse { ..response },
        )))
    } else {
        Err("invalid response type".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn refusal(terminal_session_id: Option<&str>) -> String {
        let request = RunProcessRequest {
            executable: "/usr/bin/true".into(),
            terminal_session_id: terminal_session_id.map(Into::into),
            ..Default::default()
        };
        run(request, &FigtermState::new()).await.unwrap_err().to_string()
    }

    fn secs(secs: u64) -> Duration {
        Duration::from_secs(secs)
    }

    fn timeouts_for(secs: u64, nanos: u32) -> (Option<Duration>, Duration) {
        timeouts(Some(&ProtoDuration { secs, nanos }))
    }

    #[test]
    fn response_wait_has_a_floor() {
        assert_eq!(timeouts(None), (None, secs(10)));
        assert_eq!(timeouts_for(5, 0), (Some(secs(5)), secs(10)));
        assert_eq!(timeouts_for(30, 0), (Some(secs(30)), secs(30)));
    }

    #[test]
    fn timeouts_are_capped_without_panicking() {
        assert_eq!(timeouts_for(u64::MAX, u32::MAX), (Some(secs(60)), secs(60)));
    }

    #[tokio::test]
    async fn missing_session_id_is_refused() {
        assert_eq!(refusal(None).await, "terminal_session_id is required");
    }

    #[tokio::test]
    async fn invalid_session_id_is_refused() {
        for id in ["", "not-a-uuid", "../../etc"] {
            let message = refusal(Some(id)).await;
            assert!(
                message.starts_with("terminal_session_id is not a valid UUID"),
                "{id:?}: {message}"
            );
        }
    }

    #[tokio::test]
    async fn unlinked_session_is_refused() {
        let id = Uuid::new_v4().to_string();
        assert_eq!(refusal(Some(&id)).await, "terminal session is not linked");
    }
}
