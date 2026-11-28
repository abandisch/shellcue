#!/usr/bin/env bash
# Runs INSIDE an Ubuntu 24.04 VM: the Multipass build VM (host.sh copies it in) or the UTM desktop VM (linux-host.sh).

set -euo pipefail

# Every pnpm install passes an explicit lockfile flag because CI=true changes pnpm's default.
export CI=true NO_COLOR=1 FORCE_COLOR=0 TERM=dumb TURBO_TELEMETRY_DISABLED=1 DO_NOT_TRACK=1

REPO_URL="https://github.com/abandisch/shellcue.git"
TOOLS="$HOME/tools"
DL="$TOOLS/dl"
ENV_FILE="$TOOLS/env.sh"
SRC="$HOME/shellcue"
SPECS_REPO_URL="https://github.com/abandisch/shellcue-specs.git"
SPECS_SRC="$HOME/shellcue-specs"
# Outside the checkout so generated icons never dirty it; host.sh pull-icons reads from here.
ICONS_OUT="$HOME/shellcue-icons"
LOCK_DIR="$HOME/.shellcue-guest.lock"
STAMPS="$SRC/build/stamps"
OUT="$SRC/build/linux"
# Written by tools when sudo needs a password; the owner runs it in the VM's own Terminal.
APT_SCRIPT="$HOME/shellcue-apt.sh"
PROGRAMS="fig_desktop:shellcue-desktop q_cli:shellcue figterm:shellcue-term"
GNOME_EXT_DIR="extensions/gnome-extension"
# Must equal the uuid in $GNOME_EXT_DIR/src/metadata.json and EXTENSION_UUID in crates/dbus/src/dbus/gnome_shell.rs.
GNOME_EXT_UUID="shellcue-gnome-integration@bandisch-apps.com"

# Change together with scripts/vm/mac-guest.sh, .mise.toml (node, pnpm, protoc), package.json (packageManager) and rust-toolchain.toml (Rust); enter_src enforces it.
NODE_VERSION="26.13.0"
PNPM_VERSION="12.17.0"
PROTOC_VERSION="36.2"
RUSTUP_VERSION="1.29.3"
RUST_TOOLCHAIN="1.87.0"

NODE_SHASUMS_URL="https://nodejs.org/dist/v$NODE_VERSION/SHASUMS256.txt.asc"
NODE_KEYRING_URL="https://github.com/nodejs/release-keys/raw/HEAD/gpg/pubring.kbx"

APT_PACKAGES=(
    build-essential pkg-config jq curl wget zstd cmake clang libssl-dev libgtk-3-dev
    libayatana-appindicator3-dev librsvg2-dev libdbus-1-dev libwebkit2gtk-4.1-dev
    libjavascriptcoregtk-4.1-dev valac libibus-1.0-dev libglib2.0-dev sqlite3 libxdo-dev
    git ca-certificates xz-utils zip unzip gpgv zsh fish shellcheck
    librsvg2-bin icnsutils
)

STEPS="tools clone lock fetch web specs icons rust-check rust-test build bundle info"

usage() {
    cat <<'EOF'
Usage: guest.sh <step> [args]

Runs inside the ShellCue build VM. Steps:
  tools                    apt packages + pinned, sha256-verified node, pnpm, protoc, rustup/rust; writes ~/tools/env.sh.
                           Where sudo needs a password it installs nothing itself: if packages are missing it writes
                           ~/shellcue-apt.sh and stops, for you to run   sudo bash ~/shellcue-apt.sh   in the VM's own Terminal
  clone <branch> <sha>     clone/fetch the public repo into ~/shellcue and check out <sha> detached
  lock                     refresh Cargo.lock (cargo update --workspace) and pnpm-lock.yaml (lockfile-only); print what changed
  fetch                    cargo fetch --locked and pnpm install --frozen-lockfile --ignore-scripts
  web                      pnpm run build, then a release build of the GNOME Shell extension, then pnpm run test:ci
  specs                    fetch the specs repo at the commit in packages/spec-compiler/SPECS_COMMIT and compile it
                           into ~/shellcue/build/specs; needs 'fetch' first (nothing from the specs repo is installed or run)
  icons [check]            generate the PNG and ICNS icons from artwork/*.svg into ~/shellcue-icons (outside the checkout)
                           and report whether each committed file matches; with 'check', fail if any does not
  rust-check               cargo check --locked --workspace --all-targets --exclude fig_desktop-fuzz
  rust-test                cargo test --locked --no-fail-fast --workspace --lib --bins --test '*' --exclude fig_desktop-fuzz,
                           with SSH_CLIENT, SSH_CONNECTION and SSH_TTY unset
  build                    cargo build --locked --release of the three programs (desktop app, CLI, terminal wrapper)
  bundle                   stage them with the popup page, the specs and the GNOME Shell extension and make the Debian
                           package with dpkg-deb, in
                           ~/shellcue/build/linux; web, specs and build must have succeeded at the checked-out commit
  info                     OS, arch, tool versions, disk free, checked-out commit, git status

Each step prints "==> step: <name>" first and, as its last line, "==> ok: <name> (<N>s)" on success
or "==> FAILED: <name> (exit <N>, <N>s)" on any failure.
EOF
}

die() {
    printf '==> ERROR: %s\n' "$*" >&2
    exit 1
}

CURRENT_STEP=""
STEP_START=0
LOCK_HELD=0

# Must stay the last thing printed, whatever ended the run.
print_verdict() {
    local rc=$? secs=$((SECONDS - STEP_START))
    if [[ $LOCK_HELD -eq 1 ]]; then rm -rf -- "$LOCK_DIR" || true; fi
    if [[ $rc -eq 0 ]]; then
        printf '==> ok: %s (%ss)\n' "$CURRENT_STEP" "$secs"
    else
        printf '==> FAILED: %s (exit %s, %ss)\n' "$CURRENT_STEP" "$rc" "$secs"
    fi
}

begin_step() {
    CURRENT_STEP="$1"
    STEP_START=$SECONDS
    trap print_verdict EXIT
    local self_sum
    self_sum="$(sha256sum -- "$0")"
    printf '==> step: %s\n' "$CURRENT_STEP"
    printf '    time (UTC): %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    printf '    guest.sh sha256: %s\n' "${self_sum%% *}"
}

# A step whose connection dropped keeps running in the VM; a second step on top of it would share its build folders.
take_lock() {
    local holder="" name pids left=""
    if ! mkdir -- "$LOCK_DIR" 2>/dev/null; then
        holder="$(cat -- "$LOCK_DIR/pid" 2>/dev/null || true)"
        case "$holder" in
            ''|*[!0-9]*) holder="" ;;
        esac
        # A lock outlives a reboot, and its number may by then belong to an unrelated process.
        if [[ -n "$holder" ]] && kill -0 "$holder" 2>/dev/null \
            && tr '\0' ' ' < "/proc/$holder/cmdline" 2>/dev/null | grep -qF -- "${0##*/}"; then
            die "an earlier ${0##*/} step is still running in the VM (process $holder), probably left by a dropped connection. Wait for it to end, or stop it in a terminal inside the VM with: kill $holder"
        fi
        printf '    removing the lock of an earlier step that is no longer running (process %s)\n' "${holder:-unknown}"
        rm -rf -- "$LOCK_DIR"
        mkdir -- "$LOCK_DIR" || die "cannot create $LOCK_DIR"
    fi
    LOCK_HELD=1
    printf '%s\n' "$$" > "$LOCK_DIR/pid"
    for name in cargo rustc node pnpm; do
        pids="$(pgrep -u "$(id -u)" -x "$name" | tr '\n' ' ' || true)"
        [[ -z "$pids" ]] || left="$left $name: $pids"
    done
    [[ -z "$left" ]] \
        || die "build processes from an earlier step are still running in the VM ($left). Wait for them to end, or stop them in the VM's own terminal with: kill <the numbers>"
}

FAILED_PARTS=()

# set -e is ignored inside functions called from a condition, so each part runs as a plain subshell.
run_part() {
    local label="$1" rc
    shift
    printf -- '--> part: %s\n' "$label"
    set +e
    ( set -e; "$@" )
    rc=$?
    set -e
    if [[ $rc -ne 0 ]]; then
        printf -- '--> FAILED: %s (exit status %s)\n' "$label" "$rc"
        FAILED_PARTS+=("$label")
    else
        printf -- '--> ok: %s\n' "$label"
    fi
}

end_parts() {
    if [[ ${#FAILED_PARTS[@]} -gt 0 ]]; then
        printf -- '--> failed parts: %s\n' "${FAILED_PARTS[*]}"
        exit 1
    fi
}

require_no_args() {
    [[ $# -eq 0 ]] || die "step '$CURRENT_STEP' takes no arguments (got: $*)"
}

detect_arch() {
    local node_arch pnpm_arch protoc_arch rust_triple
    ARCH="$(uname -m)"
    case "$ARCH" in
        aarch64)
            node_arch="arm64"
            pnpm_arch="arm64"
            protoc_arch="aarch_64"
            rust_triple="aarch64-unknown-linux-gnu"
            NODE_SHA256="406fe3fbde1c054adff8b297c16fdb5af6659db327c1ac8acd46b56ef1af9a92"
            PNPM_SHA256="ce5a5b95a9974a0bbd713a5ef74416dbcd0e3fdf11f69ea92c5995b9c18ccb47"
            PROTOC_SHA256="8b8f18bd2b30346efbc698dd5a73dd7c805f3ef8380f6dfc95c768f3f1852f6a"
            RUSTUP_SHA256="9f608c142e0e38a78957d0661ba1dfaf0b6fb37184cfaa4b9a0aa57af6184282"
            ;;
        x86_64)
            node_arch="x64"
            pnpm_arch="x64"
            protoc_arch="x86_64"
            rust_triple="x86_64-unknown-linux-gnu"
            NODE_SHA256="d28a96af6c38596f6d9d97c0ee531c13d238d757742387d1928de822d959e997"
            PNPM_SHA256="2c58959fd5df5cb9f4e72784c08a60ffc7ee558fed9b04b22254e29112203a51"
            PROTOC_SHA256="121f6c7afe1d4d0e3ea6aab9432038599250134cbf4474cb1167d2c7decd4278"
            RUSTUP_SHA256="b1c8cb14e30889e7e2999e6b69e62d9e9b9ffc0f4df8180564d306d9680e6c55"
            ;;
        *)
            die "unsupported architecture from 'uname -m': '$ARCH' (expected aarch64 or x86_64)"
            ;;
    esac
    NODE_URL="https://nodejs.org/dist/v$NODE_VERSION/node-v$NODE_VERSION-linux-$node_arch.tar.xz"
    PNPM_URL="https://github.com/pnpm/pnpm/releases/download/v$PNPM_VERSION/pnpm-linux-$pnpm_arch.tar.gz"
    PROTOC_URL="https://github.com/protocolbuffers/protobuf/releases/download/v$PROTOC_VERSION/protoc-$PROTOC_VERSION-linux-$protoc_arch.zip"
    RUSTUP_URL="https://static.rust-lang.org/rustup/archive/$RUSTUP_VERSION/$rust_triple/rustup-init"
    NODE_TARBALL="${NODE_URL##*/}"
}

sha256_of() {
    local out
    out="$(sha256sum -- "$1")"
    printf '%s' "${out%% *}"
}

curl_https() {
    curl -q --proto '=https' --proto-redir '=https' --tlsv1.2 --connect-timeout 30 --retry 3 \
        --speed-limit 1024 --speed-time 60 -fsSL "$@"
}

# Nothing downloaded is extracted or executed until its sha256 matches the pin.
fetch_verified() {
    local url="$1" dest="$2" want="$3" got
    if [[ -f "$dest" ]]; then
        got="$(sha256_of "$dest")"
        if [[ "$got" == "$want" ]]; then
            printf '    cached, sha256 ok: %s\n' "$dest"
            return 0
        fi
        printf '    cached file has wrong sha256, re-downloading: %s (%s)\n' "$dest" "$got"
        rm -f -- "$dest"
    fi
    printf '    downloading %s\n' "$url"
    rm -f -- "$dest.part"
    curl_https -o "$dest.part" "$url" || die "download failed: $url"
    got="$(sha256_of "$dest.part")"
    if [[ "$got" != "$want" ]]; then
        mv -f -- "$dest.part" "$dest.mismatch"
        printf '==> SHA256 MISMATCH for %s\n' "$url" >&2
        printf '    expected: %s\n' "$want" >&2
        printf '    actual:   %s\n' "$got" >&2
        printf '    kept for inspection (not extracted, not executed): %s\n' "$dest.mismatch" >&2
        printf '    The download was not extracted or run. Do not edit the pinned hash and do not retry blindly;\n' >&2
        printf '    keep this log and have it reviewed, together with %s.\n' "$dest.mismatch" >&2
        exit 1
    fi
    mv -f -- "$dest.part" "$dest"
    printf '    sha256 ok: %s\n' "$dest"
}

check_os() {
    [[ -r /etc/os-release ]] || die "/etc/os-release not readable; cannot confirm Ubuntu 24.04"
    local id version pretty
    id="$(. /etc/os-release && printf '%s' "${ID:-}")"
    version="$(. /etc/os-release && printf '%s' "${VERSION_ID:-}")"
    pretty="$(. /etc/os-release && printf '%s' "${PRETTY_NAME:-}")"
    printf '    os-release: ID=%s VERSION_ID=%s (%s)\n' "$id" "$version" "$pretty"
    [[ "$id" == "ubuntu" && "$version" == "24.04" ]] \
        || die "this script only runs on Ubuntu 24.04; found ID='$id' VERSION_ID='$version'"
}

verify_node_signature() {
    local asc="$DL/SHASUMS256.txt.asc" keyring="$DL/nodejs-keyring.kbx" verified="$DL/SHASUMS256.txt"
    printf '    gpgv check of Node SHASUMS256.txt.asc (independent second check)\n'
    rm -f -- "$asc" "$keyring" "$verified"
    curl_https -o "$asc" "$NODE_SHASUMS_URL" || die "download failed: $NODE_SHASUMS_URL"
    curl_https -o "$keyring" "$NODE_KEYRING_URL" || die "download failed: $NODE_KEYRING_URL"
    gpgv --keyring="$keyring" --output "$verified" "$asc" \
        || die "gpgv could not verify $NODE_SHASUMS_URL against the Node release keyring. The download was not extracted or run. Do not edit the pinned hash and do not retry blindly; keep this log and have it reviewed."
    grep -qxF "$NODE_SHA256  $NODE_TARBALL" "$verified" \
        || die "gpgv-verified SHASUMS256.txt has no line '$NODE_SHA256  $NODE_TARBALL'"
    printf '    gpgv ok; signed SHASUMS256.txt lists the pinned hash for %s\n' "$NODE_TARBALL"
}

version_of() {
    "$@" 2>/dev/null || true
}

install_node() {
    local dir="$TOOLS/node"
    rm -rf -- "$dir" "$dir.staging"
    mkdir -p -- "$dir.staging"
    tar -xJf "$DL/$NODE_TARBALL" -C "$dir.staging" --strip-components=1 --no-same-owner
    [[ -x "$dir.staging/bin/node" ]] || die "node tarball did not contain bin/node"
    mv -- "$dir.staging" "$dir"
}

# The pnpm 12 tarball layout is unconfirmed; the whole archive stays intact and its pnpm directory goes on PATH.
find_pnpm_dir() {
    local root="$1" found=()
    while IFS= read -r -d '' f; do
        found+=("$f")
    done < <(find "$root" -name pnpm -type f -perm -u+x -print0)
    if [[ ${#found[@]} -ne 1 ]]; then
        printf '    executable files named pnpm under %s: %s\n' "$root" "${#found[@]}" >&2
        if [[ ${#found[@]} -gt 0 ]]; then printf '      %s\n' "${found[@]}" >&2; fi
        die "expected exactly one executable named 'pnpm' in the pnpm archive"
    fi
    dirname -- "${found[0]}"
}

install_pnpm() {
    local dir="$TOOLS/pnpm" tgz="$DL/pnpm-linux.tar.gz"
    printf '    pnpm archive listing:\n'
    tar -tvzf "$tgz" | sed 's/^/      /'
    rm -rf -- "$dir" "$dir.staging"
    mkdir -p -- "$dir.staging"
    tar -xzf "$tgz" -C "$dir.staging" --no-same-owner
    mv -- "$dir.staging" "$dir"
    PNPM_DIR="$(find_pnpm_dir "$dir")"
    printf '    pnpm executable: %s/pnpm\n' "$PNPM_DIR"
}

install_protoc() {
    local dir="$TOOLS/protoc" zip="$DL/protoc-linux.zip"
    printf '    protoc zip listing:\n'
    unzip -l "$zip" | sed 's/^/      /'
    rm -rf -- "$dir" "$dir.staging"
    mkdir -p -- "$dir.staging"
    unzip -q "$zip" -d "$dir.staging"
    [[ -f "$dir.staging/bin/protoc" ]] || die "protoc zip did not contain bin/protoc"
    [[ -d "$dir.staging/include" ]] || die "protoc zip did not contain include/"
    chmod u+x -- "$dir.staging/bin/protoc"
    mv -- "$dir.staging" "$dir"
}

rustup_version_ok() {
    local out first
    [[ -x "$CARGO_HOME/bin/rustup" ]] || return 1
    out="$(version_of "$CARGO_HOME/bin/rustup" --version)"
    first="${out%%$'\n'*}"
    [[ "$first" == "rustup $RUSTUP_VERSION" || "$first" == "rustup $RUSTUP_VERSION "* ]]
}

install_rustup() {
    chmod u+x -- "$DL/rustup-init"
    "$DL/rustup-init" -y --default-toolchain none --profile minimal --no-modify-path
}

write_env() {
    local path_prefix="$TOOLS/node/bin:$PNPM_DIR:$TOOLS/protoc/bin:$CARGO_HOME/bin"
    {
        printf '# Generated by guest.sh tools; sourced by every later step.\n'
        printf 'export RUSTUP_HOME=%q\n' "$RUSTUP_HOME"
        printf 'export CARGO_HOME=%q\n' "$CARGO_HOME"
        printf 'export RUSTUP_TOOLCHAIN=%q\n' "$RUST_TOOLCHAIN"
        printf 'export CARGO_TERM_COLOR=never\n'
        printf 'export SHELLCUE_PROTOC=%q\n' "$TOOLS/protoc/bin/protoc"
        printf 'export PATH=%q:"$PATH"\n' "$path_prefix"
    } > "$ENV_FILE.tmp"
    mv -f -- "$ENV_FILE.tmp" "$ENV_FILE"
    printf '    wrote %s:\n' "$ENV_FILE"
    sed 's/^/      /' "$ENV_FILE"
}

load_env() {
    [[ -f "$ENV_FILE" ]] || die "$ENV_FILE not found; run the 'tools' step first"
    # shellcheck source=/dev/null
    . "$ENV_FILE"
    local t p
    for t in node pnpm protoc cargo rustc rustup; do
        p="$(type -P "$t" || true)"
        [[ -n "$p" ]] || die "'$t' not on PATH after sourcing $ENV_FILE; re-run the 'tools' step"
    done
    # fig_proto's build.rs downloads its own protoc unless a new-enough one is first on PATH.
    p="$(type -P protoc)"
    [[ "$p" == "$SHELLCUE_PROTOC" ]] || die "protoc on PATH is '$p', expected '$SHELLCUE_PROTOC'"
}

require_pin() {
    local file="$1" key="$2" want="$3" found
    grep -qF -- "$key$want" "$file" && return 0
    found="$(grep -F -- "$key" "$file" || true)"
    die "$file does not agree with guest.sh: guest.sh pins '$key$want', $file has '${found:-no ${key% } line}'"
}

check_pins() {
    require_pin package.json '"packageManager": ' "\"pnpm@$PNPM_VERSION\""
    require_pin rust-toolchain.toml 'channel = ' "\"$RUST_TOOLCHAIN\""
    require_pin .mise.toml 'node = ' "\"$NODE_VERSION\""
    require_pin .mise.toml 'pnpm = ' "\"$PNPM_VERSION\""
    require_pin .mise.toml 'protoc = ' "\"$PROTOC_VERSION\""
}

enter_src() {
    [[ -d "$SRC/.git" ]] || die "no checkout at $SRC; run the 'clone' step first"
    cd "$SRC"
    printf '    checkout: %s @ %s\n' "$SRC" "$(git rev-parse HEAD)"
    check_pins
    local f changed=()
    for f in Cargo.lock pnpm-lock.yaml; do
        git diff --quiet HEAD -- "$f" || changed+=("$f")
    done
    if [[ ${#changed[@]} -gt 0 ]]; then
        printf '    NOTICE: %s differ(s) from the committed version; this run uses lockfiles that are not committed\n' "${changed[*]}"
    fi
}

tree_is_clean() {
    [[ -z "$(git status --porcelain=v1 --untracked-files=no)" ]]
}

require_clean_tree() {
    local dirty
    dirty="$(git status --porcelain=v1 --untracked-files=no)"
    if [[ -n "$dirty" ]]; then
        printf '    tracked files modified in %s:\n' "$SRC" >&2
        printf '%s\n' "$dirty" | sed 's/^/      /' >&2
        die "refusing to work from modified tracked files. In the VM's own terminal run: git -C ~/shellcue checkout HEAD -- .   and then run this step again"
    fi
}

# A stamp records the commit a step last succeeded at, so bundle never mixes outputs of different commits.
write_stamp() {
    if ! tree_is_clean; then
        printf '    no stamp written for %s: tracked files are modified, so bundle will not accept this run\n' "$1"
        return 0
    fi
    mkdir -p -- "$STAMPS"
    git rev-parse HEAD > "$STAMPS/$1"
}

require_stamps() {
    local head name got missing=""
    head="$(git rev-parse HEAD)"
    for name in "$@"; do
        got="$(cat -- "$STAMPS/$name" 2>/dev/null || true)"
        printf '    stamp %s: %s\n' "$name" "${got:-never}"
        [[ "$got" == "$head" ]] || missing="$missing $name"
    done
    [[ -z "$missing" ]] \
        || die "these steps have not succeeded at the checked-out commit $head:$missing. Run the fetch step if it has not run at this commit, then each step named here in the order web specs build, then this step again"
}

# Build inputs and outputs must not be reachable from the host: a folder shared into the VM would break that.
check_no_host_mounts() {
    local shared gvfs
    shared="$(awk '$3 ~ /^(9p|virtiofs|fuse\.sshfs|vboxsf|nfs|nfs4|cifs|smb3|davfs)$/ { print $2 " (" $3 ")" }' /proc/mounts)"
    [[ -z "$shared" ]] || die "a shared or network folder is mounted in this VM: $shared. Remove the shared directory from the VM's settings and start it again"
    # Folders opened in Files, and UTM's WebDAV share, appear only here; their mount type is always present on a desktop.
    gvfs="/run/user/$(id -u)/gvfs"
    if [[ -d "$gvfs" ]]; then
        shared="$(find "$gvfs" -mindepth 1 -maxdepth 1 -print 2>/dev/null | sed -n 1,5p)"
        [[ -z "$shared" ]] || die "a shared or network folder is open in the VM's Files app: $shared. Unmount it there (the eject button beside it), remove any shared directory from the VM's settings, and run this step again"
    fi
    printf '    no shared or network folders mounted\n'
}

host_triple() {
    local out triple
    out="$(rustc -vV)"
    triple="$(printf '%s\n' "$out" | sed -n 's/^host: //p')"
    [[ -n "$triple" ]] || die "could not read host triple from 'rustc -vV'"
    printf '%s' "$triple"
}

# Nothing here ever feeds sudo a password: where sudo would ask, the owner runs the install in the VM's own Terminal.
install_apt_packages() {
    local p missing=()
    if sudo -n true 2>/dev/null; then
        sudo apt-get update
        sudo DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends "${APT_PACKAGES[@]}"
        return 0
    fi
    for p in "${APT_PACKAGES[@]}"; do
        [[ "$(dpkg-query -W -f='${db:Status-Status}' "$p" 2>/dev/null || true)" == "installed" ]] || missing+=("$p")
    done
    if [[ ${#missing[@]} -eq 0 ]]; then
        printf '    all %s apt packages are installed (sudo needs a password here, so nothing was updated)\n' "${#APT_PACKAGES[@]}"
        return 0
    fi
    printf '    apt packages not installed: %s\n' "${missing[*]}"
    rm -f -- "$APT_SCRIPT"
    {
        printf '#!/bin/sh\n# Written by guest.sh tools. Run in the Terminal of the VM: sudo bash ~/shellcue-apt.sh\n'
        printf 'set -e\napt-get -o DPkg::Lock::Timeout=300 update\nDEBIAN_FRONTEND=noninteractive apt-get -o DPkg::Lock::Timeout=300 install -y --no-install-recommends'
        printf ' %s' "${APT_PACKAGES[@]}"
        printf '\n'
    } > "$APT_SCRIPT"
    die "system packages are missing and sudo needs a password here. In the VM's own Terminal run:   sudo bash ~/shellcue-apt.sh   and then run the tools step again"
}

step_tools() {
    require_no_args "$@"
    check_os
    detect_arch
    printf '    arch: %s\n' "$ARCH"

    install_apt_packages

    mkdir -p -- "$DL"
    export RUSTUP_HOME="$TOOLS/rustup"
    export CARGO_HOME="$TOOLS/cargo"

    printf -- '--> node %s\n' "$NODE_VERSION"
    fetch_verified "$NODE_URL" "$DL/$NODE_TARBALL" "$NODE_SHA256"
    verify_node_signature
    printf -- '--> pnpm %s\n' "$PNPM_VERSION"
    fetch_verified "$PNPM_URL" "$DL/pnpm-linux.tar.gz" "$PNPM_SHA256"
    printf -- '--> protoc %s\n' "$PROTOC_VERSION"
    fetch_verified "$PROTOC_URL" "$DL/protoc-linux.zip" "$PROTOC_SHA256"
    printf -- '--> rustup-init %s\n' "$RUSTUP_VERSION"
    fetch_verified "$RUSTUP_URL" "$DL/rustup-init" "$RUSTUP_SHA256"

    install_node
    install_pnpm
    install_protoc
    install_rustup
    write_env
    load_env

    cd "$TOOLS"
    rustup set auto-self-update disable
    rustup toolchain install "$RUST_TOOLCHAIN" --profile minimal --component rustfmt --component clippy

    local v_node v_pnpm v_protoc v_rustup v_rustc
    v_node="$(node --version)"
    [[ "$v_node" == "v$NODE_VERSION" ]] || die "node --version printed '$v_node', expected 'v$NODE_VERSION'"
    v_pnpm="$(pnpm --version)"
    [[ "$v_pnpm" == "$PNPM_VERSION" ]] || die "pnpm --version printed '$v_pnpm', expected '$PNPM_VERSION'"
    v_protoc="$(protoc --version)"
    [[ "$v_protoc" == "libprotoc $PROTOC_VERSION" ]] || die "protoc --version printed '$v_protoc', expected 'libprotoc $PROTOC_VERSION'"
    rustup_version_ok || die "rustup --version does not report $RUSTUP_VERSION: '$(version_of rustup --version)'"
    v_rustup="$(rustup --version 2>/dev/null)"
    v_rustup="${v_rustup%%$'\n'*}"
    v_rustc="$(rustc --version)"
    [[ "$v_rustc" == "rustc $RUST_TOOLCHAIN" || "$v_rustc" == "rustc $RUST_TOOLCHAIN "* ]] \
        || die "rustc --version printed '$v_rustc', expected it to start with 'rustc $RUST_TOOLCHAIN'"
    printf '    cargo: %s\n' "$(cargo --version)"

    printf '\n    %-12s %-40s %s\n' "tool" "version" "download"
    printf '    %-12s %-40s %s\n' "node" "$v_node" "sha256 verified (+ gpgv)"
    printf '    %-12s %-40s %s\n' "pnpm" "$v_pnpm" "sha256 verified"
    printf '    %-12s %-40s %s\n' "protoc" "$v_protoc" "sha256 verified"
    printf '    %-12s %-40s %s\n' "rustup" "$v_rustup" "sha256 verified"
    printf '    %-12s %-40s %s\n\n' "rustc" "$v_rustc" "via rustup (rustup's own checks)"
    printf '    limit: the Rust toolchain files under %s are not re-verified on re-runs; destroy and recreate the VM for a clean slate.\n' "$RUSTUP_HOME"
}

step_clone() {
    [[ $# -eq 2 ]] || die "usage: guest.sh clone <branch> <40-hex-commit-sha>"
    local branch="$1" sha="$2" branch_re='^[A-Za-z0-9._/-]+$' sha_re='^[0-9a-f]{40}$'
    [[ "$branch" =~ $branch_re && "$branch" != -* ]] || die "invalid branch name: '$branch'"
    [[ "$sha" =~ $sha_re ]] || die "invalid commit sha (need 40 lowercase hex characters): '$sha'"

    if [[ ! -e "$SRC" ]]; then
        git clone --branch "$branch" "$REPO_URL" "$SRC"
        cd "$SRC"
    else
        [[ -d "$SRC/.git" ]] || die "$SRC exists but is not a git checkout"
        cd "$SRC"
        local origin
        origin="$(git remote get-url origin)"
        [[ "$origin" == "$REPO_URL" ]] || die "origin of $SRC is '$origin', expected '$REPO_URL'"
        git fetch origin "$branch"
    fi

    local line path bad=() locks=() current
    while IFS= read -r line; do
        [[ -n "$line" ]] || continue
        path="${line:3}"
        case "$path" in
            Cargo.lock|pnpm-lock.yaml) locks+=("$path") ;;
            *) bad+=("$line") ;;
        esac
    done < <(git status --porcelain=v1 --untracked-files=no)
    if [[ ${#bad[@]} -gt 0 ]]; then
        printf '    tracked files modified in %s:\n' "$SRC" >&2
        printf '      %s\n' "${bad[@]}" >&2
        die "refusing to check out over modified tracked files other than Cargo.lock/pnpm-lock.yaml"
    fi
    if [[ ${#locks[@]} -gt 0 ]]; then
        current="$(git rev-parse HEAD)"
        if [[ "$current" == "$sha" ]]; then
            printf '    keeping locally modified lockfile(s): %s (requested sha is the current HEAD)\n' "${locks[*]}"
        else
            git checkout HEAD -- "${locks[@]}"
            printf '    discarded local modifications to: %s (requested sha %s is not the current HEAD %s)\n' \
                "${locks[*]}" "$sha" "$current"
        fi
    fi

    git checkout --detach "$sha"
    local head
    head="$(git rev-parse HEAD)"
    [[ "$head" == "$sha" ]] || die "HEAD is $head after checkout, expected $sha"
    git log -1 --oneline
}

# One line per [[package]] block: "name version|source|checksum" (local crates leave the last two empty).
cargo_pairs() {
    awk 'function flush() { if (n != "") print n " " v "|" s "|" c; n = ""; v = ""; s = ""; c = "" }
         /^\[/          { flush(); next }
         /^name = /     { n = $3; gsub(/"/, "", n); next }
         /^version = /  { v = $3; gsub(/"/, "", v); next }
         /^source = /   { s = $3; gsub(/"/, "", s); next }
         /^checksum = / { c = $3; gsub(/"/, "", c); next }
         END            { flush() }' "$1" | LC_ALL=C sort -u
}

# One line per key under packages: "name version|resolution" with all spaces removed from the resolution.
pnpm_pairs() {
    awk 'function flush() { if (k != "") print k "|" r; k = ""; r = "" }
         /^packages:[[:space:]]*$/      { f = 1; next }
         f && /^[^[:space:]]/           { flush(); f = 0 }
         f && /^  [^[:space:]]/         { flush(); k = $0; sub(/^  /, "", k); sub(/:[[:space:]]*$/, "", k); next }
         f && k != "" && /^    resolution:/ { r = $0; sub(/^    resolution:/, "", r); gsub(/ /, "", r); next }
         END                            { flush() }' "$1" \
        | sed -E "s/^['\"]([^|]*)['\"]\\|/\\1|/; s/^([^|]+)@([^@|]+)\\|/\\1 \\2|/" | LC_ALL=C sort -u
}

cargo_source_warnings() {
    local warnings
    warnings="$(awk -F'|' '$2 != "" && $2 != "registry+https://github.com/rust-lang/crates.io-index" {
        print "WARNING: Cargo.lock: " $1 " has non-registry source " $2 }' "$1")"
    if [[ -n "$warnings" ]]; then printf '%s\n' "$warnings"; else printf '    Cargo.lock: no non-registry sources found\n'; fi
}

pnpm_source_warnings() {
    local warnings
    warnings="$(awk '{ i = index($0, "|"); r = substr($0, i + 1) }
        r ~ /tarball:|commit:|repo:/ || r !~ /integrity:/ {
        print "WARNING: pnpm-lock.yaml (best effort): " substr($0, 1, i - 1) " has non-registry or unverified resolution " r }' "$1")"
    if [[ -n "$warnings" ]]; then printf '%s\n' "$warnings"; else printf '    pnpm-lock.yaml (best effort): no non-registry sources found\n'; fi
}

version_diff() {
    local before="$1" after="$2" label="$3" removed added changed
    removed="$(LC_ALL=C comm -23 "$before" "$after")"
    added="$(LC_ALL=C comm -13 "$before" "$after")"
    printf '    %s: removed (%s):\n' "$label" "$(printf '%s' "$removed" | grep -c . || true)"
    [[ -z "$removed" ]] || printf '%s\n' "$removed" | sed 's/^/      - /'
    printf '    %s: added (%s):\n' "$label" "$(printf '%s' "$added" | grep -c . || true)"
    [[ -z "$added" ]] || printf '%s\n' "$added" | sed 's/^/      + /'
    changed="$(LC_ALL=C comm -12 \
        <(printf '%s\n' "$removed" | awk 'NF { print $1 }' | LC_ALL=C sort -u) \
        <(printf '%s\n' "$added" | awk 'NF { print $1 }' | LC_ALL=C sort -u))"
    if [[ -z "$changed" ]]; then
        printf '    %s: no package name is in both lists, so nothing was CHANGED\n' "$label"
        return 0
    fi
    local name old new
    while IFS= read -r name; do
        old="$(printf '%s\n' "$removed" | awk -v n="$name" '$1 == n { printf "%s ", $2 }')"
        new="$(printf '%s\n' "$added" | awk -v n="$name" '$1 == n { printf "%s ", $2 }')"
        printf '    CHANGED: %s %s-> %s\n' "$name" "$old" "$new"
    done <<< "$changed"
}

lock_cargo() {
    local work="$1"
    git show HEAD:Cargo.lock > "$work/Cargo.lock.head"
    printf '    Cargo.lock [[package]] count: committed=%s, working tree before=%s\n' \
        "$(grep -c '^\[\[package\]\]' "$work/Cargo.lock.head" || true)" \
        "$(grep -c '^\[\[package\]\]' Cargo.lock || true)"
    cargo update --workspace
    printf '    Cargo.lock [[package]] count after: %s\n' "$(grep -c '^\[\[package\]\]' Cargo.lock || true)"
    printf '    Cargo.lock name version|source|checksum changes vs committed version:\n'
    cargo_pairs "$work/Cargo.lock.head" > "$work/cargo.before"
    cargo_pairs Cargo.lock > "$work/cargo.after"
    version_diff "$work/cargo.before" "$work/cargo.after" "Cargo.lock"
    cargo_source_warnings "$work/cargo.after"
}

lock_pnpm() {
    local work="$1"
    git show HEAD:pnpm-lock.yaml > "$work/pnpm-lock.yaml.head"
    printf '    pnpm-lock.yaml line count: committed=%s, working tree before=%s\n' \
        "$(wc -l < "$work/pnpm-lock.yaml.head")" "$(wc -l < pnpm-lock.yaml)"
    pnpm install --lockfile-only --no-frozen-lockfile --ignore-scripts
    printf '    pnpm-lock.yaml line count after: %s\n' "$(wc -l < pnpm-lock.yaml)"
    printf '    pnpm-lock.yaml packages: name version|resolution changes vs committed version (BEST EFFORT, parsed with awk/sed, not a YAML parser):\n'
    pnpm_pairs "$work/pnpm-lock.yaml.head" > "$work/pnpm.before"
    pnpm_pairs pnpm-lock.yaml > "$work/pnpm.after"
    version_diff "$work/pnpm.before" "$work/pnpm.after" "pnpm-lock.yaml (best effort)"
    pnpm_source_warnings "$work/pnpm.after"
}

step_lock() {
    require_no_args "$@"
    load_env
    enter_src
    local work
    work="$(mktemp -d)"
    run_part cargo lock_cargo "$work"
    run_part pnpm lock_pnpm "$work"
    printf '    git diff --stat:\n'
    git diff --stat -- Cargo.lock pnpm-lock.yaml
    rm -rf -- "$work"
    printf '    sha256 of the lockfiles in the VM (compare with host.sh pull-lockfiles):\n'
    sha256sum Cargo.lock pnpm-lock.yaml
    end_parts
}

fetch_cargo() {
    local triple
    triple="$(host_triple)"
    printf '    target: %s\n' "$triple"
    cargo fetch --locked --target "$triple"
}

step_fetch() {
    require_no_args "$@"
    load_env
    enter_src
    run_part cargo fetch_cargo
    run_part pnpm pnpm install --frozen-lockfile --ignore-scripts
    end_parts
}

# turbo restores only dist on a cache hit and passes no RELEASE, so the copy that is packaged is always built here.
build_extension() {
    local zip="$GNOME_EXT_DIR/$GNOME_EXT_UUID.zip"
    rm -f -- "$zip"
    ( cd "$GNOME_EXT_DIR" && RELEASE=1 pnpm run build )
    [[ -f "$zip" && ! -L "$zip" ]] || die "$zip was not produced"
}

step_web() {
    require_no_args "$@"
    load_env
    enter_src
    rm -f -- "$STAMPS/web"
    run_part build pnpm run build
    run_part extension build_extension
    run_part test:ci pnpm run test:ci
    end_parts
    write_stamp web
}

# Nothing in the specs checkout is installed or executed (no pnpm/npm/node there); the compiler only reads it.
fetch_specs() {
    local pin="$1" head
    rm -rf -- "$SPECS_SRC"
    git init -q "$SPECS_SRC"
    git -C "$SPECS_SRC" remote add origin "$SPECS_REPO_URL"
    GIT_TERMINAL_PROMPT=0 git -C "$SPECS_SRC" fetch --depth 1 origin "$pin" \
        || die "cannot fetch $pin from $SPECS_REPO_URL (is the commit pushed and the repository public?)"
    git -C "$SPECS_SRC" checkout -q --detach FETCH_HEAD
    head="$(git -C "$SPECS_SRC" rev-parse HEAD)"
    [[ "$head" == "$pin" ]] || die "specs HEAD is $head after checkout, expected $pin"
    git -C "$SPECS_SRC" log -1 --oneline
}

step_specs() {
    require_no_args "$@"
    load_env
    enter_src
    local pin_file="packages/spec-compiler/SPECS_COMMIT" sha_re='^[0-9a-f]{40}$' lines pin
    [[ -f "$pin_file" ]] || die "$pin_file not found"
    lines="$(wc -l < "$pin_file")"
    pin="$(< "$pin_file")"
    [[ "$lines" -eq 1 && "$pin" =~ $sha_re ]] \
        || die "$pin_file must be exactly one line holding a 40 lowercase hex commit sha"
    printf '    specs pin: %s\n' "$pin"
    rm -f -- "$STAMPS/specs"
    fetch_specs "$pin"

    local out="$SRC/build/specs" license="$SPECS_SRC/LICENSE" tree
    rm -rf -- "$out"
    mkdir -p -- "$SRC/build"
    [[ -d packages/spec-compiler/node_modules/esbuild ]] \
        || die "packages/spec-compiler/node_modules/esbuild is missing; run the 'fetch' step first"
    [[ -d "$SPECS_SRC/src" && ! -L "$SPECS_SRC/src" ]] || die "$SPECS_SRC/src is not a real directory"
    node packages/spec-compiler/src/cli.js --src "$SPECS_SRC/src" --out "$out"
    # The checkout is untrusted: a symlinked LICENSE could pull in any file from the VM.
    [[ -f "$license" && ! -L "$license" ]] || die "$license is not a regular file"
    cp -P -- "$license" "$out/LICENSE"
    printf '    .js files: %s\n' "$(find "$out" -type f -name '*.js' | wc -l)"
    printf '    size: %s\n' "$(du -sh -- "$out" | cut -f1)"
    printf '    index.json sha256: %s\n' "$(sha256_of "$out/index.json")"
    tree="$(cd "$out" && find . -type f -print0 | LC_ALL=C sort -z | xargs -0 sha256sum | sha256sum)"
    printf '    tree sha256 (sha256sum of the sorted per-file sha256sum list): %s\n' "${tree%% *}"
    write_stamp specs
}

step_icons() {
    local mode="${1:-}"
    [[ $# -le 1 && ( -z "$mode" || "$mode" == "check" ) ]] || die "usage: guest.sh icons [check]"
    enter_src
    local t manifest="scripts/icons/manifest.txt"
    for t in rsvg-convert png2icns; do
        [[ -n "$(type -P "$t" || true)" ]] || die "'$t' not on PATH; run the 'tools' step first"
    done
    [[ -f "$manifest" ]] || die "$manifest not found"
    dpkg-query -W librsvg2-bin icnsutils | sed 's/^/    /'
    rm -rf -- "$ICONS_OUT"
    bash scripts/icons/generate.sh "$ICONS_OUT"

    local kind width height path same=0 differs=0 missing=0
    while read -r kind width height path || [[ -n "$kind" ]]; do
        if [[ ! -e "$SRC/$path" ]]; then
            missing=$((missing + 1))
            printf '    not in repo: %s\n' "$path"
        elif cmp -s -- "$ICONS_OUT/$path" "$SRC/$path"; then
            same=$((same + 1))
            printf '    same: %s\n' "$path"
        else
            differs=$((differs + 1))
            printf '    DIFFERS: %s\n' "$path"
        fi
    done < "$manifest"
    printf '    generated vs committed: %s same, %s differ, %s not in repo\n' "$same" "$differs" "$missing"
    if [[ "$mode" == "check" && $((differs + missing)) -ne 0 ]]; then
        die "committed icons do not match what artwork/*.svg generates"
    fi
}

step_rust_check() {
    require_no_args "$@"
    load_env
    enter_src
    printf -- '--> cargo check is running (several minutes on a first run); the short terminal view shows nothing more until it ends\n'
    cargo check --locked --workspace --all-targets --exclude fig_desktop-fuzz
}

step_rust_test() {
    require_no_args "$@"
    load_env
    enter_src
    local rc=0 v set_vars=()
    # fig_os_shim Env::in_ssh treats any of these as a remote session, which breaks directories::linux_tests::all_paths.
    for v in SSH_CLIENT SSH_CONNECTION SSH_TTY; do
        [[ -z "${!v+x}" ]] || set_vars+=("$v")
    done
    printf '    SSH session variables set (unset for cargo test): %s\n' "${set_vars[*]:-none}"
    printf -- '--> cargo test is running (limit 90 minutes); the short terminal view shows nothing more until it ends\n'
    timeout --kill-after=60 5400 env -u SSH_CLIENT -u SSH_CONNECTION -u SSH_TTY \
        cargo test --locked --no-fail-fast --workspace --lib --bins --test '*' --exclude fig_desktop-fuzz || rc=$?
    if [[ $rc -eq 124 || $rc -eq 137 ]]; then
        printf '    the test run exceeded 90 minutes and was stopped (timeout exit status %s)\n' "$rc"
    fi
    [[ $rc -eq 0 ]] || exit "$rc"
}

# A program must be a plain ELF file for this machine before it is packaged.
require_program() {
    local f="$1" magic machine want
    [[ -f "$f" && ! -L "$f" && -x "$f" ]] || die "$f is missing, is not a regular file, or is not executable"
    magic="$(od -An -tx1 -N4 -- "$f" | tr -d ' \n')"
    [[ "$magic" == "7f454c46" ]] || die "$f is not an ELF program"
    machine="$(od -An -tx1 -j18 -N2 -- "$f" | tr -d ' \n')"
    case "$(uname -m)" in
        aarch64) want="b700" ;;
        x86_64) want="3e00" ;;
        *) die "unsupported architecture: $(uname -m)" ;;
    esac
    [[ "$machine" == "$want" ]] || die "$f is built for another machine type (ELF e_machine bytes $machine, expected $want)"
}

step_build() {
    require_no_args "$@"
    load_env
    enter_src
    require_clean_tree
    check_no_host_mounts
    rm -f -- "$STAMPS/build"
    local triple pair pkg
    triple="$(host_triple)"
    # Same settings as the build script inherited from Amazon.
    export CARGO_INCREMENTAL=0 CARGO_PROFILE_RELEASE_LTO=thin
    export RUSTFLAGS="-C force-frame-pointers=yes -C link-arg=-Wl,--compress-debug-sections=zlib"
    export SHELLCUE_BUILD_TARGET_TRIPLE="$triple"
    SHELLCUE_BUILD_HASH="$(git rev-parse HEAD)"
    # The commit's own time: a changing value would recompile every crate of ours on each run.
    SHELLCUE_BUILD_DATETIME="$(TZ=UTC git log -1 --date=format-local:%Y-%m-%dT%H:%M:%SZ --format=%cd)"
    [[ -n "$SHELLCUE_BUILD_DATETIME" ]] || die "cannot read the time of the checked-out commit"
    export SHELLCUE_BUILD_HASH SHELLCUE_BUILD_DATETIME
    printf '    target: %s, release profile with thin LTO\n' "$triple"
    printf '    build hash: %s\n    build time (the time of the commit): %s\n' "$SHELLCUE_BUILD_HASH" "$SHELLCUE_BUILD_DATETIME"
    printf -- '--> cargo build --release is running (well over ten minutes on a first run); the short terminal view shows nothing more until it ends\n'
    cargo build --locked --release -p fig_desktop -p q_cli -p figterm
    for pair in $PROGRAMS; do
        pkg="${pair%%:*}"
        require_program "target/release/$pkg"
        printf '    %s sha256: %s\n' "target/release/$pkg" "$(sha256_of "target/release/$pkg")"
    done
    write_stamp build
}

workspace_version() {
    local v version_re='^[0-9]+\.[0-9]+\.[0-9]+$'
    v="$(sed -n '/^\[workspace\.package\]/,/^\[/s/^version = "\(.*\)"$/\1/p' Cargo.toml)"
    [[ "$v" =~ $version_re ]] || die "cannot read a plain x.y.z version from [workspace.package] in Cargo.toml (got '$v')"
    printf '%s' "$v"
}

# Build outputs come from third-party build tools: only plain files and folders are copied into the package.
copy_plain_tree() {
    local src="$1" dest="$2" odd
    [[ -d "$src" && ! -L "$src" ]] || die "$src is not a real directory"
    odd="$(find "$src" ! -type f ! -type d -print | sed -n 1,5p)"
    [[ -z "$odd" ]] || die "$src holds something that is neither a plain file nor a folder: $odd"
    [[ ! -e "$dest" ]] || die "$dest already exists"
    cp -R -- "$src" "$dest"
}

copy_plain_file() {
    local src="$1" dest="$2" mode="$3"
    [[ -f "$src" && ! -L "$src" ]] || die "$src is missing or is not a regular file"
    install -D -m "$mode" -- "$src" "$dest"
}

stage_extension() {
    local dest="$1" zip="$GNOME_EXT_DIR/$GNOME_EXT_UUID.zip" odd need uuid
    grep -qF "\"$GNOME_EXT_UUID\"" crates/dbus/src/dbus/gnome_shell.rs || die "crates/dbus/src/dbus/gnome_shell.rs does not name the uuid $GNOME_EXT_UUID"
    [[ -f "$zip" && ! -L "$zip" ]] || die "$zip is missing; run the 'web' step"
    [[ ! -e "$dest" ]] || die "$dest already exists"
    mkdir -p -- "$dest"
    unzip -q "$zip" -d "$dest"
    odd="$(find "$dest" ! -type f ! -type d -print | sed -n 1,5p)"
    [[ -z "$odd" ]] || die "$zip holds something that is neither a plain file nor a folder: $odd"
    for need in metadata.json extension.js schemas/gschemas.compiled; do
        [[ -f "$dest/$need" ]] || die "$zip has no $need"
    done
    uuid="$(jq -r .uuid "$dest/metadata.json")"
    [[ "$uuid" == "$GNOME_EXT_UUID" ]] || die "$zip is for uuid '$uuid', expected '$GNOME_EXT_UUID'"
    # The first line is what $GNOME_EXT_DIR/build-scripts/extension.js prepends to a release build.
    [[ "$(head -n 1 "$dest/extension.js")" == "const DEBUG=false;" ]] || die "$zip is a debug build; run the 'web' step"
}

stage_deb() {
    local stage="$1" version="$2" arch="$3" pair pkg name share="$1/usr/share" control="$1/DEBIAN/control" kb
    for pair in $PROGRAMS; do
        pkg="${pair%%:*}"
        name="${pair##*:}"
        require_program "target/release/$pkg"
        copy_plain_file "target/release/$pkg" "$stage/usr/bin/$name" 0755
    done
    copy_plain_file bundle/deb/shellcue.desktop "$share/applications/shellcue.desktop" 0644
    copy_plain_file crates/fig_desktop/icons/128x128.png "$share/icons/hicolor/128x128/apps/shellcue.png" 0644
    mkdir -p -- "$share/shellcue"
    [[ -f packages/autocomplete-app/dist/index.html ]] || die "packages/autocomplete-app/dist/index.html is missing; run the 'web' step"
    copy_plain_tree packages/autocomplete-app/dist "$share/shellcue/autocomplete"
    [[ -f build/specs/index.json ]] || die "build/specs/index.json is missing; run the 'specs' step"
    copy_plain_tree build/specs "$share/shellcue/specs"
    stage_extension "$share/gnome-shell/extensions/$GNOME_EXT_UUID"
    chmod -R u=rwX,go=rX -- "$stage"

    copy_plain_file bundle/deb/prerm "$stage/DEBIAN/prerm" 0755
    copy_plain_file bundle/deb/postrm "$stage/DEBIAN/postrm" 0755
    chmod 0755 -- "$stage/DEBIAN"
    [[ -f bundle/deb/control && ! -L bundle/deb/control ]] || die "bundle/deb/control is missing or is not a regular file"
    kb="$(du -sk --exclude=DEBIAN -- "$stage" | cut -f1)"
    sed -e 's/\$VERSION/'"$version"'/' -e 's/\$APT_ARCH/'"$arch"'/' bundle/deb/control > "$control"
    printf 'Installed-Size: %s\n' "$kb" >> "$control"
    chmod 0644 -- "$control"
    ! grep -q '\$' "$control" || die "$control still holds an unfilled \$ placeholder"
    grep -qx "Version: $version" "$control" || die "$control has no line 'Version: $version'"
    grep -qx "Architecture: $arch" "$control" || die "$control has no line 'Architecture: $arch'"
}

step_bundle() {
    require_no_args "$@"
    enter_src
    require_clean_tree
    check_no_host_mounts
    require_stamps web specs build
    local version arch name stage deb sum
    version="$(workspace_version)"
    arch="$(dpkg --print-architecture)"
    case "$arch" in
        arm64|amd64) ;;
        *) die "unexpected Debian architecture: '$arch'" ;;
    esac
    name="shellcue_${version}_${arch}"
    stage="$OUT/$name"
    deb="$OUT/$name.deb"
    printf -- '--> version %s, architecture %s\n' "$version" "$arch"

    [[ "$OUT" == "$HOME/shellcue/build/linux" ]] || die "refusing to clear an unexpected output folder: $OUT"
    rm -rf -- "$OUT"
    mkdir -p -- "$stage"
    stage_deb "$stage" "$version" "$arch"

    printf '    control:\n'
    sed 's/^/      /' "$stage/DEBIAN/control"
    printf '    programs:\n'
    for name in shellcue-desktop shellcue shellcue-term; do
        printf '      %s  usr/bin/%s\n' "$(sha256_of "$stage/usr/bin/$name")" "$name"
    done
    printf '    extension: %s for GNOME Shell %s\n' "$GNOME_EXT_UUID" "$(jq -c '."shell-version"' "$stage/usr/share/gnome-shell/extensions/$GNOME_EXT_UUID/metadata.json")"
    printf '    files: %s, size: %s\n' "$(find "$stage" -type f | wc -l)" "$(du -sh -- "$stage" | cut -f1)"

    printf -- '--> making the package (a minute or two)\n'
    dpkg-deb --build --root-owner-group "$stage" "$deb"
    dpkg-deb --info "$deb" | sed 's/^/      /'
    printf '    package contents outside usr/share/shellcue:\n'
    dpkg-deb --contents "$deb" | grep -v ' \./usr/share/shellcue/.' | sed 's/^/      /'
    sum="$(sha256_of "$deb")"
    printf '%s  %s\n' "$sum" "${deb##*/}" > "$deb.sha256"
    printf '    package size: %s\n' "$(du -h -- "$deb" | cut -f1)"
    printf -- '--> package %s sha256 %s\n' "$deb" "$sum"
    printf -- '--> the package stays in the VM; nothing is copied to the Mac. It is not signed.\n'
    printf -- '--> to install it for a trial: take a UTM snapshot first, then run in the Terminal of the VM:   sudo apt install ~/shellcue/build/linux/%s\n' "${deb##*/}"
}

step_info() {
    require_no_args "$@"
    printf '    os: %s\n' "$( (. /etc/os-release && printf '%s' "${PRETTY_NAME:-unknown}") 2>/dev/null || printf 'unknown')"
    printf '    kernel/arch: %s %s\n' "$(uname -r)" "$(uname -m)"
    if [[ -f "$ENV_FILE" ]]; then
        # shellcheck source=/dev/null
        . "$ENV_FILE"
        local t out
        for t in node pnpm protoc rustup rustc cargo; do
            out="$(cd "$HOME" && version_of "$t" --version)"
            out="${out%%$'\n'*}"
            printf '    %-7s %s\n' "$t:" "${out:-not found}"
        done
    else
        printf '    tools: %s missing (run the tools step)\n' "$ENV_FILE"
    fi
    printf '    disk:\n'
    df -h "$HOME" | sed 's/^/      /'
    if [[ -d "$SRC/.git" ]]; then
        printf '    checkout: %s\n' "$(git -C "$SRC" log -1 --format='%H %s')"
        printf '    git status --short:\n'
        git -C "$SRC" status --short | sed 's/^/      /'
    else
        printf '    checkout: none at %s\n' "$SRC"
    fi
}

main() {
    if [[ $# -eq 0 || "$1" == "--help" || "$1" == "-h" ]]; then
        usage
        exit 0
    fi
    local step="$1"
    shift
    case " $STEPS " in
        *" $step "*) ;;
        *) usage >&2; die "unknown step: '$step'" ;;
    esac
    begin_step "$step"
    [[ "$step" == "info" ]] || take_lock
    case "$step" in
        tools) step_tools "$@" ;;
        clone) step_clone "$@" ;;
        lock) step_lock "$@" ;;
        fetch) step_fetch "$@" ;;
        web) step_web "$@" ;;
        specs) step_specs "$@" ;;
        icons) step_icons "$@" ;;
        rust-check) step_rust_check "$@" ;;
        rust-test) step_rust_test "$@" ;;
        build) step_build "$@" ;;
        bundle) step_bundle "$@" ;;
        info) step_info "$@" ;;
        *) die "step '$step' is listed in STEPS but has no handler" ;;
    esac
}

main "$@"
