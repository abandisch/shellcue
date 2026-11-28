use fig_proto::fig::{
    NotificationRequest,
    NotificationType,
};

use super::{
    RequestResult,
    RequestResultImpl,
};
use crate::WebviewNotificationsState;
use crate::webview::WindowId;

pub async fn handle_request(
    request: NotificationRequest,
    window_id: WindowId,
    message_id: i64,
    state: &WebviewNotificationsState,
) -> RequestResult {
    let Some(notification_type) = request.r#type else {
        return RequestResult::error("Notification type is missing");
    };
    let Ok(notification_type) = NotificationType::try_from(notification_type) else {
        return RequestResult::error(format!("Unknown notification type {notification_type}"));
    };

    if request.subscribe.unwrap_or(true) {
        subscribe(window_id, message_id, notification_type, state)
    } else {
        unsubscribe(&window_id, notification_type, state)
    }
}

fn subscribe(
    window_id: WindowId,
    channel: i64,
    notification_type: NotificationType,
    state: &WebviewNotificationsState,
) -> RequestResult {
    if notification_type == NotificationType::All {
        return RequestResult::error("Cannot subscribe to All notification type");
    }

    let entry = state.subscriptions.entry(window_id).or_default();
    if entry.contains_key(&notification_type) {
        return RequestResult::error(format!("Already subscribed to notification type {notification_type:?}",));
    }

    entry.insert(notification_type, channel);

    RequestResult::success()
}

fn unsubscribe(
    window_id: &WindowId,
    notification_type: NotificationType,
    state: &WebviewNotificationsState,
) -> RequestResult {
    if notification_type == NotificationType::All {
        return unsubscribe_all(window_id, state);
    }

    match state.subscriptions.get(window_id) {
        Some(subscriptions) if !subscriptions.contains_key(&notification_type) => {
            return RequestResult::error(format!("Not subscribed notification type {notification_type:?}",));
        },
        None => {
            return RequestResult::error(format!("Not subscribed notification type {notification_type:?}",));
        },
        Some(subscriptions) => {
            subscriptions.remove(&notification_type);
        },
    }

    RequestResult::success()
}

fn unsubscribe_all(window_id: &WindowId, state: &WebviewNotificationsState) -> RequestResult {
    if let Some(subscriptions) = state.subscriptions.get(window_id) {
        subscriptions.clear();
    }

    RequestResult::success()
}

#[cfg(test)]
mod tests {
    use fig_proto::fig::server_originated_message::Submessage as ServerOriginatedSubMessage;

    use super::*;
    use crate::AUTOCOMPLETE_ID;

    async fn error_message(r#type: Option<i32>) -> Option<String> {
        let request = NotificationRequest {
            subscribe: Some(true),
            r#type,
        };
        let state = WebviewNotificationsState::default();
        match *handle_request(request, AUTOCOMPLETE_ID, 0, &state).await.ok()? {
            ServerOriginatedSubMessage::Error(message) => Some(message),
            _ => None,
        }
    }

    #[tokio::test]
    async fn missing_type_is_an_error() {
        assert_eq!(
            error_message(None).await.as_deref(),
            Some("Notification type is missing")
        );
    }

    #[tokio::test]
    async fn unknown_type_is_an_error() {
        for value in [-1, 9, 12, i32::MAX] {
            assert_eq!(
                error_message(Some(value)).await,
                Some(format!("Unknown notification type {value}")),
                "{value}"
            );
        }
    }
}
