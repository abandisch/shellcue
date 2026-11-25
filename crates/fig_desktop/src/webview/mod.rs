pub mod autocomplete;
pub mod notification;
pub mod window;
pub mod window_id;

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{
    Arc,
    LazyLock,
    OnceLock,
};

use cfg_if::cfg_if;
use fig_desktop_api::init_script::javascript_init;
use fig_desktop_api::kv::DashKVStore;
use fig_os_shim::Context;
use fig_proto::fig::ClientOriginatedMessage;
use fig_proto::fig::client_originated_message::Submessage;
use fig_remote_ipc::figterm::FigtermState;
use fig_util::directories;
use fnv::FnvBuildHasher;
use muda::MenuEvent;
use regex::RegexSet;
use tao::dpi::LogicalSize;
use tao::event::{
    Event as WryEvent,
    StartCause,
    WindowEvent as WryWindowEvent,
};
use tao::event_loop::{
    ControlFlow,
    EventLoopBuilder,
};
use tao::window::{
    Theme as TaoTheme,
    Window,
    WindowBuilder,
    WindowId as WryWindowId,
};
use tracing::{
    debug,
    error,
    info,
    trace,
    warn,
};
use url::Url;
use window::WindowState;
use wry::{
    Theme as WryTheme,
    WebContext,
    WebView,
    WebViewBuilder,
};

use self::notification::WebviewNotificationsState;
use crate::event::{
    Event,
    WindowEvent,
};
use crate::notification_bus::{
    JsonNotification,
    NOTIFICATION_BUS,
};
use crate::platform::{
    PlatformBoundEvent,
    PlatformState,
};
use crate::protocol::{
    api,
    icons,
    resource,
    spec,
};
use crate::remote_ipc::RemoteHook;
use crate::request::api_request;
use crate::tray::{
    self,
    build_tray,
};
use crate::webview::window_id::AutocompleteId;
pub use crate::webview::window_id::{
    AUTOCOMPLETE_ID,
    WindowId,
};
#[cfg(target_os = "macos")]
pub use crate::webview::window_id::SETUP_ID;
use crate::{
    EventLoop,
    EventLoopProxy,
    InterceptState,
    file_watcher,
    local_ipc,
    utils,
};

pub const AUTOCOMPLETE_WINDOW_TITLE: &str = "ShellCue Autocomplete";

fn map_theme(theme: &str) -> Option<WryTheme> {
    match theme {
        "dark" => Some(WryTheme::Dark),
        "light" => Some(WryTheme::Light),
        _ => None,
    }
}

pub(crate) fn to_tao_theme(theme: WryTheme) -> Option<TaoTheme> {
    match theme {
        WryTheme::Dark => Some(TaoTheme::Dark),
        WryTheme::Light => Some(TaoTheme::Light),
        WryTheme::Auto => None,
    }
}

/// The setup window takes nothing from the rest of the app but its theme.
fn delivers(window_state: &WindowState, window_event: &WindowEvent) -> bool {
    #[cfg(target_os = "macos")]
    if window_state.window_id == SETUP_ID {
        return matches!(window_event, WindowEvent::SetTheme(_));
    }
    window_state.enabled() || window_event.is_allowed_while_disabled()
}

pub static THEME: LazyLock<Option<WryTheme>> = LazyLock::new(|| {
    fig_settings::settings::get_string("app.theme")
        .ok()
        .flatten()
        .as_deref()
        .and_then(map_theme)
});

pub type FigIdMap = HashMap<WindowId, Rc<WindowState>, FnvBuildHasher>;
pub type WryIdMap = HashMap<WryWindowId, Rc<WindowState>, FnvBuildHasher>;

pub struct WebviewManager {
    fig_id_map: FigIdMap,
    window_id_map: WryIdMap,
    event_loop: EventLoop,
    figterm_state: Arc<FigtermState>,
    intercept_state: Arc<InterceptState>,
    platform_state: Arc<PlatformState>,
    notifications_state: Arc<WebviewNotificationsState>,
    dash_kv_store: Arc<DashKVStore>,
    context: Arc<Context>,
}

pub static GLOBAL_PROXY: OnceLock<EventLoopProxy> = OnceLock::new();
pub static FIGTERM_STATE: OnceLock<Arc<FigtermState>> = OnceLock::new();
pub static INTERCEPT_STATE: OnceLock<Arc<InterceptState>> = OnceLock::new();
pub static PLATFORM_STATE: OnceLock<Arc<PlatformState>> = OnceLock::new();
pub static NOTIFICATIONS_STATE: OnceLock<Arc<WebviewNotificationsState>> = OnceLock::new();
pub static DASH_KV_STORE: OnceLock<Arc<DashKVStore>> = OnceLock::new();

impl WebviewManager {
    #[allow(unused_mut)]
    pub fn new(context: Arc<Context>) -> Self {
        let mut event_loop = EventLoopBuilder::with_user_event().build();

        #[cfg(target_os = "macos")]
        {
            use tao::platform::macos::{
                ActivationPolicy,
                EventLoopExtMacOS,
            };

            event_loop.set_activation_policy(ActivationPolicy::Accessory);
        }

        let proxy = event_loop.create_proxy();
        GLOBAL_PROXY.set(proxy.clone()).unwrap();

        let figterm_state = Arc::new(FigtermState::default());
        FIGTERM_STATE.set(figterm_state.clone()).unwrap();

        let intercept_state = Arc::new(InterceptState::default());
        INTERCEPT_STATE.set(intercept_state.clone()).unwrap();

        let platform_state = Arc::new(PlatformState::new(proxy));
        PLATFORM_STATE.set(platform_state.clone()).unwrap();

        let notifications_state = Arc::new(WebviewNotificationsState::default());
        NOTIFICATIONS_STATE.set(notifications_state.clone()).unwrap();

        let dash_kv_store = Arc::new(DashKVStore::new());
        DASH_KV_STORE.set(dash_kv_store.clone()).unwrap();

        Self {
            fig_id_map: Default::default(),
            window_id_map: Default::default(),
            event_loop,
            figterm_state,
            intercept_state,
            platform_state,
            notifications_state,
            dash_kv_store,
            context,
        }
    }

    fn insert_webview(
        &mut self,
        window: Window,
        window_id: WindowId,
        webview: WebView,
        context: WebContext,
        enabled: bool,
        url: Url,
    ) {
        let webview_arc = Rc::new(WindowState::new(
            window,
            window_id.clone(),
            webview,
            context,
            enabled,
            url,
        ));
        self.fig_id_map.insert(window_id, webview_arc.clone());
        self.window_id_map.insert(webview_arc.window.id(), webview_arc);
    }

    pub fn build_webview<T>(
        &mut self,
        window_id: WindowId,
        builder: impl Fn(Arc<Context>, &mut WebContext, &EventLoop, T) -> anyhow::Result<(Window, WebView)>,
        options: T,
        enabled: bool,
        url_fn: impl Fn() -> Url,
    ) -> anyhow::Result<()> {
        let web_context_path = directories::fig_data_dir()?
            .join("webcontexts")
            .join(window_id.0.as_ref());
        let mut web_context = WebContext::new(Some(web_context_path));
        let (window, webview) = builder(Arc::clone(&self.context), &mut web_context, &self.event_loop, options)?;
        self.insert_webview(window, window_id, webview, web_context, enabled, url_fn());
        Ok(())
    }

    #[allow(unused_mut)]
    pub async fn run(mut self) -> wry::Result<()> {
        self.platform_state
            .handle(
                PlatformBoundEvent::Initialize,
                &self.event_loop,
                &self.fig_id_map,
                &self.notifications_state,
            )
            .expect("Failed to initialize platform state");

        // TODO: implement
        // tokio::spawn(figterm::clean_figterm_cache(self.figterm_state.clone()));

        // Start the local ipc task, listens for requests to the desktop socket.
        {
            let platform_state = self.platform_state.clone();
            let figterm_state = self.figterm_state.clone();
            let notifications_state = self.notifications_state.clone();
            let event_loop = self.event_loop.create_proxy();
            tokio::spawn(async move {
                match local_ipc::start_local_ipc(platform_state, figterm_state, notifications_state, event_loop).await {
                    Ok(_) => (),
                    Err(err) => error!("Unable to start local ipc: {:?}", err),
                }
            });
        }

        tokio::spawn(fig_remote_ipc::remote::start_remote_ipc(
            fig_util::directories::local_remote_socket_path().unwrap(),
            self.figterm_state.clone(),
            RemoteHook {
                notifications_state: self.notifications_state.clone(),
                proxy: self.event_loop.create_proxy(),
            },
        ));

        let (api_handler_tx, mut api_handler_rx) = tokio::sync::mpsc::unbounded_channel::<(WindowId, String)>();
        let (sync_api_handler_tx, mut sync_api_handler_rx) = tokio::sync::mpsc::unbounded_channel::<(
            WindowId,
            fig_desktop_api::error::Result<ClientOriginatedMessage>,
        )>();

        {
            let sync_proxy = self.event_loop.create_proxy();
            let sync_figterm_state = self.figterm_state.clone();
            let sync_intercept_state = self.intercept_state.clone();
            let sync_notifications_state = self.notifications_state.clone();
            let dash_kv_store = self.dash_kv_store.clone();

            tokio::spawn(async move {
                while let Some((fig_id, message)) = sync_api_handler_rx.recv().await {
                    let proxy = sync_proxy.clone();
                    let figterm_state = sync_figterm_state.clone();
                    let intercept_state = sync_intercept_state.clone();
                    let notifications_state = sync_notifications_state.clone();
                    api_request(
                        fig_id,
                        message,
                        &figterm_state,
                        &intercept_state,
                        &notifications_state,
                        &proxy.clone(),
                        &dash_kv_store,
                    )
                    .await;
                }
            });

            let proxy = self.event_loop.create_proxy();
            let figterm_state = self.figterm_state.clone();
            let intercept_state = self.intercept_state.clone();
            let notifications_state = self.notifications_state.clone();
            let dash_kv_store = self.dash_kv_store.clone();

            tokio::spawn(async move {
                while let Some((fig_id, payload)) = api_handler_rx.recv().await {
                    let message = fig_desktop_api::handler::request_from_b64(&payload);
                    if matches!(
                        message,
                        Ok(ClientOriginatedMessage {
                            id: _,
                            submessage: Some(Submessage::PositionWindowRequest(_))
                        })
                    ) {
                        sync_api_handler_tx.send((fig_id, message)).ok();
                    } else {
                        let proxy = proxy.clone();
                        let figterm_state = figterm_state.clone();
                        let intercept_state = intercept_state.clone();
                        let notifications_state = notifications_state.clone();
                        let dash_kv_store = dash_kv_store.clone();
                        tokio::spawn(async move {
                            api_request(
                                fig_id,
                                message,
                                &figterm_state,
                                &intercept_state,
                                &notifications_state,
                                &proxy.clone(),
                                &dash_kv_store,
                            )
                            .await;
                        });
                    }
                }
            });
        }

        file_watcher::setup_listeners(self.notifications_state.clone(), self.event_loop.create_proxy()).await;

        init_webview_notification_listeners(self.event_loop.create_proxy()).await;

        let tray_visible = !fig_settings::settings::get_bool_or("app.hideMenubarIcon", false);
        let (tray, launch_at_login_item) = build_tray(&self.event_loop, &self.figterm_state).await.unwrap();
        if let Err(err) = tray.set_visible(tray_visible) {
            error!(%err, "Failed to set tray visible");
        }

        let proxy = self.event_loop.create_proxy();
        proxy
            .send_event(Event::PlatformBoundEvent(PlatformBoundEvent::InitializePostRun))
            .expect("Failed to send post init event");

        self.event_loop.run(move |event, window_target, control_flow| {
            *control_flow = ControlFlow::Wait;
            trace!(?event, "Main loop event");

            if let Ok(menu_event) = MenuEvent::receiver().try_recv() {
                info!(?menu_event, "Menu Event");
                tray::handle_event(&menu_event, &proxy);
            }

            match event {
                WryEvent::NewEvents(StartCause::Init) => info!("ShellCue has started"),
                WryEvent::WindowEvent { event, window_id, .. } => {
                    if let Some(window_state) = self.window_id_map.get(&window_id) {
                        match event {
                            WryWindowEvent::CloseRequested => {
                                #[cfg(target_os = "macos")]
                                if window_state.window_id == SETUP_ID {
                                    crate::setup::close(&proxy);
                                }
                                // This is async so we need to pass 'visible' explicitly
                                window_state.window.set_visible(false);
                            },
                            WryWindowEvent::ThemeChanged(theme) => window_state.set_theme(match theme {
                                TaoTheme::Light => Some(WryTheme::Light),
                                TaoTheme::Dark => Some(WryTheme::Dark),
                                _ => None,
                            }),
                            WryWindowEvent::Focused(focused) => {
                                if focused && window_state.window_id != AUTOCOMPLETE_ID {
                                    proxy
                                        .send_event(Event::WindowEvent {
                                            window_id: AUTOCOMPLETE_ID,
                                            window_event: WindowEvent::Hide,
                                        })
                                        .unwrap();
                                }
                                #[cfg(target_os = "macos")]
                                if focused && window_state.window_id == SETUP_ID {
                                    crate::setup::refresh(&proxy);
                                }
                            },
                            _ => (),
                        }
                    }
                },
                WryEvent::UserEvent(event) => {
                    match event {
                        Event::WindowEvent {
                            window_id,
                            window_event,
                        } => match self.fig_id_map.get(&window_id) {
                            Some(window_state) => {
                                if delivers(window_state, &window_event) {
                                    window_state.handle(
                                        window_event,
                                        &self.figterm_state,
                                        &self.platform_state,
                                        &self.notifications_state,
                                        window_target,
                                        &api_handler_tx,
                                    );
                                } else {
                                    trace!(
                                        window_id =% window_state.window_id,
                                        ?window_event,
                                        "Ignoring event for disabled window"
                                    );
                                }
                            },
                            None => {
                                // TODO(grant): figure out how to handle this gracefully
                                warn!("No window {window_id} available for event");
                                trace!(?window_event, "Event");
                            },
                        },
                        Event::WindowEventAll { window_event } => {
                            for (_window_id, window_state) in self.window_id_map.iter() {
                                if delivers(window_state, &window_event) {
                                    window_state.handle(
                                        window_event.clone(),
                                        &self.figterm_state,
                                        &self.platform_state,
                                        &self.notifications_state,
                                        window_target,
                                        &api_handler_tx,
                                    );
                                } else {
                                    trace!(
                                        window_id =% window_state.window_id,
                                        ?window_event,
                                        "Ignoring event for disabled window"
                                    );
                                }
                            }
                        },
                        Event::ControlFlow(new_control_flow) => {
                            *control_flow = new_control_flow;
                        },
                        Event::ReloadAccessibility => {
                            // tray.set_menu(Some(Box::new(get_context_menu())));

                            let autocomplete_enabled =
                                !fig_settings::settings::get_bool_or("autocomplete.disable", false)
                                    && PlatformState::accessibility_is_enabled().unwrap_or(true);

                            proxy
                                .send_event(Event::WindowEvent {
                                    window_id: AUTOCOMPLETE_ID,
                                    window_event: WindowEvent::SetEnabled(autocomplete_enabled),
                                })
                                .unwrap();
                        },
                        Event::SetTrayVisible(visible) => {
                            if let Err(err) = tray.set_visible(visible) {
                                error!(%err, "Failed to set tray visible");
                            }
                        },
                        Event::SetLaunchAtLoginChecked(checked) => {
                            if let Some(item) = &launch_at_login_item {
                                item.set_checked(checked);
                            }
                        },
                        Event::PlatformBoundEvent(native_event) => {
                            if let Err(err) = self.platform_state.handle(
                                native_event,
                                window_target,
                                &self.fig_id_map,
                                &self.notifications_state,
                            ) {
                                debug!(%err, "Failed to handle native event");
                            }
                        },
                        #[cfg(target_os = "macos")]
                        Event::Setup(setup_event) => match setup_event {
                            crate::setup::SetupEvent::Show => {
                                let created = !self.fig_id_map.contains_key(&SETUP_ID);
                                if created {
                                    match crate::setup::build_window(window_target, proxy.clone()) {
                                        Ok(window_state) => {
                                            let window_state = Rc::new(window_state);
                                            self.fig_id_map.insert(SETUP_ID, window_state.clone());
                                            self.window_id_map.insert(window_state.window.id(), window_state);
                                        },
                                        Err(err) => error!(%err, "Failed to build the setup window"),
                                    }
                                }
                                if let Some(window_state) = self.fig_id_map.get(&SETUP_ID) {
                                    // `set_focus` acts only on a visible window.
                                    window_state.window.set_visible(true);
                                    window_state.window.set_focus();
                                    // A new page asks for its view once loaded.
                                    if !created {
                                        crate::setup::refresh(&proxy);
                                    }
                                }
                            },
                            crate::setup::SetupEvent::Command(command) => {
                                crate::setup::handle_command(command, &proxy);
                            },
                            crate::setup::SetupEvent::Push(view) => match self.fig_id_map.get(&SETUP_ID) {
                                Some(window_state) => match crate::setup::update_script(&view) {
                                    Ok(script) => {
                                        if let Err(err) = window_state.webview.evaluate_script(&script) {
                                            warn!(%err, "Failed to update the setup page");
                                        }
                                    },
                                    Err(err) => error!(%err, "Failed to serialise the setup view"),
                                },
                                None => debug!("Setup window closed; view not shown"),
                            },
                            crate::setup::SetupEvent::Close => {
                                if let Some(window_state) = self.fig_id_map.remove(&SETUP_ID) {
                                    self.window_id_map.remove(&window_state.window.id());
                                    window_state.window.set_visible(false);
                                }
                            },
                        },
                    }
                },
                #[cfg(target_os = "macos")]
                WryEvent::Reopen { .. } => crate::setup::show(crate::setup::Trigger::Reopen, &proxy),
                WryEvent::MainEventsCleared | WryEvent::NewEvents(StartCause::WaitCancelled { .. }) => {},
                event => trace!(?event, "Unhandled event"),
            }
        });
    }
}

fn navigation_handler<I, S>(window_id: WindowId, exprs: I) -> impl Fn(String) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let regex_set = RegexSet::new(exprs);

    if let Err(ref err) = regex_set {
        error!("Failed to compile regex: {err}");
    }

    move |url: String| match regex_set.as_ref().ok().and_then(|r| {
        Url::parse(&url)
            .ok()
            .and_then(|url| url.domain().map(|domain| r.is_match(domain)))
    }) {
        Some(true) => {
            trace!("{window_id} allowed url: {url}");
            true
        },
        Some(false) | None => {
            warn!("{window_id} denied url: {url}");
            false
        },
    }
}

pub struct AutocompleteOptions;

pub fn build_autocomplete(
    ctx: Arc<Context>,
    web_context: &mut WebContext,
    event_loop: &EventLoop,
    _autocomplete_options: AutocompleteOptions,
) -> anyhow::Result<(Window, WebView)> {
    let mut window_builder = WindowBuilder::new()
        .with_title(AUTOCOMPLETE_WINDOW_TITLE)
        .with_transparent(true)
        .with_decorations(false)
        .with_always_on_top(true)
        .with_focused(false)
        .with_window_icon(Some(utils::icon()))
        .with_inner_size(LogicalSize::new(1.0, 1.0))
        .with_theme(THEME.and_then(to_tao_theme));

    cfg_if!(
        if #[cfg(target_os = "linux")] {
            use tao::platform::unix::WindowBuilderExtUnix;
            window_builder = window_builder.with_resizable(true).with_skip_taskbar(true);
        } else if #[cfg(target_os = "macos")] {
            use tao::platform::macos::WindowBuilderExtMacOS;
            window_builder = window_builder.with_resizable(false).with_has_shadow(false).with_visible(false);
        }
    );

    let window = window_builder.build(event_loop)?;

    #[cfg(target_os = "linux")]
    {
        use gtk::gdk::WindowTypeHint;
        use gtk::traits::{
            GtkWindowExt,
            WidgetExt,
        };
        use tao::platform::unix::WindowExtUnix;

        let gtk_window = window.gtk_window();
        gtk_window.set_role("autocomplete");
        gtk_window.set_type_hint(WindowTypeHint::Utility);
        gtk_window.set_accept_focus(false);
        gtk_window.set_decorated(false);
        if let Some(window) = gtk_window.window() {
            window.set_override_redirect(true);
        }
    }

    let proxy = event_loop.create_proxy();

    let webview_builder = WebViewBuilder::with_web_context(web_context)
        .with_url(autocomplete::url().as_str())
        .with_ipc_handler(move |payload| {
            proxy
                .send_event(Event::WindowEvent {
                    window_id: AUTOCOMPLETE_ID.clone(),
                    window_event: WindowEvent::Api {
                        payload: payload.into_body(),
                    },
                })
                .unwrap();
        })
        .with_asynchronous_custom_protocol(
            "fig".into(),
            utils::wrap_custom_protocol(Arc::clone(&ctx), "fig", AutocompleteId, icons::handle),
        )
        .with_asynchronous_custom_protocol(
            "icon".into(),
            utils::wrap_custom_protocol(Arc::clone(&ctx), "icon", AutocompleteId, icons::handle),
        )
        .with_asynchronous_custom_protocol(
            "spec".into(),
            utils::wrap_custom_protocol(Arc::clone(&ctx), "spec", AutocompleteId, spec::handle),
        )
        .with_asynchronous_custom_protocol(
            "qcliresource".into(),
            utils::wrap_custom_protocol(
                Arc::clone(&ctx),
                "qcliresource::Autocomplete",
                AutocompleteId,
                resource::handle::<resource::Autocomplete>,
            ),
        )
        .with_asynchronous_custom_protocol(
            "api".into(),
            utils::wrap_custom_protocol(Arc::clone(&ctx), "api", AutocompleteId, api::handle),
        )
        .with_devtools(true)
        .with_transparent(true)
        .with_initialization_script(&javascript_init(true))
        .with_navigation_handler(navigation_handler(AUTOCOMPLETE_ID, &[r"localhost$", r"^127\.0\.0\.1$"]))
        .with_clipboard(true)
        .with_hotkeys_zoom(true)
        .with_accept_first_mouse(true);

    cfg_if! {
        if #[cfg(target_os = "linux")] {
            use tao::platform::unix::WindowExtUnix;
            use wry::WebViewBuilderExtUnix;
            let vbox = window.default_vbox().unwrap();
            let webview = webview_builder.build_gtk(vbox)?;
        } else {
            let webview = webview_builder.build(&window)?;
        }
    };

    Ok((window, webview))
}

async fn init_webview_notification_listeners(proxy: EventLoopProxy) {
    #[allow(unused_macros)]
    macro_rules! watcher {
        ($type:ident, $name:expr, $on_update:expr) => {{
            paste::paste! {
                let proxy = proxy.clone();
                tokio::spawn(async move {
                    let mut rx = NOTIFICATION_BUS.[<subscribe_ $type>]($name.into());
                    loop {
                        let res = rx.recv().await;
                        match res {
                            Ok(val) => {
                                #[allow(clippy::redundant_closure_call)]
                                ($on_update)(val, &proxy);
                            },
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                                warn!("Notification bus '{}' lagged by {n} messages", $name);
                            },
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                        }
                    }
                });
            }
        };};
    }

    #[cfg(target_os = "linux")]
    {
        use fig_integrations::Integration;
        use fig_integrations::desktop_entry::{
            AutostartIntegration,
            should_install_autostart_entry,
        };
        use fig_settings::{
            Settings,
            State,
        };
        use fig_util::consts::{
            LAUNCH_ON_STARTUP_DEFAULT,
            LAUNCH_ON_STARTUP_KEY,
        };

        use crate::notification_bus::JsonNotification;
        watcher!(
            settings,
            "autocomplete.disable",
            |notification: JsonNotification, proxy: &EventLoopProxy| {
                let enabled = !notification.as_bool().unwrap_or(false);
                debug!(%enabled, "Autocomplete");
                proxy
                    .send_event(Event::WindowEvent {
                        window_id: AUTOCOMPLETE_ID,
                        window_event: WindowEvent::SetEnabled(enabled),
                    })
                    .unwrap();
            }
        );
        watcher!(
            settings,
            LAUNCH_ON_STARTUP_KEY,
            |notification: JsonNotification, _proxy: &EventLoopProxy| {
                let enabled = notification.as_bool().unwrap_or(LAUNCH_ON_STARTUP_DEFAULT);
                debug!(%enabled, "{}", LAUNCH_ON_STARTUP_KEY);
                tokio::spawn(async move {
                    let ctx = Context::new();
                    let settings = Settings::new();
                    let state = State::new();
                    let autostart = match AutostartIntegration::new(&ctx) {
                        Ok(autostart) => autostart,
                        Err(err) => {
                            error!(
                                ?err,
                                "failed to update the autostart integration installed status to {}", enabled
                            );
                            return;
                        },
                    };
                    if should_install_autostart_entry(&ctx, &settings, &state) {
                        autostart
                            .install()
                            .await
                            .map_err(|err| warn!(?err, "unable to install autostart integration"))
                            .ok();
                    } else {
                        autostart
                            .uninstall()
                            .await
                            .map_err(|err| warn!(?err, "unable to uninstall autostart integration"))
                            .ok();
                    }
                });
            }
        );
    }

    watcher!(
        settings,
        "app.theme",
        |notification: JsonNotification, proxy: &EventLoopProxy| {
            let theme = notification.as_string().as_deref().and_then(map_theme);
            debug!(?theme, "Theme changed");
            proxy
                .send_event(Event::WindowEventAll {
                    window_event: WindowEvent::SetTheme(theme),
                })
                .unwrap();
        }
    );

    watcher!(
        settings,
        "app.hideMenubarIcon",
        |notification: JsonNotification, proxy: &EventLoopProxy| {
            let enabled = !notification.as_bool().unwrap_or(false);
            debug!(%enabled, "Tray icon");
            proxy.send_event(Event::SetTrayVisible(enabled)).unwrap();
        }
    );

    watcher!(
        settings,
        "developer.autocomplete.build",
        |_notification: JsonNotification, proxy: &EventLoopProxy| {
            let url = autocomplete::url();
            debug!(%url, "Autocomplete host");
            proxy
                .send_event(Event::WindowEvent {
                    window_id: AUTOCOMPLETE_ID,
                    window_event: WindowEvent::NavigateAbsolute { url },
                })
                .unwrap();
        }
    );
}
