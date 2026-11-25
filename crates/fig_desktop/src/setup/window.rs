//! The setup window and its webview, which get none of the autocomplete protocols or API.

use fig_util::consts::PRODUCT_NAME;
use tao::dpi::LogicalSize;
use tao::window::WindowBuilder;
use tracing::warn;
use url::Url;
use wry::http::Request;
use wry::{
    WebContext,
    WebViewBuilder,
};

use super::SetupEvent;
use super::assets;
use super::bridge::{
    PAGE_URL,
    SCHEME,
    is_setup_origin,
    parse_command,
};
use crate::event::Event;
use crate::webview::window::WindowState;
use crate::webview::{
    SETUP_ID,
    THEME,
    to_tao_theme,
};
use crate::{
    EventLoopProxy,
    EventLoopWindowTarget,
    utils,
};

pub fn build_window(window_target: &EventLoopWindowTarget, proxy: EventLoopProxy) -> anyhow::Result<WindowState> {
    let url = Url::parse(PAGE_URL)?;
    let window = WindowBuilder::new()
        .with_title(format!("Set up {PRODUCT_NAME}"))
        .with_inner_size(LogicalSize::new(720.0, 760.0))
        .with_resizable(false)
        .with_maximizable(false)
        .with_visible(false)
        .with_window_icon(Some(utils::icon()))
        .with_theme(THEME.and_then(to_tao_theme))
        .build(window_target)?;

    let mut context = WebContext::new(None);
    let webview = WebViewBuilder::with_web_context(&mut context)
        .with_url(PAGE_URL)
        .with_incognito(true)
        .with_devtools(false)
        .with_custom_protocol(SCHEME.into(), |_webview_id, request| assets::respond(&request))
        .with_ipc_handler(move |request: Request<String>| forward(&proxy, &request))
        .with_navigation_handler(|url: String| {
            let allowed = is_setup_origin(&url);
            if !allowed {
                warn!(%url, "Setup window navigation blocked");
            }
            allowed
        })
        .with_new_window_req_handler(|url: String| {
            warn!(%url, "Setup window new window blocked");
            false
        })
        .build(&window)?;

    Ok(WindowState::new(window, SETUP_ID, webview, context, true, url))
}

fn forward(proxy: &EventLoopProxy, request: &Request<String>) {
    let origin = request.uri().to_string();
    if !is_setup_origin(&origin) {
        warn!(%origin, "Setup message from another origin dropped");
        return;
    }
    let Some(command) = parse_command(request.body()) else {
        warn!(len = request.body().len(), "Unknown setup message dropped");
        return;
    };
    if proxy.send_event(Event::Setup(SetupEvent::Command(command))).is_err() {
        warn!("Event loop closed; setup command dropped");
    }
}
