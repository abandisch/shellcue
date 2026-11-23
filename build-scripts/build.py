from dataclasses import dataclass
import os
import json
import pathlib
import shutil
from typing import Mapping, Sequence
from util import (
    isDarwin,
    isLinux,
    run_cmd,
    run_cmd_output,
    info,
    fail,
    set_executable,
    version,
    tauri_product_name,
)
from rust import cargo_cmd_name, rust_targets, rust_env, get_target_triple
from test import run_cargo_tests, run_clippy
from const import (
    APP_NAME,
    CLI_BINARY_NAME,
    CLI_PACKAGE_NAME,
    DESKTOP_BINARY_NAME,
    DESKTOP_PACKAGE_NAME,
    DESKTOP_PACKAGE_PATH,
    LINUX_LEGACY_GNOME_EXTENSION_UUID,
    LINUX_MODERN_GNOME_EXTENSION_UUID,
    LINUX_PACKAGE_NAME,
    PTY_BINARY_NAME,
    PTY_PACKAGE_NAME,
)

BUILD_DIR_RELATIVE = pathlib.Path(os.environ.get("BUILD_DIR") or "build")
BUILD_DIR = BUILD_DIR_RELATIVE.absolute()


@dataclass
class NpmBuildOutput:
    autocomplete_path: pathlib.Path


def build_npm_packages(run_test: bool = True) -> NpmBuildOutput:
    run_cmd(["pnpm", "install", "--frozen-lockfile"])

    run_cmd(["pnpm", "build"])
    if run_test:
        run_cmd(["pnpm", "test", "--", "--run"])

    # copy to output
    autocomplete_path = BUILD_DIR / "autocomplete"
    shutil.rmtree(autocomplete_path, ignore_errors=True)
    shutil.copytree("packages/autocomplete-app/dist", autocomplete_path)

    return NpmBuildOutput(autocomplete_path=autocomplete_path)


def compiled_specs_path() -> pathlib.Path:
    specs_path = BUILD_DIR / "specs"
    if not (specs_path / "index.json").is_file():
        raise RuntimeError(f"{specs_path / 'index.json'} not found; the specs must be compiled first (VM step: specs)")
    return specs_path


def build_cargo_bin(
    release: bool,
    package: str,
    output_name: str | None = None,
    features: Mapping[str, Sequence[str]] | None = None,
    targets: Sequence[str] = [],
) -> pathlib.Path:
    args = [cargo_cmd_name(), "build", "--locked", "--package", package]

    if release:
        args.append("--release")

    for target in targets:
        args.extend(["--target", target])

    if features and features.get(package):
        args.extend(["--features", ",".join(features[package])])

    run_cmd(
        args,
        env={
            **os.environ,
            **rust_env(release=release),
        },
    )

    if release:
        target_subdir = "release"
    else:
        target_subdir = "debug"

    # linux does not cross compile arch
    target = targets[0]
    target_path = pathlib.Path("target") / target / target_subdir / package
    out_path = BUILD_DIR / "bin" / f"{(output_name or package)}-{target}"
    out_path.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(target_path, out_path)
    return out_path


def linux_tauri_config(
    cli_path: pathlib.Path,
    pty_path: pathlib.Path,
    autocomplete_path: pathlib.Path,
    specs_path: pathlib.Path,
    legacy_extension_dir_path: pathlib.Path,
    modern_extension_dir_path: pathlib.Path,
    target: str,
) -> str:
    config = {
        "tauri": {
            "systemTray": {"iconPath": "icons/32x32.png"},
            "bundle": {
                "externalBin": [
                    str(cli_path).removesuffix(f"-{target}"),
                    str(pty_path).removesuffix(f"-{target}"),
                ],
                "targets": ["appimage"],
                "icon": ["icons/128x128.png"],
                "resources": {
                    autocomplete_path.absolute().as_posix(): "autocomplete",
                    specs_path.absolute().as_posix(): "specs",
                    legacy_extension_dir_path.absolute().as_posix(): LINUX_LEGACY_GNOME_EXTENSION_UUID,
                    modern_extension_dir_path.absolute().as_posix(): LINUX_MODERN_GNOME_EXTENSION_UUID,
                },
            },
        }
    }
    return json.dumps(config)


def linux_desktop_entry() -> str:
    return (
        "[Desktop Entry]\n"
        "Categories=Development;\n"
        f"Exec={DESKTOP_BINARY_NAME}\n"
        f"Icon={LINUX_PACKAGE_NAME}\n"
        f"Name={APP_NAME}\n"
        "Terminal=false\n"
        "Type=Application"
    )


@dataclass
class LinuxDebResources:
    cli_path: pathlib.Path
    pty_path: pathlib.Path
    desktop_path: pathlib.Path
    specs_path: pathlib.Path
    legacy_extension_dir_path: pathlib.Path
    modern_extension_dir_path: pathlib.Path
    npm_packages: NpmBuildOutput


@dataclass
class DebBuildOutput:
    deb_path: pathlib.Path
    sha_path: pathlib.Path


def build_linux_deb(
    resources: LinuxDebResources,
    control_path: pathlib.Path,
    deb_suffix: str,
    release: bool,
) -> DebBuildOutput:
    """
    Builds a deb using the control file given by `control_path`.
    The deb will be named using the format: `f"{LINUX_PACKAGE_NAME}{deb_suffix}.deb"`

    This is kept generic in the case that we require different control files per Debian distribution.
    """
    info("Packaging deb bundle for control file:", control_path)

    bundles_dir = BUILD_DIR / "linux-bundles"
    bundle_dir = bundles_dir / deb_suffix
    shutil.rmtree(bundle_dir, ignore_errors=True)
    bundle_dir.mkdir(parents=True)

    info("Copying binaries")
    bin_path = bundle_dir / "usr/bin"
    bin_path.mkdir(parents=True)
    shutil.copy(resources.cli_path, bin_path / CLI_BINARY_NAME)
    shutil.copy(resources.pty_path, bin_path / PTY_BINARY_NAME)
    shutil.copy(resources.desktop_path, bin_path / DESKTOP_BINARY_NAME)

    info("Copying /usr/share resources")
    desktop_entry_path = bundle_dir / f"usr/share/applications/{LINUX_PACKAGE_NAME}.desktop"
    desktop_entry_path.parent.mkdir(parents=True)
    desktop_entry_path.write_text(linux_desktop_entry())
    desktop_icon_path = bundle_dir / f"usr/share/icons/hicolor/128x128/packages/{LINUX_PACKAGE_NAME}.png"
    desktop_icon_path.parent.mkdir(parents=True)
    share_path = bundle_dir / f"usr/share/{LINUX_PACKAGE_NAME}"
    share_path.mkdir(parents=True)
    shutil.copy(DESKTOP_PACKAGE_PATH / "icons" / "128x128.png", desktop_icon_path)
    shutil.copytree(resources.legacy_extension_dir_path, share_path / LINUX_LEGACY_GNOME_EXTENSION_UUID)
    shutil.copytree(resources.modern_extension_dir_path, share_path / LINUX_MODERN_GNOME_EXTENSION_UUID)
    shutil.copytree(resources.npm_packages.autocomplete_path, share_path / "autocomplete")
    shutil.copytree(resources.specs_path, share_path / "specs")

    def replace_text(file: pathlib.Path, old: str, new: str):
        file.write_text(file.read_text().replace(old, new))

    info("Creating DEBIAN structure")
    debian_path = bundle_dir / "DEBIAN"
    debian_path.mkdir(parents=True)
    shutil.copy(control_path, bundle_dir / "DEBIAN/control")
    shutil.copy("bundle/deb/postrm", bundle_dir / "DEBIAN/postrm")
    shutil.copy("bundle/deb/prerm", bundle_dir / "DEBIAN/prerm")
    replace_text(bundle_dir / "DEBIAN/control", "$VERSION", version())
    replace_text(bundle_dir / "DEBIAN/control", "$APT_ARCH", "amd64")
    set_executable(bundle_dir / "DEBIAN/postrm")

    info("Running dpkg-deb build")
    dpkg_deb_args = ["dpkg-deb", "--build", "--root-owner-group"]
    if not release:
        # Remove compression to increase build time.
        dpkg_deb_args.append("-z0")
    run_cmd([*dpkg_deb_args, bundle_dir], cwd=bundles_dir)

    deb_path = BUILD_DIR / f"{LINUX_PACKAGE_NAME}{deb_suffix}.deb"
    info("Moving built deb to", deb_path)
    (bundles_dir / f"{bundle_dir}.deb").rename(deb_path)
    run_cmd(["dpkg-deb", "--info", deb_path])
    sha_path = generate_sha(deb_path)
    return DebBuildOutput(deb_path=deb_path, sha_path=sha_path)


def build_linux_full(
    release: bool,
    cli_path: pathlib.Path,
    pty_path: pathlib.Path,
    npm_packages: NpmBuildOutput,
    features: Mapping[str, Sequence[str]] | None = None,
):
    target = get_target_triple()

    specs_path = compiled_specs_path()

    info("Grabbing GNOME extensions")

    # Creating a directory for each GNOME extension with the structure:
    # - {extension_uuid}.zip         <-- extension zip installable with gnome-extensions cli
    # - {extension_uuid}.version.txt <-- simple text file containing the extension version within the zip
    def copy_extension(extension_uuid, extension_dir_name):
        extension_dir_path = BUILD_DIR / extension_uuid
        shutil.rmtree(extension_dir_path, ignore_errors=True)
        extension_dir_path.mkdir(parents=True)
        extension_zip_path = extension_dir_path / f"{extension_uuid}.zip"
        shutil.copy(
            pathlib.Path(f"extensions/{extension_dir_name}/{extension_uuid}.zip"),
            extension_zip_path,
        )
        metadata = run_cmd_output(["unzip", "-p", extension_zip_path, "metadata.json"])
        extension_version = json.loads(metadata)["version"]
        pathlib.Path(extension_dir_path / f"{extension_uuid}.version.txt").write_text(str(extension_version))
        return extension_dir_path

    legacy_extension_dir_path = copy_extension(LINUX_LEGACY_GNOME_EXTENSION_UUID, "gnome-legacy-extension")
    modern_extension_dir_path = copy_extension(LINUX_MODERN_GNOME_EXTENSION_UUID, "gnome-extension")

    info("Building tauri config")
    tauri_config_path = DESKTOP_PACKAGE_PATH / "build-config.json"
    tauri_config_path.write_text(
        linux_tauri_config(
            cli_path=cli_path,
            pty_path=pty_path,
            autocomplete_path=npm_packages.autocomplete_path,
            specs_path=specs_path,
            legacy_extension_dir_path=legacy_extension_dir_path,
            modern_extension_dir_path=modern_extension_dir_path,
            target=target,
        )
    )

    cargo_tauri_args = [
        "cargo-tauri",
        "build",
        "--config",
        "build-config.json",
        "--target",
        target,
    ]
    if features and features.get(DESKTOP_PACKAGE_NAME):
        cargo_tauri_args.extend(["--features", ",".join(features[DESKTOP_PACKAGE_NAME])])
    if not release:
        cargo_tauri_args.extend(["--debug"])

    info("Building", DESKTOP_PACKAGE_NAME)
    run_cmd(
        cargo_tauri_args,
        cwd=DESKTOP_PACKAGE_PATH,
        env={**os.environ, **rust_env(release=release), "BUILD_DIR": BUILD_DIR},
    )
    desktop_path = pathlib.Path(f"target/{target}/{'release' if release else 'debug'}/{DESKTOP_BINARY_NAME}")

    deb_resources = LinuxDebResources(
        cli_path=cli_path,
        pty_path=pty_path,
        desktop_path=desktop_path,
        specs_path=specs_path,
        legacy_extension_dir_path=legacy_extension_dir_path,
        modern_extension_dir_path=modern_extension_dir_path,
        npm_packages=npm_packages,
    )
    build_linux_deb(
        resources=deb_resources,
        control_path=pathlib.Path("bundle/deb/control"),
        deb_suffix="",
        release=release,
    )

    info("Copying AppImage to build directory")
    # Determine architecture suffix based on the target triple
    arch_suffix = "aarch64" if "aarch64" in target else "amd64"
    info(f"Using architecture suffix: {arch_suffix} for target: {target}")

    bundle_name = f"{tauri_product_name()}_{version()}_{arch_suffix}"
    target_subdir = "release" if release else "debug"
    bundle_grandparent_path = f"target/{target}/{target_subdir}/bundle"
    appimage_path = BUILD_DIR / f"{LINUX_PACKAGE_NAME}.appimage"
    shutil.copy(
        pathlib.Path(f"{bundle_grandparent_path}/appimage/{bundle_name}.AppImage"),
        appimage_path,
    )
    generate_sha(appimage_path)


def generate_sha(path: pathlib.Path) -> pathlib.Path:
    if isLinux():
        shasum_output = run_cmd_output(["sha256sum", path])
    else:
        raise Exception("Unsupported platform")

    sha = shasum_output.split(" ")[0]
    path = path.with_name(f"{path.name}.sha256")
    path.write_text(sha)
    info(f"Wrote sha256sum to {path}:", sha)
    return path


@dataclass
class BinaryPaths:
    cli_path: pathlib.Path
    pty_path: pathlib.Path


def build(
    release: bool,
    run_lints: bool = True,
    run_test: bool = True,
) -> BinaryPaths:
    if isDarwin():
        fail(
            "macOS packaging is not done by this script; it runs in the macOS build VM, "
            "driven from this Mac by scripts/vm/mac-host.sh (see: scripts/vm/mac-host.sh --help)"
        )
        raise SystemExit(1)

    cargo_features: Mapping[str, Sequence[str]] = {}

    info(f"Release: {release}")
    info(f"Cargo features: {cargo_features}")

    BUILD_DIR.mkdir(parents=True, exist_ok=True)

    info("Building npm packages")
    npm_packages = build_npm_packages(run_test=run_test)

    targets = rust_targets()

    # Mac has multiple targets, so just use the default for the platform
    # for testing and linting.
    cargo_test_target = None if isDarwin() else targets[0]

    if run_test:
        info("Running cargo tests")
        run_cargo_tests(features=cargo_features, target=cargo_test_target)

    if run_lints:
        run_clippy(features=cargo_features, target=cargo_test_target)

    info("Building", CLI_PACKAGE_NAME)
    cli_path = build_cargo_bin(
        release=release,
        package=CLI_PACKAGE_NAME,
        output_name=CLI_BINARY_NAME,
        features=cargo_features,
        targets=targets,
    )

    info("Building", PTY_PACKAGE_NAME)
    pty_path = build_cargo_bin(
        release=release,
        package=PTY_PACKAGE_NAME,
        output_name=PTY_BINARY_NAME,
        features=cargo_features,
        targets=targets,
    )

    if isLinux():
        build_linux_full(
            release=release,
            cli_path=cli_path,
            pty_path=pty_path,
            npm_packages=npm_packages,
            features=cargo_features,
        )

    return BinaryPaths(cli_path=cli_path, pty_path=pty_path)
