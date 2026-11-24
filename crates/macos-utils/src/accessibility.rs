use accessibility_sys::{
    AXIsProcessTrusted,
    AXIsProcessTrustedWithOptions,
    kAXTrustedCheckOptionPrompt,
};
use core_foundation::base::TCFType;
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::CFDictionary;
use core_foundation::string::CFString;
use objc2::ClassType;
use objc2_app_kit::NSWorkspace;
use objc2_foundation::{
    NSURL,
    ns_string,
};

static ACCESSIBILITY_SETTINGS_URL: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility";

pub fn open_accessibility() -> bool {
    let string = ns_string!(ACCESSIBILITY_SETTINGS_URL);
    let url = unsafe { NSURL::initWithString(NSURL::alloc(), string) };
    if let Some(url) = url {
        let workspace = unsafe { NSWorkspace::sharedWorkspace() };
        unsafe { workspace.openURL(&url) }
    } else {
        false
    }
}

pub fn accessibility_is_enabled() -> bool {
    unsafe { AXIsProcessTrusted() }
}

pub fn prompt_accessibility() -> bool {
    let key = unsafe { CFString::wrap_under_get_rule(kAXTrustedCheckOptionPrompt) };
    let options = CFDictionary::from_CFType_pairs(&[(key, CFBoolean::true_value())]);
    unsafe { AXIsProcessTrustedWithOptions(options.as_concrete_TypeRef()) }
}
