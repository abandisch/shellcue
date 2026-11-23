#!/bin/bash
# Runs INSIDE the throwaway macOS build VM as its login user; mac-host.sh copies it in. Must stay compatible with macOS /bin/bash 3.2.

set -euo pipefail

# Files left in the home folder by an earlier build must not be able to shadow the system tools.
export PATH="/usr/bin:/bin:/usr/sbin:/sbin"

# Every pnpm install passes an explicit lockfile flag because CI=true changes pnpm's default.
export CI=true NO_COLOR=1 FORCE_COLOR=0 TERM=dumb TURBO_TELEMETRY_DISABLED=1 DO_NOT_TRACK=1

REPO_URL="https://github.com/abandisch/shellcue.git"
TOOLS="$HOME/tools"
DL="$TOOLS/dl"
ENV_FILE="$TOOLS/env.sh"
SRC="$HOME/shellcue"
LOCK_DIR="$HOME/.shellcue-mac-guest.lock"
TEST_LIMIT_SECS=5400

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

STEPS="tools clone fetch web rust-check rust-test info"

usage() {
    cat <<'EOF'
Usage: mac-guest.sh <step> [args]

Runs inside the ShellCue macOS build VM (Apple Silicon). Steps:
  tools                    pinned, sha256-verified node, pnpm, protoc and rustup, then the pinned Rust toolchain through rustup;
                           writes ~/tools/env.sh. Needs Apple's Command Line Tools already installed (xcode-select --install).
  clone <branch> <sha>     clone/fetch the public repo into ~/shellcue and check out <sha> detached
  fetch                    cargo fetch --locked --target <host triple> and pnpm install --frozen-lockfile --ignore-scripts
  web                      pnpm run build and pnpm run test:ci (both always run)
  rust-check               cargo check --locked --workspace --all-targets --exclude fig_desktop-fuzz
  rust-test                cargo test --locked --no-fail-fast --workspace --lib --bins --test '*' --exclude fig_desktop-fuzz,
                           with SSH_CLIENT, SSH_CONNECTION and SSH_TTY unset; stopped after 90 minutes
  info                     OS, arch, Command Line Tools, tool versions, disk free, checked-out commit, git status

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
    [[ -d "$SRC/.git" ]] || die "no checkout at $SRC; run the 'clone' step first"
    cd "$SRC"
    printf '    checkout: %s @ %s\n' "$SRC" "$(git rev-parse HEAD)"
    check_pins
    local f
    for f in Cargo.lock pnpm-lock.yaml; do
        git diff --quiet HEAD -- "$f" \
            || die "$f differs from the committed version; lockfiles are refreshed only in the Linux VM. In the VM's own Terminal run: git -C ~/shellcue checkout HEAD -- $f"
    done
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
    run_part build pnpm run build
    run_part test:ci pnpm run test:ci
    end_parts
}

step_rust_check() {
    require_no_args "$@"
    load_env
    enter_src
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
    run_limited "$TEST_LIMIT_SECS" env -u SSH_CLIENT -u SSH_CONNECTION -u SSH_TTY \
        cargo test --locked --no-fail-fast --workspace --lib --bins --test '*' --exclude fig_desktop-fuzz || rc=$?
    if [[ $rc -eq 142 ]]; then
        printf '    the test run exceeded %s minutes; cargo and the processes it started were stopped (exit status %s)\n' "$((TEST_LIMIT_SECS / 60))" "$rc"
    fi
    [[ $rc -eq 0 ]] || exit "$rc"
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
        info) step_info "$@" ;;
        *) die "step '$step' is listed in STEPS but has no handler" ;;
    esac
}

main "$@"
