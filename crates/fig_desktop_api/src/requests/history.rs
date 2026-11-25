use fig_proto::fig::server_originated_message::Submessage as ServerOriginatedSubMessage;
use fig_proto::fig::{
    HistoryQueryRequest,
    HistoryQueryResponse,
};
use fig_settings::history::History;

use super::RequestResult;

/// The only history query the page can trigger; the JSON row keys are what the page parses.
const HISTORY_QUERY: &str = "SELECT command, shell, session_id, cwd, start_time, exit_code FROM history";

pub async fn query(_request: HistoryQueryRequest) -> RequestResult {
    let history = History::new();
    let results = history
        .query(HISTORY_QUERY)
        .map_err(|err| format!("Failed querying history: {err}"))?;

    let json_array =
        serde_json::to_string(&results).map_err(|err| format!("Failed serializing history query results: {err}"))?;

    let response = ServerOriginatedSubMessage::HistoryQueryResponse(HistoryQueryResponse { json_array });
    Ok(response.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_history_query_is_fixed_select() {
        assert_eq!(
            HISTORY_QUERY,
            "SELECT command, shell, session_id, cwd, start_time, exit_code FROM history"
        );
        assert!(HISTORY_QUERY.starts_with("SELECT "));
        assert!(HISTORY_QUERY.ends_with(" FROM history"));
        assert!(!HISTORY_QUERY.contains('?'));
        assert!(!HISTORY_QUERY.contains(':'));
        assert!(!HISTORY_QUERY.contains('@'));
        assert!(!HISTORY_QUERY.contains('$'));
        assert!(!HISTORY_QUERY.contains(';'));
    }
}
