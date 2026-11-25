use std::borrow::Cow;

use cfg_if::cfg_if;
use fig_remote_ipc::figterm::FigtermState;
use fig_settings::{
    JsonStore,
    OldSettings,
};
use fig_util::consts::PRODUCT_NAME;
use fig_util::directories;
use muda::{
    CheckMenuItem,
    CheckMenuItemBuilder,
    IconMenuItem,
    Menu,
    MenuEvent,
    MenuId,
    MenuItemBuilder,
    PredefinedMenuItem,
    Submenu,
};
use tao::event_loop::ControlFlow;
use tracing::{
    trace,
    warn,
};
use tray_icon::{
    Icon,
    TrayIcon,
    TrayIconBuilder,
};

use crate::event::Event;
use crate::launch_at_login::{
    launch_at_login_enabled,
    toggle_launch_at_login,
};
use crate::{
    EventLoopProxy,
    EventLoopWindowTarget,
};

// macro_rules! icon {
//     ($icon:literal) => {{
//         #[cfg(target_os = "macos")]
//         {
//             Some(include_bytes!(concat!(
//                 env!("TRAY_ICONS_PROCESSED"),
//                 "/",
//                 $icon,
//                 ".png"
//             )))
//         }
//         #[cfg(not(target_os = "macos"))]
//         {
//             None
//         }
//     }};
// }

pub fn handle_event(menu_event: &MenuEvent, proxy: &EventLoopProxy) {
    match &*menu_event.id().0 {
        "quit" => {
            proxy.send_event(Event::ControlFlow(ControlFlow::Exit)).unwrap();
        },
        #[cfg(target_os = "macos")]
        "setup" => crate::setup::show(crate::setup::Trigger::Menu, proxy),
        "settings" => {
            tokio::runtime::Handle::current().spawn(open_settings_file());
        },
        "launch-at-login" => launch_at_login_clicked(proxy),
        id => {
            trace!(?id, "Unhandled tray event");
        },
    }
}

/// muda flips the tick on click; it is reset from the real state once the change is done.
fn launch_at_login_clicked(proxy: &EventLoopProxy) {
    let proxy = proxy.clone();
    tokio::runtime::Handle::current().spawn_blocking(move || match toggle_launch_at_login() {
        Ok(enabled) => send_launch_at_login(&proxy, enabled),
        Err(err) => {
            warn!("Failed to change launch at login: {err:#}");
            show_launch_at_login(&proxy);
        },
    });
}

/// Sets the tray tick to the real state; blocks on file and settings reads.
pub fn show_launch_at_login(proxy: &EventLoopProxy) {
    send_launch_at_login(proxy, launch_at_login_enabled());
}

fn send_launch_at_login(proxy: &EventLoopProxy, enabled: bool) {
    if proxy.send_event(Event::SetLaunchAtLoginChecked(enabled)).is_err() {
        warn!("Event loop closed; launch at login tick not updated");
    }
}

async fn open_settings_file() {
    // Creates the file as `{}` when it does not exist yet.
    if let Err(err) = OldSettings::load_from_file() {
        warn!(%err, "Failed to load the settings file");
    }

    let path = match directories::settings_path() {
        Ok(path) => path,
        Err(err) => {
            warn!(%err, "Failed to get the settings path");
            return;
        },
    };

    cfg_if! {
        if #[cfg(target_os = "macos")] {
            let opener = "open";
        } else {
            let opener = "xdg-open";
        }
    }

    match tokio::process::Command::new(opener).arg(&path).status().await {
        Ok(status) if status.success() => {},
        Ok(status) => warn!(%status, ?path, "Failed to open the settings file"),
        Err(err) => warn!(%err, ?path, "Failed to open the settings file"),
    }
}

#[allow(dead_code)]
#[cfg(target_os = "linux")]
fn load_icon(path: impl AsRef<std::path::Path>) -> Option<Icon> {
    let image = image::open(path).ok()?.into_rgba8();
    let (width, height) = image.dimensions();
    let rgba = image.into_raw();
    Icon::from_rgba(rgba, width, height).ok()
}

pub async fn build_tray(
    _event_loop_window_target: &EventLoopWindowTarget,
    _figterm_state: &FigtermState,
) -> tray_icon::Result<(TrayIcon, Option<CheckMenuItem>)> {
    let (menu, launch_at_login) = get_context_menu();
    let tray = TrayIconBuilder::new()
        .with_icon(get_icon())
        .with_icon_as_template(true)
        .with_menu(Box::new(menu))
        .build()?;
    Ok((tray, launch_at_login))
}

pub fn get_icon() -> Icon {
    let (icon_rgba, icon_width, icon_height) = {
        let bytes = {
            cfg_if! {
                if #[cfg(target_os = "linux")] {
                    include_bytes!("../icons/icon-monochrome-light.png").to_vec()
                } else {
                    include_bytes!("../icons/icon-monochrome.png").to_vec()
                }
            }
        };
        let image = image::load_from_memory(&bytes)
            .expect("Failed to open icon path")
            .into_rgba8();
        let (width, height) = image.dimensions();
        let rgba = image.into_raw();
        (rgba, width, height)
    };
    Icon::from_rgba(icon_rgba, icon_width, icon_height).expect("Failed to open icon")
}

/// The menu, and its launch at login item so the tick can follow changes made elsewhere.
pub fn get_context_menu() -> (Menu, Option<CheckMenuItem>) {
    let mut tray_menu = Menu::new();
    let mut launch_at_login = None;

    let elements = menu(launch_at_login_enabled());
    for elem in elements {
        if let Some(item) = elem.add_to_menu(&mut tray_menu) {
            launch_at_login = Some(item);
        }
    }

    (tray_menu, launch_at_login)
}

enum MenuElement {
    Info {
        image_icon: Option<muda::Icon>,
        text: Cow<'static, str>,
    },
    Entry {
        emoji_icon: Option<Cow<'static, str>>,
        image_icon: Option<muda::Icon>,
        text: Cow<'static, str>,
        id: Cow<'static, str>,
    },
    Check {
        text: Cow<'static, str>,
        id: Cow<'static, str>,
        checked: bool,
    },
    Separator,
    #[allow(dead_code)]
    SubMenu {
        title: Cow<'static, str>,
        elements: Vec<MenuElement>,
    },
}

impl MenuElement {
    fn info(image_icon: Option<(Vec<u8>, u32, u32)>, text: impl Into<Cow<'static, str>>) -> Self {
        Self::Info {
            image_icon: image_icon.and_then(|(bytes, width, height)| muda::Icon::from_rgba(bytes, width, height).ok()),
            text: text.into(),
        }
    }

    fn entry(
        emoji_icon: Option<Cow<'static, str>>,
        image_icon: Option<(Vec<u8>, u32, u32)>,
        text: impl Into<Cow<'static, str>>,
        id: impl Into<Cow<'static, str>>,
    ) -> Self {
        Self::Entry {
            emoji_icon,
            image_icon: image_icon.and_then(|(bytes, width, height)| muda::Icon::from_rgba(bytes, width, height).ok()),
            text: text.into(),
            id: id.into(),
        }
    }

    fn check(text: impl Into<Cow<'static, str>>, id: impl Into<Cow<'static, str>>, checked: bool) -> Self {
        Self::Check {
            text: text.into(),
            id: id.into(),
            checked,
        }
    }

    fn check_item(text: &str, id: &str, checked: bool) -> CheckMenuItem {
        CheckMenuItemBuilder::new()
            .text(text)
            .id(MenuId::new(id))
            .enabled(true)
            .checked(checked)
            .build()
    }

    // fn sub_menu(title: impl Into<Cow<'static, str>>, elements: Vec<MenuElement>) -> Self {
    //     Self::SubMenu {
    //         title: title.into(),
    //         elements,
    //     }
    // }

    /// Returns the item when it is a check item.
    fn add_to_menu(&self, menu: &mut Menu) -> Option<CheckMenuItem> {
        match self {
            MenuElement::Info { text, image_icon } => {
                let menu_item = IconMenuItem::new(
                    text,
                    false,
                    image_icon.clone(), // Some(muda::Icon::from_rgba(bytes, width, height).unwrap()),
                    None,
                );
                menu.append(&menu_item).unwrap();
            },
            MenuElement::Entry {
                emoji_icon,
                image_icon,
                text,
                id,
                ..
            } => {
                let text = match (std::env::consts::OS, emoji_icon) {
                    ("linux", Some(emoji_icon)) => format!("{emoji_icon} {text}"),
                    _ => text.to_string(),
                };
                let menu_item = muda::IconMenuItemBuilder::new()
                    .text(text)
                    .id(MenuId::new(id))
                    .enabled(true)
                    .icon(image_icon.clone())
                    .build();
                menu.append(&menu_item).unwrap();
            },
            MenuElement::Check { text, id, checked } => {
                let menu_item = Self::check_item(text, id, *checked);
                menu.append(&menu_item).unwrap();
                return Some(menu_item);
            },
            MenuElement::Separator => {
                menu.append(&PredefinedMenuItem::separator()).unwrap();
            },
            MenuElement::SubMenu { title, elements } => {
                let sub_menu = Submenu::new(title, true);
                for element in elements {
                    element.add_to_submenu(&sub_menu);
                }

                menu.append(&sub_menu).unwrap();
            },
        }
        None
    }

    fn add_to_submenu(&self, submenu: &Submenu) {
        match self {
            MenuElement::Info { image_icon, text } => {
                // menu.append(MenuItemAttributes::new(info).with_enabled(false));
                let menu_item = IconMenuItem::new(
                    text,
                    false,
                    image_icon.clone(), // Some(muda::Icon::from_rgba(bytes, width, height).unwrap()),
                    None,
                );
                submenu.append(&menu_item).unwrap();
            },
            MenuElement::Entry {
                emoji_icon, text, id, ..
            } => {
                let text: String = match (std::env::consts::OS, emoji_icon) {
                    ("linux", Some(emoji_icon)) => format!("{emoji_icon} {text}"),
                    _ => text.to_string(),
                };
                let menu_item = MenuItemBuilder::new()
                    .text(text)
                    .id(MenuId::new(id))
                    .enabled(true)
                    .build();
                submenu.append(&menu_item).unwrap();
            },
            MenuElement::Check { text, id, checked } => {
                submenu.append(&Self::check_item(text, id, *checked)).unwrap();
            },
            MenuElement::Separator => {
                submenu.append(&PredefinedMenuItem::separator()).unwrap();
            },
            MenuElement::SubMenu { title, elements } => {
                let sub_menu = Submenu::new(title, true);
                for element in elements {
                    element.add_to_submenu(&sub_menu);
                }

                submenu.append(&sub_menu).unwrap();
            },
        }
    }
}

fn menu(launch_at_login: bool) -> Vec<MenuElement> {
    cfg_if! {
        if #[cfg(target_os = "macos")] {
            let setup = Some(MenuElement::entry(None, None, format!("Set up {PRODUCT_NAME}…"), "setup"));
        } else {
            let setup = None;
        }
    }
    setup
        .into_iter()
        .chain([
            MenuElement::entry(None, None, "Settings", "settings"),
            MenuElement::check("Launch at login", "launch-at-login", launch_at_login),
            MenuElement::Separator,
            MenuElement::info(None, format!("Version: {}", env!("CARGO_PKG_VERSION"))),
            MenuElement::Separator,
            MenuElement::entry(None, None, format!("Quit {PRODUCT_NAME}"), "quit"),
        ])
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_shape_is_fixed() {
        let elements = menu(false);
        let shape: Vec<&str> = elements
            .iter()
            .map(|element| match element {
                MenuElement::Entry { id, .. } | MenuElement::Check { id, .. } => &**id,
                MenuElement::Info { .. } => "info",
                MenuElement::Separator => "---",
                MenuElement::SubMenu { .. } => "submenu",
            })
            .collect();
        #[cfg(target_os = "macos")]
        assert_eq!(shape, ["setup", "settings", "launch-at-login", "---", "info", "---", "quit"]);
        #[cfg(not(target_os = "macos"))]
        assert_eq!(shape, ["settings", "launch-at-login", "---", "info", "---", "quit"]);
    }

    #[test]
    fn launch_at_login_tick_shows_the_state() {
        for state in [false, true] {
            let elements = menu(state);
            let checks: Vec<(&str, &str, bool)> = elements
                .iter()
                .filter_map(|element| match element {
                    MenuElement::Check { text, id, checked } => Some((&**text, &**id, *checked)),
                    _ => None,
                })
                .collect();
            assert_eq!(checks, [("Launch at login", "launch-at-login", state)]);
        }
    }
}
