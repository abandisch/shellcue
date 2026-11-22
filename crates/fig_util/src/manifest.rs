use std::fmt::Display;
use std::str::FromStr;
use std::sync::OnceLock;

use cfg_if::cfg_if;
use serde::{
    Deserialize,
    Deserializer,
    Serialize,
};
use strum::{
    Display,
    EnumString,
};
use crate::build::TARGET_TRIPLE;

#[derive(Deserialize)]
pub struct Manifest {
    #[serde(deserialize_with = "deser_enum_other")]
    pub managed_by: ManagedBy,
    #[serde(deserialize_with = "deser_enum_other")]
    pub target_triple: TargetTriple,
    pub packaged_at: String,
    pub packaged_by: String,
}

#[derive(EnumString, Display, Deserialize, Serialize, PartialEq, Eq, Clone, Debug)]
#[serde(rename_all = "camelCase")]
#[strum(serialize_all = "camelCase")]
pub enum ManagedBy {
    None,
    #[strum(default)]
    Other(String),
}

/// The target triplet, describes a platform on which the project is build for. Note that this also
/// includes "fake" targets like `universal-apple-darwin` as provided by [Tauri](https://tauri.app/v1/guides/building/macos/#binary-targets)
#[derive(Deserialize, Serialize, PartialEq, Eq, EnumString, Debug, Clone, Display)]
pub enum TargetTriple {
    #[serde(rename = "universal-apple-darwin")]
    #[strum(serialize = "universal-apple-darwin")]
    UniversalAppleDarwin,
    #[serde(rename = "x86_64-unknown-linux-gnu")]
    #[strum(serialize = "x86_64-unknown-linux-gnu")]
    X86_64UnknownLinuxGnu,
    #[serde(rename = "x86_64-unknown-linux-musl")]
    #[strum(serialize = "x86_64-unknown-linux-musl")]
    X86_64UnknownLinuxMusl,
    #[serde(rename = "aarch64-unknown-linux-gnu")]
    #[strum(serialize = "aarch64-unknown-linux-gnu")]
    AArch64UnknownLinuxGnu,
    #[serde(rename = "aarch64-unknown-linux-musl")]
    #[strum(serialize = "aarch64-unknown-linux-musl")]
    AArch64UnknownLinuxMusl,
    #[serde(rename = "x86_64-pc-windows-msvc")]
    #[strum(serialize = "x86_64-pc-windows-msvc")]
    X86_64PcWindowsMsvc,
    #[serde(rename = "i686-pc-windows-msvc")]
    #[strum(serialize = "i686-pc-windows-msvc")]
    I686PcWindowsMsvc,
    #[serde(rename = "aarch64-pc-windows-msvc")]
    #[strum(serialize = "aarch64-pc-windows-msvc")]
    AArch64PcWindowsMsvc,
    #[strum(default)]
    Other(String),
}

impl TargetTriple {
    const fn from_system() -> Self {
        cfg_if! {
            if #[cfg(target_os = "macos")] {
                TargetTriple::UniversalAppleDarwin
            } else if #[cfg(all(target_os = "linux", target_env = "gnu", target_arch = "x86_64"))] {
                TargetTriple::X86_64UnknownLinuxGnu
            } else if #[cfg(all(target_os = "linux", target_env = "gnu", target_arch = "aarch64"))] {
                TargetTriple::AArch64UnknownLinuxGnu
            } else if #[cfg(all(target_os = "linux", target_env = "musl", target_arch = "x86_64"))] {
                TargetTriple::X86_64UnknownLinuxMusl
            } else if #[cfg(all(target_os = "linux", target_env = "musl", target_arch = "aarch64"))] {
                TargetTriple::AArch64UnknownLinuxMusl
            } else if #[cfg(all(target_os = "windows", target_arch = "x86_64"))] {
                TargetTriple::X86_64PcWindowsMsvc
            } else if #[cfg(all(target_os = "windows", target_arch = "x86"))] {
                TargetTriple::I686PcWindowsMsvc
            } else if #[cfg(all(target_os = "windows", target_arch = "aarch64"))] {
                TargetTriple::AArch64PcWindowsMsvc
            } else {
                compile_error!("unknown target")
            }
        }
    }
}

#[derive(EnumString, Display, Deserialize, Serialize, PartialEq, Eq, Clone, Debug)]
#[serde(rename_all = "camelCase")]
#[strum(serialize_all = "camelCase")]
pub enum Os {
    Macos,
    Linux,
    Windows,
    #[strum(default)]
    Other(String),
}

impl Os {
    pub fn current() -> Self {
        match std::env::consts::OS {
            "macos" => Os::Macos,
            "linux" => Os::Linux,
            "windows" => Os::Windows,
            _ => panic!("Unsupported OS: {}", std::env::consts::OS),
        }
    }

    pub fn is_current_os(&self) -> bool {
        self == &Os::current()
    }
}

#[derive(EnumString, Display, Deserialize, Serialize, PartialEq, Eq, Clone, Debug)]
#[serde(rename_all = "camelCase")]
#[strum(serialize_all = "camelCase")]
pub enum FileType {
    Dmg,
    TarGz,
    TarXz,
    TarZst,
    Zip,
    AppImage,
    Deb,
    #[strum(default)]
    Other(String),
}

fn deser_enum_other<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: FromStr,
    T::Err: Display,
{
    match T::from_str(<&str as Deserialize<'de>>::deserialize(deserializer)?) {
        Ok(s) => Ok(s),
        Err(err) => Err(serde::de::Error::custom(err)),
    }
}

/// Returns the manifest, reading and parsing it if necessary
pub fn manifest() -> &'static Manifest {
    static CACHED: OnceLock<Manifest> = OnceLock::new();
    CACHED.get_or_init(|| Manifest {
        managed_by: ManagedBy::None,
        target_triple: match TARGET_TRIPLE {
            Some(target) => TargetTriple::from_str(target).expect("parsing target triple should not fail"),
            _ => TargetTriple::from_system(),
        },
        packaged_at: "unknown".into(),
        packaged_by: "unknown".into(),
    })
}

/// Gets the version from the manifest
#[deprecated = "versions are unified, use env!(\"CARGO_PKG_VERSION\")"]
pub fn version() -> Option<&'static str> {
    Some(env!("CARGO_PKG_VERSION"))
}

#[cfg(test)]
mod tests {
    use serde_json::{
        from_str,
        to_string,
    };

    use super::*;

    macro_rules! test_ser_deser {
        ($ty:ident, $variant:expr, $text:expr) => {
            let quoted = format!("\"{}\"", $text);
            assert_eq!(quoted, to_string(&$variant).unwrap());
            assert_eq!($variant, from_str(&quoted).unwrap());
            assert_eq!($variant, $ty::from_str($text).unwrap());
            assert_eq!($text, $variant.to_string());
        };
    }

    #[test]
    fn test_target_triple_serialize_deserialize() {
        test_ser_deser!(
            TargetTriple,
            TargetTriple::UniversalAppleDarwin,
            "universal-apple-darwin"
        );
        test_ser_deser!(
            TargetTriple,
            TargetTriple::X86_64UnknownLinuxGnu,
            "x86_64-unknown-linux-gnu"
        );
        test_ser_deser!(
            TargetTriple,
            TargetTriple::AArch64UnknownLinuxGnu,
            "aarch64-unknown-linux-gnu"
        );
        test_ser_deser!(
            TargetTriple,
            TargetTriple::X86_64UnknownLinuxMusl,
            "x86_64-unknown-linux-musl"
        );
        test_ser_deser!(
            TargetTriple,
            TargetTriple::AArch64UnknownLinuxMusl,
            "aarch64-unknown-linux-musl"
        );
    }

    #[test]
    fn test_file_type_serialize_deserialize() {
        test_ser_deser!(FileType, FileType::Dmg, "dmg");
        test_ser_deser!(FileType, FileType::TarGz, "tarGz");
        test_ser_deser!(FileType, FileType::TarXz, "tarXz");
        test_ser_deser!(FileType, FileType::TarZst, "tarZst");
        test_ser_deser!(FileType, FileType::Zip, "zip");
        test_ser_deser!(FileType, FileType::AppImage, "appImage");
        test_ser_deser!(FileType, FileType::Deb, "deb");
    }

    #[test]
    fn test_managed_by_serialize_deserialize() {
        test_ser_deser!(ManagedBy, ManagedBy::None, "none");
    }
}
