import pathlib


APP_NAME = "ShellCue"
CLI_BINARY_NAME = "shellcue"
PTY_BINARY_NAME = "shellcue-term"
DESKTOP_BINARY_NAME = "shellcue-desktop"
TAURI_PRODUCT_NAME = "shellcue-desktop"
LINUX_PACKAGE_NAME = "shellcue"

# macos specific
DMG_NAME = APP_NAME

# Linux specific
LINUX_LEGACY_GNOME_EXTENSION_UUID = "shellcue-legacy-gnome-integration@bandisch-apps.com"
LINUX_MODERN_GNOME_EXTENSION_UUID = "shellcue-gnome-integration@bandisch-apps.com"

# cargo packages
CLI_PACKAGE_NAME = "q_cli"
PTY_PACKAGE_NAME = "figterm"
DESKTOP_PACKAGE_NAME = "fig_desktop"
DESKTOP_FUZZ_PACKAGE_NAME = "fig_desktop-fuzz"

DESKTOP_PACKAGE_PATH = pathlib.Path("crates", "fig_desktop")
