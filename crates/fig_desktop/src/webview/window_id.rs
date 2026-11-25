use std::borrow::Cow;
use std::fmt;

pub const AUTOCOMPLETE_ID: WindowId = WindowId(Cow::Borrowed("autocomplete"));
#[cfg(target_os = "macos")]
pub const SETUP_ID: WindowId = WindowId(Cow::Borrowed("setup"));

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WindowId(pub Cow<'static, str>);

impl fmt::Display for WindowId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl serde::Serialize for WindowId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

pub trait WindowIdProvider {
    fn window_id(&self) -> WindowId;
}

impl WindowIdProvider for WindowId {
    fn window_id(&self) -> WindowId {
        self.clone()
    }
}

#[derive(Debug, Clone, Copy)]
pub struct AutocompleteId;

impl WindowIdProvider for AutocompleteId {
    fn window_id(&self) -> WindowId {
        AUTOCOMPLETE_ID
    }
}
