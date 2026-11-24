#!/bin/bash
# Runs INSIDE the throwaway macOS build VM as its login user; mac-host.sh copies it in. Must stay compatible with macOS /bin/bash 3.2.

set -euo pipefail

# Files left in the home folder by an earlier build must not be able to shadow the system tools.
export PATH="/usr/bin:/bin:/usr/sbin:/sbin"

# Every pnpm install passes an explicit lockfile flag because CI=true changes pnpm's default.
export CI=true NO_COLOR=1 FORCE_COLOR=0 TERM=dumb TURBO_TELEMETRY_DISABLED=1 DO_NOT_TRACK=1

REPO_URL="https://github.com/abandisch/shellcue.git"
SPECS_REPO_URL="https://github.com/abandisch/shellcue-specs.git"
SPECS_SRC="$HOME/shellcue-specs"
TOOLS="$HOME/tools"
DL="$TOOLS/dl"
ENV_FILE="$TOOLS/env.sh"
SRC="$HOME/shellcue"
LOCK_DIR="$HOME/.shellcue-mac-guest.lock"
TEST_LIMIT_SECS=5400
STAMPS="$SRC/build/stamps"
OUT="$SRC/build/macos"
APP_BUNDLE_ID="com.bandisch-apps.shellcue"
IME_BUNDLE_ID="com.bandisch-apps.inputmethod.shellcue"
# Apple Silicon needs macOS 11 or later; Rust's own floor for this target is the same.
MIN_MACOS="11.0"
# The signing identity lives outside the checkout and is never copied out of the VM.
SIGN_DIR="$HOME/shellcue-signing"
SIGN_KEYCHAIN="$SIGN_DIR/signing.keychain-db"
SIGN_PASS_FILE="$SIGN_DIR/keychain-password"
SIGN_CERT="$SIGN_DIR/certificate.pem"
SIGN_READY="$SIGN_DIR/ready"
SIGN_WORK="$SIGN_DIR/work"
SIGN_NAME="ShellCue Local Signing"
SIGN_DAYS=3650
SIGN_LIMIT_SECS=300
# macOS ties the permissions it grants to these identifiers and the certificate; changing either loses the grants.
CLI_SIGN_ID="$APP_BUNDLE_ID.cli"
TERM_SIGN_ID="$APP_BUNDLE_ID.term"
SIGN_REPAIR="If it cannot be put right, the identity has to be replaced: in the VM's own Terminal run: rm -rf $SIGN_DIR   and then, on the Mac: scripts/vm/mac-host.sh step sign-setup   That destroys the key (it has no copy) and macOS forgets every permission granted to ShellCue"

# Change together with scripts/vm/guest.sh, .mise.toml (node, pnpm, protoc), package.json (packageManager) and rust-toolchain.toml (Rust); enter_src enforces it.
NODE_VERSION="26.13.0"
PNPM_VERSION="12.17.0"
PROTOC_VERSION="36.2"
RUSTUP_VERSION="1.29.3"
RUST_TOOLCHAIN="1.87.0"

RUST_TRIPLE="aarch64-apple-darwin"
NODE_SHA256="d38bd5e5a2ddce6bbb5ce45e46e208337da4d51fffe611e567fd32073d6439f6"
PNPM_SHA256="446c334d493bdfc6cc2db693764d1812fca06456f6a4be7996a13d7008780239"
PROTOC_SHA256="9cd98a532c5c5e0c4161314de0225de27e4c8a323917b6ea7b1b714d3ae23466"
RUSTUP_SHA256="63752a1919a3a4793173ec70f8f212621a7ce38ba6a246095984482f4de20f9f"
NODE_TARBALL="node-v$NODE_VERSION-darwin-arm64.tar.gz"
NODE_URL="https://nodejs.org/dist/v$NODE_VERSION/$NODE_TARBALL"
PNPM_URL="https://github.com/pnpm/pnpm/releases/download/v$PNPM_VERSION/pnpm-darwin-arm64.tar.gz"
PROTOC_URL="https://github.com/protocolbuffers/protobuf/releases/download/v$PROTOC_VERSION/protoc-$PROTOC_VERSION-osx-aarch_64.zip"
RUSTUP_URL="https://static.rust-lang.org/rustup/archive/$RUSTUP_VERSION/$RUST_TRIPLE/rustup-init"

STEPS="tools clone fetch web specs rust-check rust-test build sign-setup bundle info"

usage() {
    cat <<'EOF'
Usage: mac-guest.sh <step> [args]

Runs inside the ShellCue macOS build VM (Apple Silicon). Steps:
  tools                    pinned, sha256-verified node, pnpm, protoc and rustup, then the pinned Rust toolchain through rustup;
                           writes ~/tools/env.sh. Needs Apple's Command Line Tools already installed (xcode-select --install).
  clone <branch> <sha>     clone/fetch the public repo into ~/shellcue and check out <sha> detached
  fetch                    cargo fetch --locked --target <host triple> and pnpm install --frozen-lockfile --ignore-scripts
  web                      pnpm run build and pnpm run test:ci (both always run)
  specs                    fetch the specs repo at the commit in packages/spec-compiler/SPECS_COMMIT and compile it
                           into ~/shellcue/build/specs; needs 'fetch' first (nothing from the specs repo is installed or run)
  rust-check               cargo check --locked --workspace --all-targets --exclude fig_desktop-fuzz
  rust-test                cargo test --locked --no-fail-fast --workspace --lib --bins --test '*' --exclude fig_desktop-fuzz,
                           with SSH_CLIENT, SSH_CONNECTION and SSH_TTY unset; stopped after 90 minutes
  build                    cargo build --locked --release of the four programs that go into the app (arm64 only)
  sign-setup               once per VM: make a self-signed code-signing certificate and its key, in their own keychain
                           under ~/shellcue-signing, and prove the identity by signing and running a throwaway program.
                           Refuses to replace an existing identity. The key has no copy outside this VM.
  bundle                   assemble ~/shellcue/build/macos/ShellCue.app from the outputs of web, specs and build, each of
                           which must have succeeded at the checked-out commit; sign it with the identity from sign-setup
                           (hardened runtime, no entitlements, no timestamp, not notarised); then make ShellCue.dmg.
                           The disk image itself is not signed. The app and the disk image stay in the VM.
  info                    OS, arch, Command Line Tools, tool versions, disk free, signing identity, checked-out commit, git status

Every step first refuses to run if the VM has a shared or network folder mounted. Every step except info also
refuses to run while an earlier step, or a cargo, rustc, node or pnpm process it left behind, is still running.
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
SIGN_UNLOCKED=0
SIGN_WORK_OWNED=0

# Must stay the last thing printed, whatever ended the run.
print_verdict() {
    local rc=$? secs=$((SECONDS - STEP_START))
    if [[ $SIGN_UNLOCKED -eq 1 ]]; then security lock-keychain "$SIGN_KEYCHAIN" >/dev/null 2>&1 || true; fi
    # The work folder holds the private key; only the sign-setup run that made it may remove it.
    if [[ $SIGN_WORK_OWNED -eq 1 ]]; then rm -rf -- "$SIGN_WORK" || true; fi
    if [[ $LOCK_HELD -eq 1 ]]; then rm -rf -- "$LOCK_DIR" || true; fi
    if [[ $rc -eq 0 ]]; then
        printf '==> ok: %s (%ss)\n' "$CURRENT_STEP" "$secs"
    else
        printf '==> FAILED: %s (exit %s, %ss)\n' "$CURRENT_STEP" "$rc" "$secs"
    fi
}

sha256_of() {
    local out
    out="$(shasum -a 256 -- "$1")"
    printf '%s' "${out%% *}"
}

begin_step() {
    CURRENT_STEP="$1"
    STEP_START=$SECONDS
    trap print_verdict EXIT
    printf '==> step: %s\n' "$CURRENT_STEP"
    printf '    time (UTC): %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    local self_sum
    self_sum="$(sha256_of "$0")"
    [[ -n "$self_sum" ]] || die "cannot compute the sha256 of $0"
    printf '    mac-guest.sh sha256: %s\n' "$self_sum"
}

# A step whose SSH connection dropped keeps running in the VM; a second step on top of it would share its build folders.
take_lock() {
    local holder="" name pids left=""
    if ! mkdir -- "$LOCK_DIR" 2>/dev/null; then
        holder="$(cat -- "$LOCK_DIR/pid" 2>/dev/null || true)"
        case "$holder" in
            ''|*[!0-9]*) holder="" ;;
        esac
        if [[ -n "$holder" ]] && kill -0 "$holder" 2>/dev/null; then
            die "an earlier step is still running in the VM (process $holder), probably left by a dropped connection. Wait for it to end (mac-host.sh step info shows it), or stop it in the VM's own Terminal with: kill $holder"
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
        || die "build processes from an earlier step are still running in the VM ($left). Wait for them to end, or stop them in the VM's own Terminal with: kill <the numbers>"
}

# macOS has no timeout(1). The command runs in its own process group so that its children are stopped with it.
run_limited() {
    local secs="$1"
    shift
    /usr/bin/perl -e '
        my $secs = shift;
        my $pid = fork();
        die "fork failed: $!\n" unless defined $pid;
        if ($pid == 0) { setpgrp(0, 0); exec @ARGV or die "exec failed: $!\n"; }
        $SIG{ALRM} = sub { kill "TERM", -$pid; sleep 60; kill "KILL", -$pid; exit 142; };
        alarm $secs;
        waitpid($pid, 0);
        exit(($? & 127) ? 128 + ($? & 127) : $? >> 8);
    ' "$secs" "$@"
}

FAILED_PARTS=""

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
        FAILED_PARTS="$FAILED_PARTS $label"
    else
        printf -- '--> ok: %s\n' "$label"
    fi
}

end_parts() {
    if [[ -n "$FAILED_PARTS" ]]; then
        printf -- '--> failed parts:%s\n' "$FAILED_PARTS"
        exit 1
    fi
}

require_no_args() {
    [[ $# -eq 0 ]] || die "step '$CURRENT_STEP' takes no arguments (got: $*)"
}

check_os() {
    local name version arch
    name="$(sw_vers -productName 2>/dev/null || true)"
    version="$(sw_vers -productVersion 2>/dev/null || true)"
    arch="$(uname -m)"
    printf '    os: %s %s (%s)\n' "${name:-unknown}" "${version:-unknown}" "$arch"
    [[ "$name" == "macOS" ]] || die "this script only runs on macOS; sw_vers -productName printed '$name'"
    [[ "$arch" == "arm64" ]] || die "this script only runs on Apple Silicon; uname -m printed '$arch'"
}

# The build VM must share no folder with the host; an unreadable mount table fails closed.
check_no_shared_folders() {
    local out line shared=""
    out="$(/sbin/mount)" || die "cannot read the mount table (/sbin/mount failed)"
    [[ -n "$out" ]] || die "the mount table is empty; cannot confirm that no shared folder is mounted"
    while IFS= read -r line; do
        case "$line" in
            *"(apfs,"*|*"(apfs)"*|*"(devfs,"*|*"(devfs)"*|*"(autofs,"*|*"(autofs)"*) ;;
            *) shared="$shared"$'\n'"      $line" ;;
        esac
    done <<< "$out"
    if [[ -n "$shared" ]]; then
        printf '\n!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!\n' >&2
        printf '!!! THIS VM HAS A MOUNT THAT IS NOT A LOCAL DISK:%s\n' "$shared" >&2
        printf '!!! The build VM must have no shared or network folders. Stop the VM, remove the\n' >&2
        printf '!!! shared directory in its UTM settings (or eject the volume), and start again.\n' >&2
        printf '!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!\n\n' >&2
        exit 1
    fi
    printf '    mounts: only apfs, devfs and autofs (no shared or network folder)\n'
}

check_command_line_tools() {
    local dir clang
    dir="$(xcode-select -p 2>/dev/null)" \
        || die "Apple's Command Line Tools are not installed; in the VM's own Terminal run: xcode-select --install"
    clang="$(xcrun --find clang 2>/dev/null)" \
        || die "xcrun cannot find clang under '$dir'; in the VM's own Terminal run: xcode-select --install"
    printf '    developer dir: %s\n' "$dir"
    printf '    clang: %s\n' "$clang"
    pkgutil --pkg-info=com.apple.pkg.CLTools_Executables 2>/dev/null | sed -n 's/^version: /    command line tools: /p' \
        || printf '    command line tools: version unknown (pkgutil has no CLTools_Executables record)\n'
}

curl_https() {
    /usr/bin/curl -q --proto '=https' --proto-redir '=https' --tlsv1.2 --connect-timeout 30 --retry 3 \
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

version_of() {
    "$@" 2>/dev/null || true
}

install_node() {
    local dir="$TOOLS/node"
    rm -rf -- "$dir" "$dir.staging"
    mkdir -p -- "$dir.staging"
    tar -xzf "$DL/$NODE_TARBALL" -C "$dir.staging" --strip-components=1 --no-same-owner
    [[ -x "$dir.staging/bin/node" ]] || die "node tarball did not contain bin/node"
    mv -- "$dir.staging" "$dir"
}

# The pnpm 12 tarball layout is unconfirmed; the whole archive stays intact and its pnpm directory goes on PATH.
find_pnpm_dir() {
    local root="$1" f count=0 first=""
    while IFS= read -r -d '' f; do
        count=$((count + 1))
        [[ -n "$first" ]] || first="$f"
        printf '      %s\n' "$f" >&2
    done < <(find "$root" -name pnpm -type f -perm -u+x -print0)
    if [[ $count -ne 1 ]]; then
        printf '    executable files named pnpm under %s: %s (listed above)\n' "$root" "$count" >&2
        die "expected exactly one executable named 'pnpm' in the pnpm archive"
    fi
    dirname -- "$first"
}

install_pnpm() {
    local dir="$TOOLS/pnpm" tgz="$DL/pnpm-darwin.tar.gz"
    printf '    pnpm archive listing:\n'
    tar -tvzf "$tgz" | sed 's/^/      /'
    rm -rf -- "$dir" "$dir.staging"
    mkdir -p -- "$dir.staging"
    tar -xzf "$tgz" -C "$dir.staging" --no-same-owner
    mv -- "$dir.staging" "$dir"
    printf '    pnpm executable found:\n'
    PNPM_DIR="$(find_pnpm_dir "$dir")"
}

install_protoc() {
    local dir="$TOOLS/protoc" zip="$DL/protoc-osx.zip"
    printf '    protoc zip listing:\n'
    unzip -l "$zip" | sed 's/^/      /'
    rm -rf -- "$dir" "$dir.staging"
    mkdir -p -- "$dir.staging"
    unzip -q "$zip" -d "$dir.staging"
    [[ -f "$dir.staging/bin/protoc" ]] || die "protoc zip did not contain bin/protoc"
    [[ -d "$dir.staging/include" ]] || die "protoc zip did not contain include/"
    chmod u+x "$dir.staging/bin/protoc"
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
    chmod u+x "$DL/rustup-init"
    "$DL/rustup-init" -y --default-toolchain none --profile minimal --no-modify-path
}

write_env() {
    local path_prefix="$TOOLS/node/bin:$PNPM_DIR:$TOOLS/protoc/bin:$CARGO_HOME/bin"
    {
        printf '# Generated by mac-guest.sh tools; sourced by every later step.\n'
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
    die "$file does not agree with mac-guest.sh: mac-guest.sh pins '$key$want', $file has '${found:-no ${key% } line}'"
}

check_pins() {
    require_pin package.json '"packageManager": ' "\"pnpm@$PNPM_VERSION\""
    require_pin rust-toolchain.toml 'channel = ' "\"$RUST_TOOLCHAIN\""
    require_pin .mise.toml 'node = ' "\"$NODE_VERSION\""
    require_pin .mise.toml 'pnpm = ' "\"$PNPM_VERSION\""
    require_pin .mise.toml 'protoc = ' "\"$PROTOC_VERSION\""
}

enter_src() {
    [[ -d "$SRC/.git" ]] || die "no checkout at $SRC; on the Mac run: scripts/vm/mac-host.sh clone"
    cd "$SRC"
    printf '    checkout: %s @ %s\n' "$SRC" "$(git rev-parse HEAD)"
    check_pins
    local f
    for f in Cargo.lock pnpm-lock.yaml; do
        git diff --quiet HEAD -- "$f" \
            || die "$f differs from the committed version; lockfiles are refreshed only in the Linux VM. In the VM's own Terminal run: git -C ~/shellcue checkout HEAD -- $f"
    done
}

require_clean_tree() {
    local dirty
    dirty="$(git status --porcelain=v1 --untracked-files=no)"
    if [[ -n "$dirty" ]]; then
        printf '    tracked files modified in %s:\n' "$SRC" >&2
        printf '%s\n' "$dirty" | sed 's/^/      /' >&2
        die "refusing to work from modified tracked files. In the VM's own Terminal run: git -C ~/shellcue checkout HEAD -- .   and then run this step again"
    fi
}

# A stamp records the commit a step last succeeded at, so bundle never mixes outputs of different commits.
write_stamp() {
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
        || die "these steps have not succeeded at the checked-out commit $head:$missing. On the Mac run, in the order fetch web specs build, scripts/vm/mac-host.sh step NAME for fetch and for each step named here, then this step again"
}

host_triple() {
    local out triple
    out="$(rustc -vV)"
    triple="$(printf '%s\n' "$out" | sed -n 's/^host: //p')"
    [[ "$triple" == "$RUST_TRIPLE" ]] || die "rustc host triple is '$triple', expected '$RUST_TRIPLE'"
    printf '%s' "$triple"
}

step_tools() {
    require_no_args "$@"
    check_command_line_tools

    mkdir -p -- "$DL"
    export RUSTUP_HOME="$TOOLS/rustup"
    export CARGO_HOME="$TOOLS/cargo"

    printf -- '--> node %s\n' "$NODE_VERSION"
    fetch_verified "$NODE_URL" "$DL/$NODE_TARBALL" "$NODE_SHA256"
    printf -- '--> pnpm %s\n' "$PNPM_VERSION"
    fetch_verified "$PNPM_URL" "$DL/pnpm-darwin.tar.gz" "$PNPM_SHA256"
    printf -- '--> protoc %s\n' "$PROTOC_VERSION"
    fetch_verified "$PROTOC_URL" "$DL/protoc-osx.zip" "$PROTOC_SHA256"
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
    printf -- '--> installing the Rust toolchain %s; the short terminal view shows nothing more until it ends\n' "$RUST_TOOLCHAIN"
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
    local v_cargo v_triple
    v_cargo="$(cargo --version)"
    v_triple="$(host_triple)"
    printf '    cargo: %s\n' "$v_cargo"
    printf '    host triple: %s\n' "$v_triple"

    printf '\n    %-12s %-40s %s\n' "tool" "version" "download"
    printf '    %-12s %-40s %s\n' "node" "$v_node" "sha256 verified"
    printf '    %-12s %-40s %s\n' "pnpm" "$v_pnpm" "sha256 verified"
    printf '    %-12s %-40s %s\n' "protoc" "$v_protoc" "sha256 verified"
    printf '    %-12s %-40s %s\n' "rustup" "$v_rustup" "sha256 verified"
    printf '    %-12s %-40s %s\n\n' "rustc" "$v_rustc" "via rustup (rustup's own checks)"
    printf '    limit: Node has no second (gpgv) check here, because macOS ships no gpgv; its pinned hash was read from the\n'
    printf '    signature-verified SHASUMS256.txt in the Linux VM.\n'
    printf '    limit: the Rust toolchain files under %s are not re-verified on re-runs; restore the clean VM copy for a clean slate.\n' "$RUSTUP_HOME"
}

step_clone() {
    [[ $# -eq 2 ]] || die "usage: mac-guest.sh clone <branch> <40-hex-commit-sha>"
    local branch="$1" sha="$2" branch_re='^[A-Za-z0-9._/-]+$' sha_re='^[0-9a-f]{40}$'
    [[ "$branch" =~ $branch_re && "$branch" != -* ]] || die "invalid branch name: '$branch'"
    [[ "$sha" =~ $sha_re ]] || die "invalid commit sha (need 40 lowercase hex characters): '$sha'"
    check_command_line_tools

    if [[ ! -e "$SRC" ]]; then
        GIT_TERMINAL_PROMPT=0 git clone --branch "$branch" "$REPO_URL" "$SRC"
        cd "$SRC"
    else
        [[ -d "$SRC/.git" ]] || die "$SRC exists but is not a git checkout"
        cd "$SRC"
        local origin
        origin="$(git remote get-url origin)"
        [[ "$origin" == "$REPO_URL" ]] || die "origin of $SRC is '$origin', expected '$REPO_URL'"
        GIT_TERMINAL_PROMPT=0 git fetch origin "$branch"
    fi

    local dirty
    dirty="$(git status --porcelain=v1 --untracked-files=no)"
    if [[ -n "$dirty" ]]; then
        printf '    tracked files modified in %s:\n' "$SRC" >&2
        printf '%s\n' "$dirty" | sed 's/^/      /' >&2
        die "refusing to check out over modified tracked files"
    fi

    git checkout --detach "$sha"
    local head
    head="$(git rev-parse HEAD)"
    [[ "$head" == "$sha" ]] || die "HEAD is $head after checkout, expected $sha"
    git log -1 --oneline
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

step_web() {
    require_no_args "$@"
    load_env
    enter_src
    require_clean_tree
    rm -f -- "$STAMPS/web"
    run_part build pnpm run build
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

tree_sha256() {
    local sum
    sum="$(cd "$1" && find . -type f -print0 | LC_ALL=C sort -z | xargs -0 shasum -a 256 | shasum -a 256)"
    printf '%s' "${sum%% *}"
}

step_specs() {
    require_no_args "$@"
    load_env
    enter_src
    require_clean_tree
    rm -f -- "$STAMPS/specs"
    local pin_file="packages/spec-compiler/SPECS_COMMIT" sha_re='^[0-9a-f]{40}$' lines pin
    [[ -f "$pin_file" ]] || die "$pin_file not found"
    lines="$(wc -l < "$pin_file")"
    pin="$(< "$pin_file")"
    [[ "$lines" -eq 1 && "$pin" =~ $sha_re ]] \
        || die "$pin_file must be exactly one line holding a 40 lowercase hex commit sha"
    printf '    specs pin: %s\n' "$pin"
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
    [[ -f "$out/index.json" ]] || die "the spec compiler wrote no index.json into $out"
    printf '    .js files: %s\n' "$(find "$out" -type f -name '*.js' | wc -l | tr -d ' ')"
    printf '    size: %s\n' "$(du -sh -- "$out" | cut -f1)"
    printf '    index.json sha256: %s\n' "$(sha256_of "$out/index.json")"
    tree="$(tree_sha256 "$out")"
    [[ -n "$tree" ]] || die "cannot hash the files under $out"
    printf '    tree sha256 (sha256 of the sorted per-file sha256 list; the Linux VM prints the same figure): %s\n' "$tree"
    write_stamp specs
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
    local rc=0 v set_vars=""
    # fig_os_shim Env::in_ssh treats any of these as a remote session, and every step here arrives over SSH.
    for v in SSH_CLIENT SSH_CONNECTION SSH_TTY; do
        if printenv "$v" >/dev/null; then set_vars="$set_vars $v"; fi
    done
    printf '    SSH session variables set (unset for cargo test):%s\n' "${set_vars:- none}"
    # The Linux VM runs shellcheck over the same init text (the snapshot tests pin it); this VM has no shellcheck.
    printf -- '--> note: four shellcheck lint tests are compiled to skip (SHELLCUE_BUILD_SKIP_SHELLCHECK_TESTS); they count as passed without linting\n'
    printf -- '--> cargo test is running (limit %s minutes); the short terminal view shows nothing more until it ends\n' "$((TEST_LIMIT_SECS / 60))"
    run_limited "$TEST_LIMIT_SECS" env -u SSH_CLIENT -u SSH_CONNECTION -u SSH_TTY SHELLCUE_BUILD_SKIP_SHELLCHECK_TESTS=1 \
        cargo test --locked --no-fail-fast --workspace --lib --bins --test '*' --exclude fig_desktop-fuzz || rc=$?
    if [[ $rc -eq 142 ]]; then
        printf '    the test run exceeded %s minutes; cargo and the processes it started were stopped (exit status %s)\n' "$((TEST_LIMIT_SECS / 60))" "$rc"
    fi
    [[ $rc -eq 0 ]] || exit "$rc"
}

# Package name, then the name the program has inside the app; change together with crates/fig_util/src/consts.rs.
APP_PROGRAMS="fig_desktop:shellcue-desktop q_cli:shellcue figterm:shellcue-term"
IME_PROGRAM="fig_input_method"

require_arm64() {
    local archs
    [[ -f "$1" && ! -L "$1" ]] || die "$1 is missing or is not a regular file"
    archs="$(lipo -archs "$1")" || die "lipo cannot read $1"
    [[ "$archs" == "arm64" ]] || die "$1 holds '$archs', expected exactly 'arm64'"
}

step_build() {
    require_no_args "$@"
    load_env
    enter_src
    require_clean_tree
    rm -f -- "$STAMPS/build"
    local triple pair pkg
    triple="$(host_triple)"
    # Same settings as the build script inherited from Amazon, minus the Intel half of its universal build.
    export MACOSX_DEPLOYMENT_TARGET="$MIN_MACOS" CARGO_INCREMENTAL=0 CARGO_PROFILE_RELEASE_LTO=thin
    export RUSTFLAGS="-C force-frame-pointers=yes"
    export SHELLCUE_BUILD_TARGET_TRIPLE="$triple"
    SHELLCUE_BUILD_HASH="$(git rev-parse HEAD)"
    # The commit's own time: a changing value would recompile every crate of ours on each run.
    SHELLCUE_BUILD_DATETIME="$(TZ=UTC git log -1 --date=format-local:%Y-%m-%dT%H:%M:%SZ --format=%cd)"
    [[ -n "$SHELLCUE_BUILD_DATETIME" ]] || die "cannot read the time of the checked-out commit"
    export SHELLCUE_BUILD_HASH SHELLCUE_BUILD_DATETIME
    printf '    target: %s, minimum macOS %s, release profile with thin LTO\n' "$triple" "$MIN_MACOS"
    printf '    build hash: %s\n    build time (the time of the commit): %s\n' "$SHELLCUE_BUILD_HASH" "$SHELLCUE_BUILD_DATETIME"
    printf -- '--> cargo build --release is running (well over ten minutes on a first run); the short terminal view shows nothing more until it ends\n'
    cargo build --locked --release -p fig_desktop -p q_cli -p figterm -p "$IME_PROGRAM"
    for pair in $APP_PROGRAMS "$IME_PROGRAM:$IME_PROGRAM"; do
        pkg="${pair%%:*}"
        require_arm64 "target/release/$pkg"
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

# Build outputs come from third-party build tools: only plain files and folders are copied into the app.
copy_plain_tree() {
    local src="$1" dest="$2" odd
    [[ -d "$src" && ! -L "$src" ]] || die "$src is not a real directory"
    odd="$(find "$src" ! -type f ! -type d -print | sed -n 1,5p)"
    [[ -z "$odd" ]] || die "$src holds something that is neither a plain file nor a folder: $odd"
    [[ ! -e "$dest" ]] || die "$dest already exists"
    cp -R -- "$src" "$dest"
}

plist_value() {
    plutil -extract "$2" raw -o - -- "$1" || die "$1 has no readable key $2"
}

require_plist_value() {
    local got
    got="$(plist_value "$1" "$2")"
    [[ "$got" == "$3" ]] || die "$1: $2 is '$got', expected '$3'"
}

write_app_plist() {
    local file="$1" version="$2"
    cat > "$file" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleDevelopmentRegion</key>
    <string>en</string>
    <key>CFBundleDisplayName</key>
    <string>ShellCue</string>
    <key>CFBundleExecutable</key>
    <string>shellcue-desktop</string>
    <key>CFBundleIconFile</key>
    <string>icon.icns</string>
    <key>CFBundleIdentifier</key>
    <string>$APP_BUNDLE_ID</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>CFBundleName</key>
    <string>ShellCue</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>$version</string>
    <key>CFBundleVersion</key>
    <string>$version</string>
    <key>LSApplicationCategoryType</key>
    <string>public.app-category.developer-tools</string>
    <key>LSMinimumSystemVersion</key>
    <string>$MIN_MACOS</string>
    <key>LSUIElement</key>
    <true/>
    <key>NSHighResolutionCapable</key>
    <true/>
</dict>
</plist>
EOF
    plutil -lint -- "$file" >/dev/null || die "$file is not a valid property list"
}

assemble_app() {
    local app="$1" version="$2" c="$1/Contents" ime="$1/Contents/Helpers/ShellCueInputMethod.app/Contents"
    local ime_plist="crates/fig_input_method/Info.plist" pair pkg name
    mkdir -p -- "$c/MacOS" "$c/Resources" "$ime/MacOS"

    for pair in $APP_PROGRAMS; do
        pkg="${pair%%:*}"
        name="${pair##*:}"
        require_arm64 "target/release/$pkg"
        cp -- "target/release/$pkg" "$c/MacOS/$name"
    done
    write_app_plist "$c/Info.plist" "$version"
    printf 'APPL????' > "$c/PkgInfo"
    cp -- crates/fig_desktop/icons/icon.icns "$c/Resources/icon.icns"
    [[ -f packages/autocomplete-app/dist/index.html ]] || die "packages/autocomplete-app/dist/index.html is missing; run the 'web' step"
    copy_plain_tree packages/autocomplete-app/dist "$c/Resources/autocomplete"
    [[ -f build/specs/index.json ]] || die "build/specs/index.json is missing; run the 'specs' step"
    copy_plain_tree build/specs "$c/Resources/specs"

    # The input method's build.rs writes its Info.plist into the crate folder on every build of that crate.
    [[ -f "$ime_plist" && ! -L "$ime_plist" ]] || die "$ime_plist is missing; run the 'build' step"
    plutil -lint -- "$ime_plist" >/dev/null || die "$ime_plist is not a valid property list"
    require_plist_value "$ime_plist" CFBundleIdentifier "$IME_BUNDLE_ID"
    require_plist_value "$ime_plist" CFBundleExecutable "$IME_PROGRAM"
    require_plist_value "$ime_plist" CFBundleShortVersionString "$version"
    require_arm64 "target/release/$IME_PROGRAM"
    cp -- "target/release/$IME_PROGRAM" "$ime/MacOS/$IME_PROGRAM"
    cp -- "$ime_plist" "$ime/Info.plist"
    copy_plain_tree crates/fig_input_method/resources "$ime/Resources"

    chmod -R u=rwX,go=rX "$app"
    xattr -cr "$app"
}

# The passwords must reach neither a command line (ps) nor the log: security reads the whole command from stdin, and
# what it prints is logged only after every secret named here is cut out. Its exit status is logged, never relied on.
security_stdin() {
    local label="$1" line="$2" out rc=0 secret
    shift 2
    out="$(printf '%s\n' "$line" | security -i 2>&1 | LC_ALL=C tr -d '\000-\010\013-\037\177')" || rc=$?
    for secret in "$@"; do
        out="${out//$secret/[hidden]}"
    done
    # Second net, should security echo a secret in a form the exact match misses.
    out="$(printf '%s\n' "$out" | sed -E 's/[0-9a-fA-F]{32,}/[hidden]/g')"
    printf '    security %s: exit status %s\n' "$label" "$rc"
    [[ -z "$out" ]] || printf '%s\n' "$out" | sed 's/^/      /'
}

# Paths are written into a line that security splits into words itself.
require_plain_sign_dir() {
    case "$SIGN_DIR" in
        *[!A-Za-z0-9._/-]*) die "the path $SIGN_DIR has characters outside A-Z a-z 0-9 . _ / -, which this script will not pass to security" ;;
    esac
}

# Earlier build steps run other people's code as this user; a planted link must not be followed. Empty mode: any.
require_private() {
    local path="$1" kind="$2" mode="$3" got want
    if [[ "$kind" == "d" ]]; then
        [[ -d "$path" && ! -L "$path" ]] || die "$path is missing or is not a real folder. $SIGN_REPAIR"
    else
        [[ -f "$path" && ! -L "$path" ]] || die "$path is missing or is not a regular file. $SIGN_REPAIR"
    fi
    if [[ -n "$mode" ]]; then
        got="$(stat -f '%u %Lp' "$path")"
        want="$(id -u) $mode"
    else
        got="$(stat -f '%u' "$path")"
        want="$(id -u)"
    fi
    [[ "$got" == "$want" ]] \
        || die "$path has owner and mode '$got', expected '$want'. If you changed it by hand in the VM, put it back (chmod ${mode:-600} $path). Otherwise: $SIGN_REPAIR"
}

# Prints the recorded certificate SHA-1; whatever else the file holds is never printed.
read_sign_ready() {
    local v hex_re='^[0-9A-F]{40}$'
    [[ -f "$SIGN_READY" && ! -L "$SIGN_READY" ]] || return 1
    v="$(< "$SIGN_READY")"
    [[ "$v" =~ $hex_re ]] || return 1
    printf '%s' "$v"
}

cert_sha1() {
    local out hex hex_re='^[0-9A-F]{40}$'
    out="$(/usr/bin/openssl x509 -in "$1" -noout -fingerprint -sha1)" || die "cannot read the certificate $1"
    hex="$(printf '%s' "${out#*=}" | tr -d ':' | tr 'a-f' 'A-F')"
    [[ "$hex" =~ $hex_re ]] || die "cannot read a SHA-1 fingerprint from the certificate $1"
    printf '%s' "$hex"
}

read_sign_password() {
    local pw pw_re='^[0-9a-f]{64}$'
    require_private "$SIGN_PASS_FILE" f 600
    pw="$(< "$SIGN_PASS_FILE")"
    [[ "$pw" =~ $pw_re ]] || die "$SIGN_PASS_FILE does not hold the 64 hex characters sign-setup wrote. $SIGN_REPAIR"
    printf '%s' "$pw"
}

# An unlock does not reliably outlive the SSH session, so every step that signs unlocks first; print_verdict locks
# again. Must not be called inside $( ), or SIGN_UNLOCKED is lost.
unlock_sign_keychain() {
    local pw
    require_plain_sign_dir
    pw="$(read_sign_password)"
    SIGN_UNLOCKED=1
    security_stdin unlock-keychain "unlock-keychain -p $pw $SIGN_KEYCHAIN" "$pw"
}

lock_sign_keychain() {
    security lock-keychain "$SIGN_KEYCHAIN" || die "cannot lock $SIGN_KEYCHAIN"
    SIGN_UNLOCKED=0
}

# security does not say whether an unlock worked, so the key is proven usable by signing a scratch copy of a
# system program (never run).
prove_key_usable() {
    local sha1="$1" dir
    dir="$(mktemp -d "$SIGN_DIR/probe.XXXXXX")"
    cp -- /usr/bin/true "$dir/probe"
    if ! run_limited "$SIGN_LIMIT_SECS" codesign --force --keychain "$SIGN_KEYCHAIN" --sign "$sha1" --timestamp=none "$dir/probe"; then
        rm -rf -- "$dir"
        die "the signing key could not be used right after unlocking. Either the keychain did not unlock with the password in $SIGN_PASS_FILE, or codesign is not allowed to use the key, or macOS refuses the certificate; codesign's own lines and the 'security unlock-keychain' lines are above in the log. Nothing was changed to make macOS trust the certificate. Stop here and have the log reviewed before trying anything else"
    fi
    rm -rf -- "$dir"
    printf '    the key is usable: a scratch copy of /usr/bin/true was signed with the certificate %s\n' "$sha1"
}

require_identity_listed() {
    local sha1="$1" found
    found="$(security find-identity -p codesigning "$SIGN_KEYCHAIN" 2>&1)" || die "security find-identity failed for $SIGN_KEYCHAIN: $found"
    printf '    identities in the signing keychain:\n'
    printf '%s\n' "$found" | sed 's/^/      /'
    case "$found" in
        *"$sha1"*) ;;
        *) die "the signing keychain does not list an identity for the certificate $sha1 (the 'security' lines above in the log say why, if security said anything). Stop here and have the log reviewed" ;;
    esac
}

# No entitlements, no timestamp server, hardened runtime; trust is never granted to the certificate here.
sign_one() {
    local item="$1" id="$2" sha1="$3"
    printf '    signing %s as %s\n' "$item" "$id"
    run_limited "$SIGN_LIMIT_SECS" codesign --force --keychain "$SIGN_KEYCHAIN" --sign "$sha1" --identifier "$id" --options runtime --timestamp=none "$item" \
        || die "codesign could not sign $item with the certificate $sha1 (or did not finish within $SIGN_LIMIT_SECS seconds); codesign's own lines are above in the log. Nothing was changed to make macOS trust the certificate, and nothing half-signed is kept: the next run starts afresh. Stop here and have the log reviewed before trying anything else"
}

check_signed() {
    local item="$1" id="$2" sha1="$3" info ent runtime_re='flags=0x[0-9a-f]+\([^)]*runtime[^)]*\)'
    codesign --verify --strict -R "=identifier \"$id\" and certificate leaf = H\"$sha1\"" "$item" \
        || die "$item does not verify as '$id' signed by the certificate $sha1. Stop here and have the log reviewed"
    info="$(codesign -d --verbose=2 -r- "$item" 2>&1)" || die "codesign cannot describe $item: $info"
    printf '%s\n' "$info" | sed 's/^/      /'
    [[ "$info" =~ $runtime_re ]] || die "$item is not signed with the hardened runtime (the flags line above lacks 'runtime'). Stop here and have the log reviewed"
    ent="$(codesign -d --entitlements - "$item" 2>/dev/null)" || die "codesign cannot read the entitlements of $item"
    [[ -z "$ent" ]] || die "$item carries entitlements, and none are meant to be applied: $ent"
}

write_cert_request() {
    cat > "$1" <<EOF
[req]
distinguished_name = dn
prompt = no
x509_extensions = ext
[dn]
CN = $SIGN_NAME
[ext]
basicConstraints = critical,CA:FALSE
keyUsage = critical,digitalSignature
extendedKeyUsage = critical,codeSigning
subjectKeyIdentifier = hash
EOF
}

# create-keychain adds the new keychain to the user's search list; nothing else in the VM is meant to find it there.
restore_keychain_search() {
    local before="$1" before_default="$2" line now paths=()
    while IFS= read -r line; do
        line="${line#"${line%%[![:space:]]*}"}"
        line="${line#\"}"
        line="${line%\"}"
        [[ -z "$line" || "$line" == "$SIGN_KEYCHAIN" ]] || paths+=("$line")
    done <<< "$before"
    security list-keychains -d user -s ${paths[@]+"${paths[@]}"} >/dev/null
    now="$(security list-keychains -d user)"
    printf '    keychain search list of this user, before and after:\n'
    printf '%s\n--\n%s\n' "$before" "$now" | sed 's/^/      /'
    case "$now" in
        *"$SIGN_KEYCHAIN"*) die "the signing keychain is still in this user's keychain search list after putting the list back. Stop here and have the log reviewed" ;;
    esac
    now="$(security default-keychain -d user 2>/dev/null || true)"
    [[ "$now" == "$before_default" ]] \
        || die "making the signing keychain changed this user's default keychain from '$before_default' to '$now'. Stop here and have the log reviewed"
}

step_sign_setup() {
    require_no_args "$@"
    check_command_line_tools
    require_plain_sign_dir
    local pw p12pw sha1 before_list before_default
    if [[ -e "$SIGN_READY" || -L "$SIGN_READY" ]]; then
        die "this VM already has a signing identity (certificate SHA-1 $(read_sign_ready || printf 'unreadable')), and this step never replaces one: a new certificate makes macOS forget every permission granted to ShellCue. To replace it on purpose, in the VM's own Terminal run: rm -rf $SIGN_DIR   and then, on the Mac: scripts/vm/mac-host.sh step sign-setup   That destroys the old key, which has no copy, and bundle cannot sign until sign-setup has succeeded again"
    fi
    if [[ -e "$SIGN_DIR" || -L "$SIGN_DIR" ]]; then
        printf -- '--> removing what a sign-setup that did not finish left behind: %s\n' "$SIGN_DIR"
        rm -rf -- "$SIGN_DIR"
    fi
    printf -- '--> if this step fails, nothing usable is left and it is safe to run again: scripts/vm/mac-host.sh step sign-setup\n'
    umask 077
    mkdir -- "$SIGN_DIR"
    mkdir -- "$SIGN_WORK"
    SIGN_WORK_OWNED=1
    printf '    openssl: %s\n' "$(/usr/bin/openssl version)"

    printf -- '--> making the key and the self-signed certificate "%s" (RSA 2048, %s days)\n' "$SIGN_NAME" "$SIGN_DAYS"
    write_cert_request "$SIGN_WORK/req.cnf"
    /usr/bin/openssl req -x509 -newkey rsa:2048 -nodes -sha256 -days "$SIGN_DAYS" -config "$SIGN_WORK/req.cnf" \
        -keyout "$SIGN_WORK/key.pem" -out "$SIGN_CERT"
    sha1="$(cert_sha1 "$SIGN_CERT")"
    /usr/bin/openssl rand -hex 32 > "$SIGN_WORK/p12-password"
    /usr/bin/openssl pkcs12 -export -inkey "$SIGN_WORK/key.pem" -in "$SIGN_CERT" -name "$SIGN_NAME" \
        -out "$SIGN_WORK/identity.p12" -passout "file:$SIGN_WORK/p12-password"
    p12pw="$(< "$SIGN_WORK/p12-password")"

    printf -- '--> making the signing keychain %s\n' "$SIGN_KEYCHAIN"
    /usr/bin/openssl rand -hex 32 > "$SIGN_PASS_FILE"
    pw="$(read_sign_password)"
    before_list="$(security list-keychains -d user)"
    before_default="$(security default-keychain -d user 2>/dev/null || true)"
    security_stdin create-keychain "create-keychain -p $pw $SIGN_KEYCHAIN" "$pw"
    [[ -f "$SIGN_KEYCHAIN" ]] || die "security did not create $SIGN_KEYCHAIN; its own lines are above in the log"
    restore_keychain_search "$before_list" "$before_default"
    # No time limit and no lock on sleep; the keychain is locked by the step that unlocked it.
    security set-keychain-settings "$SIGN_KEYCHAIN"
    unlock_sign_keychain
    # -x: not exportable again. -T and the partition list let codesign use the key with no dialog, which SSH cannot answer.
    security_stdin import "import $SIGN_WORK/identity.p12 -k $SIGN_KEYCHAIN -f pkcs12 -P $p12pw -x -T /usr/bin/codesign" "$p12pw" "$pw"
    security_stdin set-key-partition-list "set-key-partition-list -S apple-tool:,apple: -s -k $pw $SIGN_KEYCHAIN" "$pw"
    rm -f -- "$SIGN_WORK/key.pem" "$SIGN_WORK/identity.p12" "$SIGN_WORK/p12-password"
    printf '    the key file and its packaged copy are deleted; the key now exists only in the keychain\n'
    printf '    (limit: on this kind of disk a deleted file is unlinked, not overwritten)\n'
    require_identity_listed "$sha1"
    prove_key_usable "$sha1"

    printf -- '--> trial: signing and running a throwaway program with the new identity\n'
    printf 'int main(void) { return 0; }\n' > "$SIGN_WORK/trial.c"
    xcrun clang -o "$SIGN_WORK/trial" "$SIGN_WORK/trial.c"
    sign_one "$SIGN_WORK/trial" "$APP_BUNDLE_ID.trial" "$sha1"
    check_signed "$SIGN_WORK/trial" "$APP_BUNDLE_ID.trial" "$sha1"
    run_limited 60 "$SIGN_WORK/trial" \
        || die "the signed trial program did not start (exit status $?): a program signed this way (with Apple's 'hardened runtime' protection) does not run. No identity was recorded. Stop here and have the log reviewed; do not run bundle"
    printf '    the signed trial program started and exited 0\n'

    # What bundle does in a later session: start locked, unlock from the password file, use the key.
    lock_sign_keychain
    unlock_sign_keychain
    prove_key_usable "$sha1"
    lock_sign_keychain
    rm -rf -- "$SIGN_WORK"
    SIGN_WORK_OWNED=0

    printf '%s\n' "$sha1" > "$SIGN_READY"
    printf '    certificate:\n'
    /usr/bin/openssl x509 -in "$SIGN_CERT" -noout -subject -dates -fingerprint -sha256 | sed 's/^/      /'
    printf -- '--> signing identity ready in %s: a self-signed certificate, fingerprint (SHA-1) %s\n' "$SIGN_DIR" "$sha1"
    printf -- '--> every bundle run prints that fingerprint; it must never change between runs\n'
    printf -- '--> the key stays in this VM and has no copy: if the VM is lost, a new certificate is made and ShellCue must be granted its permissions again\n'
    printf -- '--> next, on the Mac (after specs and build): scripts/vm/mac-host.sh step bundle\n'
}

require_sign_identity() {
    local sha1 want
    [[ -e "$SIGN_DIR" || -L "$SIGN_DIR" ]] \
        || die "this VM has no signing identity yet. On the Mac run: scripts/vm/mac-host.sh step sign-setup   and then: scripts/vm/mac-host.sh step bundle"
    require_private "$SIGN_DIR" d 700
    want="$(read_sign_ready)" \
        || die "$SIGN_READY is missing or does not hold a certificate fingerprint. If sign-setup has never succeeded in this VM, run on the Mac: scripts/vm/mac-host.sh step sign-setup   and then: scripts/vm/mac-host.sh step bundle   Otherwise the signing folder was changed after it was made. $SIGN_REPAIR"
    require_private "$SIGN_KEYCHAIN" f ""
    require_private "$SIGN_CERT" f 600
    sha1="$(cert_sha1 "$SIGN_CERT")"
    [[ "$sha1" == "$want" ]] || die "$SIGN_CERT has SHA-1 $sha1 but sign-setup recorded $want; the signing folder was changed after it was made. $SIGN_REPAIR"
    printf '%s' "$sha1"
}

sign_app() {
    local app="$1" sha1="$2" version="$3" c="$1/Contents" ime_app="$1/Contents/Helpers/ShellCueInputMethod.app" said
    unlock_sign_keychain
    require_identity_listed "$sha1"
    prove_key_usable "$sha1"
    # Inside out: nested code is signed before the bundle whose seal covers it. Every program in APP_PROGRAMS but the
    # main one needs its own line here and below.
    sign_one "$ime_app" "$IME_BUNDLE_ID" "$sha1"
    sign_one "$c/MacOS/shellcue" "$CLI_SIGN_ID" "$sha1"
    sign_one "$c/MacOS/shellcue-term" "$TERM_SIGN_ID" "$sha1"
    sign_one "$app" "$APP_BUNDLE_ID" "$sha1"
    # Verifying needs no key.
    lock_sign_keychain

    printf '    verifying the whole app:\n'
    codesign --verify --strict --deep --verbose=2 "$app" 2>&1 | sed 's/^/      /' \
        || die "$app does not verify after signing. Stop here and have the log reviewed"
    check_signed "$ime_app" "$IME_BUNDLE_ID" "$sha1"
    check_signed "$c/MacOS/shellcue" "$CLI_SIGN_ID" "$sha1"
    check_signed "$c/MacOS/shellcue-term" "$TERM_SIGN_ID" "$sha1"
    check_signed "$app" "$APP_BUNDLE_ID" "$sha1"
    # Proves a program signed this way still starts; --version prints and exits before the program does anything else.
    said="$(cd / && run_limited 60 env -i HOME="$HOME" PATH="$PATH" "$c/MacOS/shellcue" --version)" \
        || die "the signed command-line program did not start, or did not exit within 60 seconds. Stop here and have the log reviewed"
    printf '    the signed command-line program starts and says: %s\n' "${said:0:200}"
    case "$said" in
        *"$version"*) ;;
        *) die "the signed command-line program did not report version $version" ;;
    esac
}

make_dmg() {
    local app="$1" dmg="$2" tree="$3" root="$OUT/dmg-root"
    mkdir -- "$root"
    cp -R -- "$app" "$root/ShellCue.app"
    # The copy, not the app that was checked, is what goes into the image.
    codesign --verify --strict --deep "$root/ShellCue.app" || die "the copy of the app made for the disk image does not verify"
    [[ "$(tree_sha256 "$root/ShellCue.app")" == "$tree" ]] || die "the copy of the app made for the disk image differs from the signed app"
    ln -s /Applications "$root/Applications"
    hdiutil create -volname ShellCue -srcfolder "$root" -fs HFS+ -format ULFO -ov "$dmg"
    hdiutil verify "$dmg"
    rm -rf -- "$root"
}

step_bundle() {
    require_no_args "$@"
    check_command_line_tools
    enter_src
    require_clean_tree
    require_stamps web specs build
    local version app="$OUT/ShellCue.app" dmg="$OUT/ShellCue.dmg" f sum tree sha1
    version="$(workspace_version)"
    sha1="$(require_sign_identity)"
    printf -- '--> version %s, bundle id %s, signing certificate SHA-1 %s\n' "$version" "$APP_BUNDLE_ID" "$sha1"

    [[ "$OUT" == "$HOME/shellcue/build/macos" ]] || die "refusing to clear an unexpected output folder: $OUT"
    rm -rf -- "$OUT"
    mkdir -p -- "$OUT"
    assemble_app "$app" "$version"

    printf '    Info.plist:\n'
    plutil -p -- "$app/Contents/Info.plist" | sed 's/^/      /'
    printf -- '--> assembled %s; signing it\n' "$app"
    sign_app "$app" "$sha1" "$version"

    # Signing rewrites the programs and adds the seal, so these figures describe the signed app.
    printf '    programs:\n'
    for f in MacOS/shellcue-desktop MacOS/shellcue MacOS/shellcue-term "Helpers/ShellCueInputMethod.app/Contents/MacOS/$IME_PROGRAM"; do
        printf '      %s  Contents/%s\n' "$(sha256_of "$app/Contents/$f")" "$f"
    done
    printf '    files: %s, size: %s\n' "$(find "$app" -type f | wc -l | tr -d ' ')" "$(du -sh -- "$app" | cut -f1)"
    tree="$(tree_sha256 "$app")"
    [[ -n "$tree" ]] || die "cannot hash the files under $app"
    printf '    app tree sha256: %s\n' "$tree"
    printf -- '--> signed %s with the self-signed certificate %s (not notarised; another Mac will not trust it by itself)\n' "$app" "$sha1"

    printf -- '--> making the disk image (a minute or two)\n'
    make_dmg "$app" "$dmg" "$tree"
    sum="$(sha256_of "$dmg")"
    printf '%s  ShellCue.dmg\n' "$sum" > "$dmg.sha256"
    printf '    disk image size: %s\n' "$(du -h -- "$dmg" | cut -f1)"
    printf -- '--> disk image %s sha256 %s\n' "$dmg" "$sum"
    printf -- '--> the app and the disk image stay in the VM; nothing is copied to the Mac\n'
}

step_info() {
    require_no_args "$@"
    printf '    kernel: %s\n' "$(uname -r)"
    local dir
    dir="$(xcode-select -p 2>/dev/null || true)"
    printf '    developer dir: %s\n' "${dir:-none (run xcode-select --install in the VM)}"
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
    printf '    signing identity: %s\n' "$(read_sign_ready || printf 'none, or not readable (made by: scripts/vm/mac-host.sh step sign-setup)')"
    if [[ -n "$dir" && -d "$SRC/.git" ]]; then
        printf '    checkout: %s\n' "$(git -C "$SRC" log -1 --format='%H %s')"
        printf '    git status --short:\n'
        git -C "$SRC" status --short | sed 's/^/      /'
    else
        printf '    checkout: none readable at %s\n' "$SRC"
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
    check_os
    check_no_shared_folders
    if [[ "$step" == "info" ]]; then
        printf '    step lock: %s\n' "$(cat -- "$LOCK_DIR/pid" 2>/dev/null || printf 'none')"
    else
        take_lock
    fi
    case "$step" in
        tools) step_tools "$@" ;;
        clone) step_clone "$@" ;;
        fetch) step_fetch "$@" ;;
        web) step_web "$@" ;;
        rust-check) step_rust_check "$@" ;;
        rust-test) step_rust_test "$@" ;;
        specs) step_specs "$@" ;;
        build) step_build "$@" ;;
        sign-setup) step_sign_setup "$@" ;;
        bundle) step_bundle "$@" ;;
        info) step_info "$@" ;;
        *) die "step '$step' is listed in STEPS but has no handler" ;;
    esac
}

main "$@"
