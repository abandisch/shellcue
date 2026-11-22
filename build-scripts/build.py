from dataclasses import dataclass
import os
import json
import pathlib
import shutil
from typing import Dict, List, Mapping, Sequence
from util import (
    Variant,
    get_variants,
    isDarwin,
    isLinux,
    run_cmd,
    run_cmd_output,
    info,
    set_executable,
    version,
    tauri_product_name,
)
from rust import build_hash, build_datetime, cargo_cmd_name, rust_targets, rust_env, get_target_triple
from test import run_cargo_tests, run_clippy
from importlib import import_module
from const import (
    APP_NAME,
    CLI_BINARY_NAME,
    CLI_PACKAGE_NAME,
    DESKTOP_BINARY_NAME,
    DESKTOP_PACKAGE_NAME,
    DESKTOP_PACKAGE_PATH,
    DMG_NAME,
    LINUX_ARCHIVE_NAME,
    LINUX_LEGACY_GNOME_EXTENSION_UUID,
    LINUX_MODERN_GNOME_EXTENSION_UUID,
    LINUX_PACKAGE_NAME,
    PTY_BINARY_NAME,
    PTY_PACKAGE_NAME,
    TAURI_PRODUCT_NAME,
)

BUILD_DIR_RELATIVE = pathlib.Path(os.environ.get("BUILD_DIR") or "build")
BUILD_DIR = BUILD_DIR_RELATIVE.absolute()


@dataclass
class NpmBuildOutput:
    autocomplete_path: pathlib.Path


@dataclass
class MacOSBuildOutput:
    dmg_path: pathlib.Path
    app_gztar_path: pathlib.Path


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
    variant: Variant,
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
            **rust_env(release=release, variant=variant),
        },
    )

    if release:
        target_subdir = "release"
    else:
        target_subdir = "debug"

    # create "universal" binary for macos
    if isDarwin():
        out_path = BUILD_DIR / f"{output_name or package}-universal-apple-darwin"
        args = [
            "lipo",
            "-create",
            "-output",
            out_path,
        ]
        for target in targets:
            args.append(pathlib.Path("target") / target / target_subdir / package)
        run_cmd(args)
        return out_path
    else:
        # linux does not cross compile arch
        target = targets[0]
        target_path = pathlib.Path("target") / target / target_subdir / package
        out_path = BUILD_DIR / "bin" / f"{(output_name or package)}-{target}"
        out_path.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(target_path, out_path)
        return out_path


def build_macos_ime(
    release: bool,
    targets: Sequence[str] = [],
) -> pathlib.Path:
    fig_input_method_bin = build_cargo_bin(
        release=release, variant=Variant.FULL, package="fig_input_method", targets=targets
    )
    input_method_app = pathlib.Path("build/CodeWhispererInputMethod.app")

    (input_method_app / "Contents/MacOS").mkdir(parents=True, exist_ok=True)

    shutil.copy2(
        fig_input_method_bin,
        input_method_app / "Contents/MacOS/fig_input_method",
    )
    shutil.copy2(
        "crates/fig_input_method/Info.plist",
        input_method_app / "Contents/Info.plist",
    )
    shutil.copytree(
        "crates/fig_input_method/resources",
        input_method_app / "Contents/Resources",
        dirs_exist_ok=True,
    )

    return input_method_app


def macos_tauri_config(cli_path: pathlib.Path, pty_path: pathlib.Path, target: str) -> str:
    config = {
        "tauri": {
            "bundle": {
                "externalBin": [
                    # Note that tauri bundling expects the target triple to be appended to each binary,
                    # but in the config it must be strippled.
                    str(cli_path).removesuffix(f"-{target}"),
                    str(pty_path).removesuffix(f"-{target}"),
                ],
            }
        }
    }
    return json.dumps(config)


def build_macos_desktop_app(
    release: bool,
    pty_path: pathlib.Path,
    cli_path: pathlib.Path,
    npm_packages: NpmBuildOutput,
    features: Mapping[str, Sequence[str]] | None = None,
    targets: Sequence[str] = [],
) -> MacOSBuildOutput:
    target = get_target_triple()

    info("Building macos ime")
    ime_app = build_macos_ime(release=release, targets=targets)

    info("Building tauri config")
    tauri_config_path = pathlib.Path(DESKTOP_PACKAGE_PATH) / "build-config.json"
    tauri_config_path.write_text(macos_tauri_config(cli_path=cli_path, pty_path=pty_path, target=target))

    info("Building", DESKTOP_PACKAGE_NAME)

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

    run_cmd(
        cargo_tauri_args,
        cwd=DESKTOP_PACKAGE_PATH,
        env={**os.environ, **rust_env(release=release, variant=Variant.FULL), "BUILD_DIR": BUILD_DIR},
    )

    # clean up
    tauri_config_path.unlink(missing_ok=True)

    target_bundle = pathlib.Path(f"target/{target}/release/bundle/macos/{TAURI_PRODUCT_NAME}.app")
    app_path = BUILD_DIR / f"{APP_NAME}.app"
    shutil.rmtree(app_path, ignore_errors=True)
    shutil.copytree(target_bundle, app_path)

    info_plist_path = app_path / "Contents/Info.plist"

    # Change the display name of the app
    run_cmd(["defaults", "write", info_plist_path, "CFBundleDisplayName", APP_NAME])
    run_cmd(["defaults", "write", info_plist_path, "CFBundleName", APP_NAME])

    # Specifies the app is an "agent app"
    run_cmd(["defaults", "write", info_plist_path, "LSUIElement", "-bool", "TRUE"])

    info("Copying CodeWhispererInputMethod.app into bundle")
    helpers_dir = app_path / "Contents/Helpers"
    helpers_dir.mkdir(parents=True, exist_ok=True)
    shutil.copytree(ime_app, helpers_dir.joinpath("CodeWhispererInputMethod.app"))

    info("Copying autocomplete into bundle")
    shutil.copytree(npm_packages.autocomplete_path, app_path / "Contents/Resources/autocomplete")

    info("Copying specs into bundle")
    shutil.copytree(compiled_specs_path(), app_path / "Contents/Resources/specs")

    # Add symlinks
    # os.symlink(f"./{CLI_BINARY_NAME}", app_path / "Contents/MacOS/cli")
    # os.symlink(f"./{PTY_BINARY_NAME}", app_path / "Contents/MacOS/pty")

    dmg_path = BUILD_DIR / f"{DMG_NAME}.dmg"
    dmg_path.unlink(missing_ok=True)

    dmg_resources_dir = pathlib.Path("bundle/dmg")
    background_path = dmg_resources_dir / "background.png"
    icon_path = dmg_resources_dir / "VolumeIcon.icns"

    # we use a dynamic import here so that we dont use this dep
    # on other platforms
    dmgbuild = import_module("dmgbuild")

    dmgbuild.build_dmg(
        volume_name=APP_NAME,
        filename=dmg_path,
        settings={
            "format": "ULFO",
            "background": str(background_path),
            "icon": str(icon_path),
            "text_size": 12,
            "icon_size": 160,
            "window_rect": ((100, 100), (660, 400)),
            "files": [str(app_path)],
            "symlinks": {"Applications": "/Applications"},
            "icon_locations": {
                app_path.name: (180, 170),
                "Applications": (480, 170),
            },
        },
    )

    info(f"Created dmg at {dmg_path}")

    app_gztar_path = shutil.make_archive(str(BUILD_DIR / APP_NAME), "gztar", app_path.parent, app_path.name)
    info(f"Created app tar.gz at {app_gztar_path}")

    return MacOSBuildOutput(dmg_path=dmg_path, app_gztar_path=pathlib.Path(app_gztar_path))


def build_linux_minimal(cli_path: pathlib.Path, pty_path: pathlib.Path):
    """
    Creates tar.gz, tar.xz, tar.zst, and zip archives under `BUILD_DIR`.

    Each archive has the following structure:
    - archive/bin/{cli_binary}
    - archive/bin/{pty_binary}
    - archive/install.sh
    - archive/README
    - archive/BUILD-INFO
    """
    archive_name = LINUX_ARCHIVE_NAME

    archive_path = pathlib.Path(archive_name)
    archive_path.mkdir(parents=True, exist_ok=True)

    shutil.copy2("bundle/linux/install.sh", archive_path)
    shutil.copy2("bundle/linux/README", archive_path)

    # write the BUILD-INFO
    build_info_path = archive_path / "BUILD-INFO"
    build_info_path.write_text(
        "\n".join(
            [
                f"BUILD_DATE={build_datetime()}",
                f"BUILD_HASH={build_hash()}",
                f"BUILD_TARGET_TRIPLE={get_target_triple()}",
                f"BUILD_VERSION={version()}",
            ]
        )
    )

    archive_bin_path = archive_path / "bin"
    archive_bin_path.mkdir(parents=True, exist_ok=True)

    shutil.copy2(cli_path, archive_bin_path / CLI_BINARY_NAME)
    shutil.copy2(pty_path, archive_bin_path / PTY_BINARY_NAME)

    info(f"Building {archive_name}.tar.gz")
    tar_gz_path = BUILD_DIR / f"{archive_name}.tar.gz"
    run_cmd(["tar", "-czf", tar_gz_path, archive_path])
    generate_sha(tar_gz_path)

    info(f"Building {archive_name}.tar.xz")
    tar_xz_path = BUILD_DIR / f"{archive_name}.tar.xz"
    run_cmd(["tar", "-cJf", tar_xz_path, archive_path])
    generate_sha(tar_xz_path)

    info(f"Building {archive_name}.tar.zst")
    tar_zst_path = BUILD_DIR / f"{archive_name}.tar.zst"
    run_cmd(["tar", "-I", "zstd", "-cf", tar_zst_path, archive_path], {"ZSTD_CLEVEL": "19"})
    generate_sha(tar_zst_path)

    info(f"Building {archive_name}.zip")
    zip_path = BUILD_DIR / f"{archive_name}.zip"
    run_cmd(["zip", "-r", zip_path, archive_path])
    generate_sha(zip_path)

    # clean up
    shutil.rmtree(archive_path)


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
    shutil.copy("bundle/deb/control_minimal", bundle_dir / "DEBIAN/control_minimal")
    shutil.copy("bundle/deb/postrm", bundle_dir / "DEBIAN/postrm")
    shutil.copy("bundle/deb/prerm", bundle_dir / "DEBIAN/prerm")
    replace_text(bundle_dir / "DEBIAN/control", "$VERSION", version())
    replace_text(bundle_dir / "DEBIAN/control", "$APT_ARCH", "amd64")
    replace_text(bundle_dir / "DEBIAN/control_minimal", "$VERSION", version())
    replace_text(bundle_dir / "DEBIAN/control_minimal", "$APT_ARCH", "amd64")
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
        env={**os.environ, **rust_env(release=release, variant=Variant.FULL), "BUILD_DIR": BUILD_DIR},
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
    if isDarwin():
        shasum_output = run_cmd_output(["shasum", "-a", "256", path])
    elif isLinux():
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


BuildOutput = Dict[Variant, BinaryPaths]


def build(
    release: bool,
    variants: List[Variant] | None = None,
    run_lints: bool = True,
    run_test: bool = True,
) -> BuildOutput:
    variants = variants or get_variants()

    cargo_features: Mapping[str, Sequence[str]] = {}

    info(f"Release: {release}")
    info(f"Cargo features: {cargo_features}")
    info(f"Variants: {[variant.name for variant in variants]}")

    BUILD_DIR.mkdir(parents=True, exist_ok=True)

    npm_packages = None
    if Variant.FULL in variants:
        info("Building npm packages")
        npm_packages = build_npm_packages(run_test=run_test)

    targets = rust_targets()

    # Mac has multiple targets, so just use the default for the platform
    # for testing and linting.
    cargo_test_target = None if isDarwin() else targets[0]

    if run_test:
        info("Running cargo tests")
        run_cargo_tests(variants=variants, features=cargo_features, target=cargo_test_target)

    if run_lints:
        run_clippy(variants=variants, features=cargo_features, target=cargo_test_target)

    build_output: BuildOutput = {}
    for variant in variants:
        info(f"Building variant: {variant.name}")

        info("Building", CLI_PACKAGE_NAME)
        cli_path = build_cargo_bin(
            variant=variant,
            release=release,
            package=CLI_PACKAGE_NAME,
            output_name=CLI_BINARY_NAME,
            features=cargo_features,
            targets=targets,
        )

        info("Building", PTY_PACKAGE_NAME)
        pty_path = build_cargo_bin(
            variant=variant,
            release=release,
            package=PTY_PACKAGE_NAME,
            output_name=PTY_BINARY_NAME,
            features=cargo_features,
            targets=targets,
        )

        if isDarwin():
            info(f"Building {DMG_NAME}.dmg")
            if not npm_packages:
                raise RuntimeError("npm packages must be built for Mac")

            build_paths = build_macos_desktop_app(
                release=release,
                cli_path=cli_path,
                pty_path=pty_path,
                npm_packages=npm_packages,
                features=cargo_features,
                targets=targets,
            )

            generate_sha(build_paths.dmg_path)
        elif isLinux():
            if variant == Variant.FULL:
                if not npm_packages:
                    raise RuntimeError(f"npm packages must be built for variant: {variant.name}")
                build_linux_full(
                    release=release,
                    cli_path=cli_path,
                    pty_path=pty_path,
                    npm_packages=npm_packages,
                    features=cargo_features,
                )
                build_output[variant] = BinaryPaths(cli_path=cli_path, pty_path=pty_path)
            else:
                build_linux_minimal(cli_path=cli_path, pty_path=pty_path)
                build_output[variant] = BinaryPaths(cli_path=cli_path, pty_path=pty_path)

    return build_output
