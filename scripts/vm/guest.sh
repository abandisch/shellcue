#!/usr/bin/env bash
# Runs INSIDE the throwaway Ubuntu 24.04 build VM as user `ubuntu`; host.sh copies it in.

set -euo pipefail

# Every pnpm install passes an explicit lockfile flag because CI=true changes pnpm's default.
export CI=true NO_COLOR=1 FORCE_COLOR=0 TERM=dumb TURBO_TELEMETRY_DISABLED=1 DO_NOT_TRACK=1

REPO_URL="https://github.com/abandisch/shellcue.git"
TOOLS="$HOME/tools"
DL="$TOOLS/dl"
ENV_FILE="$TOOLS/env.sh"
SRC="$HOME/shellcue"

# Change together with .mise.toml, package.json (packageManager) and rust-toolchain.toml; enter_src enforces it.
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
    libfuse2t64 git ca-certificates xz-utils unzip gpgv zsh fish shellcheck
)

STEPS="tools clone lock fetch web rust-check rust-test info"

usage() {
    cat <<'EOF'
Usage: guest.sh <step> [args]

Runs inside the ShellCue build VM. Steps:
  tools                    apt packages + pinned, sha256-verified node, pnpm, protoc, rustup/rust; writes ~/tools/env.sh
  clone <branch> <sha>     clone/fetch the public repo into ~/shellcue and check out <sha> detached
  lock                     refresh Cargo.lock (cargo update --workspace) and pnpm-lock.yaml (lockfile-only); print what changed
  fetch                    cargo fetch --locked and pnpm install --frozen-lockfile --ignore-scripts
  web                      pnpm run build, then pnpm run test:ci
  rust-check               cargo check --locked --workspace --all-targets --exclude fig_desktop-fuzz
  rust-test                cargo test --locked --no-fail-fast --workspace --lib --bins --test '*' --exclude fig_desktop-fuzz
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

# Must stay the last thing printed, whatever ended the run.
print_verdict() {
    local rc=$? secs=$((SECONDS - STEP_START))
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
    require_pin .mise.toml 'rust = ' "\"$RUST_TOOLCHAIN\""
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

host_triple() {
    local out triple
    out="$(rustc -vV)"
    triple="$(printf '%s\n' "$out" | sed -n 's/^host: //p')"
    [[ -n "$triple" ]] || die "could not read host triple from 'rustc -vV'"
    printf '%s' "$triple"
}

step_tools() {
    require_no_args "$@"
    check_os
    detect_arch
    printf '    arch: %s\n' "$ARCH"

    sudo apt-get update
    sudo DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends "${APT_PACKAGES[@]}"

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
    local rc=0
    timeout --kill-after=60 5400 \
        cargo test --locked --no-fail-fast --workspace --lib --bins --test '*' --exclude fig_desktop-fuzz || rc=$?
    if [[ $rc -eq 124 || $rc -eq 137 ]]; then
        printf '    the test run exceeded 90 minutes and was stopped (timeout exit status %s)\n' "$rc"
    fi
    [[ $rc -eq 0 ]] || exit "$rc"
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
    case "$step" in
        tools) step_tools "$@" ;;
        clone) step_clone "$@" ;;
        lock) step_lock "$@" ;;
        fetch) step_fetch "$@" ;;
        web) step_web "$@" ;;
        rust-check) step_rust_check "$@" ;;
        rust-test) step_rust_test "$@" ;;
        info) step_info "$@" ;;
        *) die "step '$step' is listed in STEPS but has no handler" ;;
    esac
}

main "$@"
