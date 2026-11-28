use std::env::consts;

use camino::Utf8PathBuf;
use fig_os_shim::Context;
use fig_util::directories;
use serde::Serialize;
use serde_json::json;

/// Everything the page is told about the machine; nothing else belongs here.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Constants {
    home: Option<Utf8PathBuf>,
    themes_folder: Option<Utf8PathBuf>,
    os: &'static str,
    new_uri_format: bool,
}

impl Constants {
    fn new() -> Self {
        let ctx = Context::new();
        Self {
            home: directories::home_dir_utf8().ok(),
            themes_folder: directories::themes_dir(&ctx)
                .ok()
                .and_then(|dir| Utf8PathBuf::try_from(dir).ok()),
            os: consts::OS,
            new_uri_format: true,
        }
    }
}

impl Constants {
    pub fn script(&self) -> String {
        format!("fig.constants = {};", json!(self))
    }
}

pub fn javascript_init() -> String {
    [
        r#"if (!window.fig || !window.fig.quiet) console.log("[fig] declaring constants...");"#.into(),
        "if (!window.fig) window.fig = {};".into(),
        "if (!window.fig.constants) fig.constants = {};".into(),
        Constants::new().script(),
    ]
    .join("\n")
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    const KEPT_KEYS: [&str; 4] = ["home", "themesFolder", "os", "newUriFormat"];

    fn assert_kept_keys(value: &serde_json::Value) {
        let keys: BTreeSet<&str> = value
            .as_object()
            .expect("constants are an object")
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(keys, BTreeSet::from(KEPT_KEYS));
    }

    #[test]
    fn constants_serialise_exactly_the_kept_keys() {
        let constants = Constants {
            home: Some(Utf8PathBuf::from("/home/user")),
            themes_folder: Some(Utf8PathBuf::from("/usr/share/shellcue/themes")),
            os: "linux",
            new_uri_format: true,
        };
        assert_kept_keys(&serde_json::to_value(&constants).unwrap());
    }

    #[test]
    fn init_script_declares_no_environment() {
        let script = javascript_init();
        let json = script
            .lines()
            .find_map(|line| line.strip_prefix("fig.constants = "))
            .and_then(|line| line.strip_suffix(';'))
            .expect("script declares the constants");
        let value: serde_json::Value = serde_json::from_str(json).unwrap();
        assert!(value.get("env").is_none());
        assert_kept_keys(&value);
    }
}
