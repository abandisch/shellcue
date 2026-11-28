use std::collections::HashMap;
use std::sync::LazyLock;
use std::sync::atomic::Ordering;

use anyhow::{
    Result,
    anyhow,
};
use fig_proto::local::FocusedWindowDataHook;
use fig_util::Terminal;
use tracing::debug;

use super::WM_REVICED_DATA;
use crate::event::{
    Event,
    WindowEvent,
};
use crate::platform::{
    ActiveWindowData,
    PlatformState,
};
use crate::{
    AUTOCOMPLETE_ID,
    EventLoopProxy,
};

pub static WM_CLASS_ALLOWLIST: LazyLock<HashMap<&'static str, Terminal>> = LazyLock::new(|| {
    let mut allowlist = HashMap::new();
    for terminal in fig_util::terminal::LINUX_TERMINALS {
        if let Some(wm_class) = terminal.wm_class() {
            allowlist.insert(wm_class, terminal.clone());
        }
    }
    allowlist
});

pub static GSE_ALLOWLIST: LazyLock<HashMap<&'static str, Terminal>> = LazyLock::new(|| {
    let mut allowlist = HashMap::new();
    for terminal in fig_util::terminal::LINUX_TERMINALS {
        // Using wm_class_instance here since on Wayland, (most?) terminals set the app_id equal to
        // the WM_CLASS Instance part. To handle Xwayland terminals, we still want to include wm_class as
        // well.
        if let (Some(instance), Some(class)) = (terminal.wm_class_instance(), terminal.wm_class()) {
            allowlist.insert(instance, terminal.clone());
            if class != instance {
                allowlist.insert(class, terminal.clone());
            }
        }
    }
    allowlist
});

fn from_source(from: &str) -> Option<&HashMap<&'static str, Terminal>> {
    match from {
        "wm_class" => Some(&WM_CLASS_ALLOWLIST),
        "gse" => Some(&GSE_ALLOWLIST),
        _ => None,
    }
}

pub fn from_hook(hook: FocusedWindowDataHook, platform_state: &PlatformState, proxy: &EventLoopProxy) -> Result<()> {
    debug!("Received FocusedWindowDataHook: {:?}", hook);
    WM_REVICED_DATA.store(true, Ordering::Relaxed);

    match focused_terminal(&hook)? {
        Some((terminal, data)) => {
            *platform_state.0.active_terminal.lock() = Some(terminal);
            *platform_state.0.active_window_data.lock() = Some(data);
        },
        None => {
            if !hook.hide() {
                *platform_state.0.active_terminal.lock() = None;
            }
            proxy.send_event(Event::WindowEvent {
                window_id: AUTOCOMPLETE_ID,
                window_event: WindowEvent::Hide,
            })?;
        },
    }

    Ok(())
}

/// `Ok(None)` means the popup should be hidden.
fn focused_terminal(hook: &FocusedWindowDataHook) -> Result<Option<(Terminal, ActiveWindowData)>> {
    if hook.hide() {
        return Ok(None);
    }

    debug!("focus event on {} from {}", hook.id, hook.source);
    let Some(terminal) = from_source(&hook.source)
        .ok_or_else(|| anyhow!("received invalid focus window data source"))?
        .get(hook.id.as_str())
    else {
        return Ok(None);
    };

    let inner = hook
        .inner
        .as_ref()
        .ok_or_else(|| anyhow!("focus window data without geometry"))?;
    let outer = hook
        .outer
        .as_ref()
        .ok_or_else(|| anyhow!("focus window data without geometry"))?;
    let usable = hook.scale.is_finite()
        && hook.scale > 0.0
        && inner.width >= 0
        && inner.height >= 0
        && outer.width >= 0
        && outer.height >= 0;
    if !usable {
        return Err(anyhow!("focus window data with unusable geometry"));
    }

    let data = ActiveWindowData {
        inner_x: inner.x,
        inner_y: inner.y,
        inner_width: inner.width,
        inner_height: inner.height,
        outer_x: outer.x,
        outer_y: outer.y,
        outer_width: outer.width,
        outer_height: outer.height,
        scale: hook.scale,
    };
    Ok(Some((terminal.clone(), data)))
}

#[cfg(test)]
mod tests {
    use fig_proto::local::BoundingBox;

    use super::*;

    fn bounds(x: i32) -> Option<BoundingBox> {
        Some(BoundingBox {
            x,
            y: 2,
            width: 300,
            height: 200,
        })
    }

    fn console_hook() -> FocusedWindowDataHook {
        FocusedWindowDataHook {
            source: "gse".into(),
            id: "org.gnome.Console".into(),
            inner: bounds(10),
            outer: bounds(5),
            hide: Some(false),
            scale: 2.0,
        }
    }

    #[test]
    fn listed_terminal_with_geometry_gives_window_data() {
        let (terminal, data) = focused_terminal(&console_hook()).unwrap().unwrap();
        assert_eq!(terminal, Terminal::GnomeConsole);
        assert_eq!((data.inner_x, data.outer_x, data.inner_width), (10, 5, 300));
        assert_eq!(data.scale, 2.0);
    }

    #[test]
    fn listed_terminal_without_geometry_is_an_error() {
        let hook = FocusedWindowDataHook {
            outer: None,
            ..console_hook()
        };
        assert!(focused_terminal(&hook).is_err());
        let hook = FocusedWindowDataHook {
            inner: None,
            ..console_hook()
        };
        assert!(focused_terminal(&hook).is_err());
    }

    #[test]
    fn unusable_scale_is_an_error() {
        for scale in [f32::NAN, 0.0, -1.0] {
            let hook = FocusedWindowDataHook {
                scale,
                ..console_hook()
            };
            assert!(focused_terminal(&hook).is_err(), "scale {scale}");
        }
    }

    #[test]
    fn negative_width_is_an_error() {
        let hook = FocusedWindowDataHook {
            inner: Some(BoundingBox {
                width: -1,
                ..bounds(10).unwrap()
            }),
            ..console_hook()
        };
        assert!(focused_terminal(&hook).is_err());
    }

    #[test]
    fn negative_height_is_an_error() {
        let hook = FocusedWindowDataHook {
            outer: Some(BoundingBox {
                height: -1,
                ..bounds(5).unwrap()
            }),
            ..console_hook()
        };
        assert!(focused_terminal(&hook).is_err());
    }

    #[test]
    fn unknown_source_is_an_error() {
        let hook = FocusedWindowDataHook {
            source: "unknown".into(),
            ..console_hook()
        };
        assert!(focused_terminal(&hook).is_err());
    }

    #[test]
    fn unlisted_window_hides() {
        let hook = FocusedWindowDataHook {
            id: "org.gnome.Nautilus".into(),
            ..console_hook()
        };
        assert!(focused_terminal(&hook).unwrap().is_none());
    }

    #[test]
    fn hide_flag_hides_regardless_of_the_rest() {
        let hook = FocusedWindowDataHook {
            source: "unknown".into(),
            inner: None,
            outer: None,
            hide: Some(true),
            ..console_hook()
        };
        assert!(focused_terminal(&hook).unwrap().is_none());
    }
}
