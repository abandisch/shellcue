use std::time::Duration;

use base64::prelude::*;
use fig_os_shim::{
    ContextArcProvider,
    ContextProvider,
    EnvProvider,
    FsProvider,
};
pub use fig_proto::fig::client_originated_message::Submessage as ClientOriginatedSubMessage;
pub use fig_proto::fig::server_originated_message::Submessage as ServerOriginatedSubMessage;
use fig_proto::fig::{
    ClientOriginatedMessage,
    InsertTextRequest,
    NotificationRequest,
    PositionWindowRequest,
    RunProcessRequest,
    ServerOriginatedMessage,
    UpdateApplicationPropertiesRequest,
};
use fig_proto::prost::Message;
use fig_settings::settings::SettingsProvider;
use fig_settings::state::StateProvider;
use tracing::warn;

use crate::error::Result;
use crate::kv::KVStore;
use crate::requests::{
    self,
    RequestResult,
    RequestResultImpl,
};

pub struct Wrapped<Ctx, Req> {
    pub message_id: i64,
    pub context: Ctx,
    pub request: Req,
}

#[async_trait::async_trait]
pub trait EventHandler {
    type Ctx: KVStore + EnvProvider + FsProvider + Sync + Send;

    async fn notification(&self, request: Wrapped<Self::Ctx, NotificationRequest>) -> RequestResult {
        RequestResult::unimplemented(request.request)
    }

    async fn insert_text(&self, request: Wrapped<Self::Ctx, InsertTextRequest>) -> RequestResult {
        RequestResult::unimplemented(request.request)
    }

    async fn position_window(&self, request: Wrapped<Self::Ctx, PositionWindowRequest>) -> RequestResult {
        RequestResult::unimplemented(request.request)
    }

    async fn run_process(&self, request: Wrapped<Self::Ctx, RunProcessRequest>) -> RequestResult {
        RequestResult::unimplemented(request.request)
    }

    async fn update_application_properties(
        &self,
        request: Wrapped<Self::Ctx, UpdateApplicationPropertiesRequest>,
    ) -> RequestResult {
        RequestResult::unimplemented(request.request)
    }
}

pub fn request_from_b64(request_b64: &str) -> Result<ClientOriginatedMessage> {
    let data = BASE64_STANDARD.decode(request_b64)?;
    Ok(ClientOriginatedMessage::decode(data.as_slice())?)
}

pub fn response_to_b64(response_message: ServerOriginatedMessage) -> String {
    BASE64_STANDARD.encode(response_message.encode_to_vec())
}

pub async fn api_request<Ctx, E>(
    event_handler: E,
    ctx: Ctx,
    request: ClientOriginatedMessage,
) -> Result<ServerOriginatedMessage>
where
    Ctx: KVStore + SettingsProvider + StateProvider + ContextProvider + ContextArcProvider + Send + Sync,
    E: EventHandler<Ctx = Ctx> + Sync,
{
    let request_id = match request.id {
        Some(request_id) => request_id,
        None => return Err(crate::error::Error::NoMessageId),
    };

    let response = match tokio::time::timeout(
        Duration::from_secs(60),
        handle_request(event_handler, ctx, request_id, request),
    )
    .await
    {
        Ok(response) => response,
        Err(_) => return Err(crate::error::Error::Timeout),
    };

    Ok(ServerOriginatedMessage {
        id: Some(request_id),
        submessage: Some(match response {
            Ok(msg) => *msg,
            Err(msg) => ServerOriginatedSubMessage::Error(msg.to_string()),
        }),
    })
}

async fn handle_request<Ctx, E>(
    event_handler: E,
    ctx: Ctx,
    message_id: i64,
    message: ClientOriginatedMessage,
) -> RequestResult
where
    Ctx: KVStore + SettingsProvider + StateProvider + ContextProvider + ContextArcProvider + Send + Sync,
    E: EventHandler<Ctx = Ctx> + Sync,
{
    macro_rules! request {
        ($request:expr) => {
            Wrapped {
                message_id,
                context: ctx,
                request: $request,
            }
        };
    }

    match message.submessage {
        Some(submessage) => {
            use ClientOriginatedSubMessage::{
                GetLocalStateRequest,
                GetSettingsPropertyRequest,
                HistoryQueryRequest,
                InsertTextRequest,
                NotificationRequest,
                PositionWindowRequest,
                ReadFileRequest,
                RunProcessRequest,
                UpdateApplicationPropertiesRequest,
            };
            use requests::*;

            match submessage {
                // figterm
                InsertTextRequest(request) => event_handler.insert_text(request!(request)).await,
                // fs
                ReadFileRequest(request) => fs::read_file(request, ctx.env(), ctx.fs()).await,
                // notifications
                NotificationRequest(request) => event_handler.notification(request!(request)).await,
                // process
                RunProcessRequest(request) => event_handler.run_process(request!(request)).await,
                // properties
                UpdateApplicationPropertiesRequest(request) => {
                    event_handler.update_application_properties(request!(request)).await
                },
                // state
                GetLocalStateRequest(request) => state::get(request).await,
                // settings
                GetSettingsPropertyRequest(request) => settings::get(request).await,
                // window
                PositionWindowRequest(request) => event_handler.position_window(request!(request)).await,
                // history
                HistoryQueryRequest(request) => history::query(request).await,
            }
        },
        None => {
            warn!("Missing submessage: {message:?}");
            RequestResult::error("Missing submessage")
        },
    }
}
