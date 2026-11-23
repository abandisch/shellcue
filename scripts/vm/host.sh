#!/bin/bash
# Runs on the owner's Mac and drives the throwaway build VM; must stay compatible with macOS /bin/bash 3.2.
# Never runs anything the VM produced, never mounts host folders, writes only to ../vm-logs and ../vm-out.

set -euo pipefail

VM_NAME="shellcue-build"
VM_IMAGE="24.04"
VM_CPUS="${SHELLCUE_VM_CPUS:-4}"
VM_MEMORY="${SHELLCUE_VM_MEMORY:-12G}"
VM_DISK="${SHELLCUE_VM_DISK:-60G}"
GUEST_PATH="/home/ubuntu/guest.sh"
GUEST_SRC="/home/ubuntu/shellcue"
GUEST_ICONS="/home/ubuntu/shellcue-icons"
GUEST_STEPS="tools clone lock fetch web specs icons rust-check rust-test info"
PULL_MAX_BYTES=52428800
ICON_MAX_BYTES=4194304
ICON_MAX_ENTRIES=64

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
OUT_PARENT="$(cd "$REPO_ROOT/.." && pwd)"
LOG_DIR="$OUT_PARENT/vm-logs"
OUT_DIR="$OUT_PARENT/vm-out"

VM_INFO=""

usage() {
    cat <<EOF
Usage: host.sh <command>

Drives the Multipass VM "$VM_NAME" (Ubuntu $VM_IMAGE). Logs go to $LOG_DIR,
fetched lockfiles and icons to $OUT_DIR (both outside the repo).

Commands:
  create                  launch the VM ($VM_CPUS CPUs, $VM_MEMORY memory, $VM_DISK disk; no mounts, no cloud-init)
  push                    copy scripts/vm/guest.sh from this working tree into the VM, logged as "push"
  step <name> [args...]   push, then run guest.sh <name> in the VM, logged; steps: $GUEST_STEPS
  clone                   step clone <current branch> <HEAD sha>; refuses if dirty or not pushed to origin
  pull-lockfiles          copy the VM's Cargo.lock and pnpm-lock.yaml into a fresh vm-out/<timestamp>/
  pull-icons              copy the icons that "step icons" generated into a fresh vm-out/<timestamp>/icons/, taking only
                          the files named in scripts/icons/manifest.txt and checking each one's type, size and dimensions
  exec -- <command...>    run one diagnostic command in the VM, logged as "exec"
  info                    multipass info $VM_NAME
  stop                    multipass stop $VM_NAME, logged as "stop"
  destroy                 multipass delete $VM_NAME (requires SHELLCUE_VM_CONFIRM=destroy)

Environment overrides (create only): SHELLCUE_VM_CPUS (digits), SHELLCUE_VM_MEMORY, SHELLCUE_VM_DISK (digits then G).

Typical first session:
  scripts/vm/host.sh create
  scripts/vm/host.sh step tools
  scripts/vm/host.sh clone
  scripts/vm/host.sh step lock
  scripts/vm/host.sh pull-lockfiles
  (review the pulled lockfiles; if accepted, copy them into the repo, commit and push)
  scripts/vm/host.sh clone
  (clone discards the VM's own lockfile edits only when it checks out a new commit, so the
   refreshed lockfiles must be committed and pushed first; if they are not, the steps below
   keep using the VM's uncommitted lockfiles)
  scripts/vm/host.sh step fetch
  scripts/vm/host.sh step web
  scripts/vm/host.sh step specs
  scripts/vm/host.sh step rust-check
  scripts/vm/host.sh step rust-test
  scripts/vm/host.sh stop

Regenerating the icons after artwork/*.svg changes (commit and push the change first):
  scripts/vm/host.sh step tools
  scripts/vm/host.sh clone
  scripts/vm/host.sh step icons
  scripts/vm/host.sh pull-icons
  (review the pulled files; if accepted, copy them into the repo, commit and push)
  scripts/vm/host.sh clone
  scripts/vm/host.sh step icons check
EOF
}

die() {
    printf 'host.sh: ERROR: %s\n' "$*" >&2
    exit 1
}

# VM output is untrusted: drop control bytes (ESC, CR, ...) but keep tab, newline and bytes >= 0x80.
sanitize() {
    LC_ALL=C tr -u -d '\000-\010\013-\037\177'
}

guard_vm_name() {
    [ "$VM_NAME" != "primary" ] || die "refusing to operate on the Multipass 'primary' instance"
}

require_multipass() {
    guard_vm_name
    command -v multipass >/dev/null 2>&1 || die "multipass not found on PATH"
}

validate_resources() {
    case "$VM_CPUS" in
        ''|*[!0-9]*) die "SHELLCUE_VM_CPUS must be digits, got '$VM_CPUS'" ;;
    esac
    case "$VM_MEMORY" in
        [0-9]*G) case "${VM_MEMORY%G}" in *[!0-9]*) die "SHELLCUE_VM_MEMORY must be digits followed by G, got '$VM_MEMORY'" ;; esac ;;
        *) die "SHELLCUE_VM_MEMORY must be digits followed by G, got '$VM_MEMORY'" ;;
    esac
    case "$VM_DISK" in
        [0-9]*G) case "${VM_DISK%G}" in *[!0-9]*) die "SHELLCUE_VM_DISK must be digits followed by G, got '$VM_DISK'" ;; esac ;;
        *) die "SHELLCUE_VM_DISK must be digits followed by G, got '$VM_DISK'" ;;
    esac
}

# Returns 0 if the VM exists (output in VM_INFO), 1 if multipass says it does not; dies if deleted or on any other failure.
vm_exists() {
    local out rc line val
    set +e
    out="$(multipass info "$VM_NAME" 2>&1)"
    rc=$?
    set -e
    while IFS= read -r line; do
        case "$line" in
            State:*)
                val="${line#State:}"
                val="${val#"${val%%[![:space:]]*}"}"
                val="${val%"${val##*[![:space:]]}"}"
                [ "$val" != "Deleted" ] || die "the name $VM_NAME is still held by a deleted instance; run 'multipass recover $VM_NAME' to bring it back, or 'multipass purge' to remove ALL deleted instances"
                ;;
        esac
    done <<< "$out"
    if [ "$rc" -eq 0 ]; then
        VM_INFO="$out"
        return 0
    fi
    case "$out" in
        *"does not exist"*) return 1 ;;
    esac
    printf '%s\n' "$out" | sanitize >&2
    die "multipass info $VM_NAME failed (exit $rc) for a reason other than a missing instance"
}

# The Mounts: value and its indented continuation lines must all be empty or "--"; an unrecognised format fails closed.
check_no_mounts() {
    local line val found=0 in_mounts=0 mounts=""
    while IFS= read -r line; do
        if [ "$in_mounts" -eq 1 ]; then
            case "$line" in
                ''|[[:space:]]*)
                    val="${line#"${line%%[![:space:]]*}"}"
                    [ -z "$val" ] || mounts="$mounts $val"
                    continue
                    ;;
            esac
            in_mounts=0
        fi
        case "$line" in
            Mounts:*)
                found=1
                in_mounts=1
                val="${line#Mounts:}"
                val="${val#"${val%%[![:space:]]*}"}"
                val="${val%"${val##*[![:space:]]}"}"
                if [ -n "$val" ] && [ "$val" != "--" ]; then
                    mounts="$mounts $val"
                fi
                ;;
        esac
    done <<< "$VM_INFO"
    if [ -z "$VM_INFO" ] || [ "$found" -eq 0 ]; then
        printf 'host.sh: ERROR: multipass info output format was not recognised (no Mounts: line); output was:\n' >&2
        printf '%s\n' "$VM_INFO" | sanitize >&2
        exit 1
    fi
    if [ -n "$mounts" ]; then
        printf '\n!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!\n' >&2
        printf '!!! WARNING: %s HAS A HOST MOUNT:%s\n' "$VM_NAME" "$mounts" | sanitize >&2
        printf '!!! The build VM must have no host folders mounted. Stop and remove it.\n' >&2
        printf '!!! Remedy: multipass umount %s, or destroy and recreate the VM.\n' "$VM_NAME" >&2
        printf '!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!\n\n' >&2
        exit 1
    fi
    printf 'mounts: none (Mounts line shows no mount)\n'
}

utc_stamp() {
    date -u +%Y%m%dT%H%M%SZ
}

run_logged() {
    local label="$1" ts log rc tr_rc tee_rc statuses cmdline
    shift
    mkdir -p "$LOG_DIR"
    ts="$(utc_stamp)"
    log="$LOG_DIR/$ts-$label.log"
    [ ! -e "$log" ] || die "log file already exists: $log"
    printf '==> host.sh %s at %s (host repo HEAD %s)\n' "$label" "$ts" \
        "$(git -C "$REPO_ROOT" rev-parse HEAD 2>/dev/null || printf 'unknown')" | tee "$log"
    cmdline="$(printf '%q ' "$@")"
    printf 'command: %s\n' "${cmdline% }" | tee -a "$log"
    set +e
    ( set -e; "$@" ) 3>>"$log" 2>&1 | sanitize | tee -a "$log"
    statuses=("${PIPESTATUS[@]}")
    set -e
    rc="${statuses[0]}"
    tr_rc="${statuses[1]}"
    tee_rc="${statuses[2]}"
    if [ "$tr_rc" -ne 0 ] || [ "$tee_rc" -ne 0 ]; then
        printf 'host.sh: WARNING: sanitize exited %s, tee exited %s; the output and log may be incomplete\n' "$tr_rc" "$tee_rc" >&2
        if [ "$rc" -eq 0 ]; then
            if [ "$tr_rc" -ne 0 ]; then rc="$tr_rc"; else rc="$tee_rc"; fi
        fi
    fi
    printf 'log: %s\nexit status: %s\n' "$log" "$rc" | tee -a "$log"
    return "$rc"
}

# Multipass spinners survive sanitize as debris, so output goes to fd 3 and reaches stdout only on failure; fd 3 must be open (run_logged points it at the log).
run_quiet() {
    local msg="$1" out rc=0
    shift
    printf '%s\n' "$msg"
    out="$("$@" 2>&1 3>&-)" || rc=$?
    if [ "$rc" -ne 0 ]; then
        [ -z "$out" ] || printf '%s\n' "$out" | sanitize
        return "$rc"
    fi
    [ -z "$out" ] || printf '%s\n' "$out" | sanitize >&3
}

create_body() {
    if vm_exists; then
        printf 'VM %s already exists; doing nothing.\n' "$VM_NAME"
        printf '%s\n' "$VM_INFO" | sanitize
        check_no_mounts
        return 0
    fi
    run_quiet "creating VM $VM_NAME (about two minutes; longer if the image must be downloaded)..." multipass launch "$VM_IMAGE" --name "$VM_NAME" --cpus "$VM_CPUS" --memory "$VM_MEMORY" --disk "$VM_DISK"
    vm_exists || die "multipass launch returned but $VM_NAME does not exist"
    printf '%s\n' "$VM_INFO" | sanitize
    check_no_mounts
}

cmd_create() {
    validate_resources
    require_multipass
    run_logged create create_body
}

cmd_push() {
    require_multipass
    local src="$SCRIPT_DIR/guest.sh"
    [ -f "$src" ] || die "guest script not found: $src"
    vm_exists || die "VM $VM_NAME does not exist; run: host.sh create"
    check_no_mounts
    multipass transfer "$src" "$VM_NAME:$GUEST_PATH"
    multipass exec "$VM_NAME" -- chmod +x "$GUEST_PATH"
    printf 'pushed %s -> %s:%s\n' "$src" "$VM_NAME" "$GUEST_PATH"
}

step_body() {
    cmd_push
    multipass exec "$VM_NAME" -- "$GUEST_PATH" "$@"
}

cmd_step() {
    [ $# -ge 1 ] || die "usage: host.sh step <name> [args...]; steps: $GUEST_STEPS"
    local name="$1"
    shift
    case " $GUEST_STEPS " in
        *" $name "*) ;;
        *) die "unknown step '$name'; valid steps: $GUEST_STEPS" ;;
    esac
    require_multipass
    run_logged "$name" step_body "$name" "$@"
}

cmd_clone() {
    command -v git >/dev/null 2>&1 || die "git not found on PATH"
    local branch sha dirty remote branch_re='^[A-Za-z0-9._/-]+$'
    branch="$(git -C "$REPO_ROOT" symbolic-ref --short HEAD)" || die "local repo is on a detached HEAD; check out a branch"
    sha="$(git -C "$REPO_ROOT" rev-parse HEAD)"
    [[ "$branch" =~ $branch_re ]] || die "branch name '$branch' contains characters guest.sh will reject"
    dirty="$(git -C "$REPO_ROOT" status --porcelain --untracked-files=no)"
    if [ -n "$dirty" ]; then
        printf '%s\n' "$dirty" >&2
        die "uncommitted changes to tracked files; commit or stash them first"
    fi
    remote="$(git -C "$REPO_ROOT" rev-parse --verify --quiet "refs/remotes/origin/$branch")" \
        || die "no remote-tracking ref origin/$branch; the VM clones from GitHub, so push the branch first"
    if [ "$remote" != "$sha" ]; then
        die "local $branch is $sha but origin/$branch is $remote. The VM clones from GitHub, so push this commit first (this check reads the local remote-tracking ref and does not contact the network; run git fetch if it is stale)"
    fi
    printf 'cloning %s @ %s in the VM\n' "$branch" "$sha"
    cmd_step clone "$branch" "$sha"
}

# Pulled files must be under the size cap and contain only tab, newline, printable ASCII or bytes >= 0x80.
check_pulled_file() {
    local f="$1" size bad
    size="$(wc -c < "$f" | tr -d ' ')"
    if [ "$size" -gt "$PULL_MAX_BYTES" ]; then
        mv -f -- "$f" "$f.rejected"
        die "rejected $f: $size bytes exceeds the $PULL_MAX_BYTES-byte (50 MB) cap; renamed to $f.rejected"
    fi
    bad="$(LC_ALL=C tr -d '\011\012\040-\176\200-\377' < "$f" | wc -c | tr -d ' ')"
    if [ "$bad" -ne 0 ]; then
        mv -f -- "$f" "$f.rejected"
        die "rejected $f: it contains $bad control byte(s) other than tab and newline; renamed to $f.rejected"
    fi
}

pull_body() {
    vm_exists || die "VM $VM_NAME does not exist"
    local dest
    mkdir -p "$OUT_DIR"
    dest="$OUT_DIR/$(utc_stamp)"
    mkdir "$dest"
    multipass transfer "$VM_NAME:$GUEST_SRC/Cargo.lock" "$dest/Cargo.lock"
    check_pulled_file "$dest/Cargo.lock"
    multipass transfer "$VM_NAME:$GUEST_SRC/pnpm-lock.yaml" "$dest/pnpm-lock.yaml"
    check_pulled_file "$dest/pnpm-lock.yaml"
    printf '%s\n%s\n' "$dest/Cargo.lock" "$dest/pnpm-lock.yaml"
    printf 'Not copied into the repo; review before using.\n'
    printf '\nsha256 of the files as received:\n'
    shasum -a 256 "$dest/Cargo.lock" "$dest/pnpm-lock.yaml"
    printf 'Compare these with the sha256sum lines at the end of the latest lock log in %s.\n' "$LOG_DIR"
    printf '\nNext steps:\n'
    printf "  1. Review both files against the repo's Cargo.lock and pnpm-lock.yaml.\n"
    printf '  2. If you accept them, copy them into the repo, commit, and push.\n'
    printf "  3. Run host.sh clone again. It discards the VM's own lockfile edits only when it checks out a new commit,\n"
    printf "     so step 2 must come first; if it does not, later steps keep using the VM's uncommitted lockfiles.\n"
    printf '  4. Only then run: host.sh step fetch\n'
}

cmd_pull_lockfiles() {
    require_multipass
    run_logged pull-lockfiles pull_body
}

hex_bytes() {
    LC_ALL=C od -An -v -tx1 -j "$2" -N "$3" "$1" | tr -d ' \n'
}

reject_icon() {
    mv -f -- "$1" "$1.rejected"
    die "rejected $1: $2; renamed to $1.rejected"
}

# Pulled icons are untrusted binaries: header, size and (for PNG) dimensions must match the manifest; structure only, nothing is decoded.
check_pulled_icon() {
    local f="$1" kind="$2" want_w="$3" want_h="$4" size w h len hex8='^[0-9a-f]{8}$'
    [ -f "$f" ] && [ ! -L "$f" ] || die "rejected $f: not a regular file"
    size="$(wc -c < "$f" | tr -d ' ')"
    [ "$size" -le "$ICON_MAX_BYTES" ] || reject_icon "$f" "$size bytes exceeds the $ICON_MAX_BYTES-byte cap"
    case "$kind" in
        png)
            [ "$size" -ge 45 ] || reject_icon "$f" "$size bytes is too small to be a PNG"
            [ "$(hex_bytes "$f" 0 8)" = "89504e470d0a1a0a" ] || reject_icon "$f" "no PNG signature"
            [ "$(hex_bytes "$f" 12 4)" = "49484452" ] || reject_icon "$f" "first chunk is not IHDR"
            w="$(hex_bytes "$f" 16 4)"
            h="$(hex_bytes "$f" 20 4)"
            [[ "$w" =~ $hex8 && "$h" =~ $hex8 ]] || reject_icon "$f" "could not read the PNG dimensions"
            [ "$((16#$w))" -eq "$want_w" ] && [ "$((16#$h))" -eq "$want_h" ] \
                || reject_icon "$f" "PNG is $((16#$w))x$((16#$h)), the manifest says ${want_w}x${want_h}"
            [ "$(hex_bytes "$f" "$((size - 12))" 12)" = "0000000049454e44ae426082" ] \
                || reject_icon "$f" "does not end with a PNG IEND chunk"
            ;;
        icns)
            [ "$size" -ge 16 ] || reject_icon "$f" "$size bytes is too small to be an ICNS"
            [ "$(hex_bytes "$f" 0 4)" = "69636e73" ] || reject_icon "$f" "no ICNS signature"
            len="$(hex_bytes "$f" 4 4)"
            [[ "$len" =~ $hex8 ]] || reject_icon "$f" "could not read the ICNS length"
            [ "$((16#$len))" -eq "$size" ] || reject_icon "$f" "ICNS header says $((16#$len)) bytes, the file has $size"
            ;;
        *) die "unknown icon kind '$kind' for $f" ;;
    esac
}

# The manifest is read from this working tree, never from the VM; every line is validated before anything is transferred.
read_icon_manifest() {
    local manifest="$1" line n=0 kind width height path line_re='^(png|icns) [0-9]{1,5} [0-9]{1,5} [A-Za-z0-9._/@-]+$'
    ICON_ENTRIES=()
    ICON_PATHS=" "
    [ -f "$manifest" ] && [ ! -L "$manifest" ] || die "icon manifest not found: $manifest"
    while IFS= read -r line || [ -n "$line" ]; do
        n=$((n + 1))
        [ "$n" -le "$ICON_MAX_ENTRIES" ] || die "$manifest has more than $ICON_MAX_ENTRIES lines"
        [[ "$line" =~ $line_re ]] || die "$manifest line $n is not '<png|icns> <width> <height> <path>'"
        read -r kind width height path <<< "$line"
        case "$path" in
            /*|-*|.*|*/|*..*|*//*|*/.*|*/-*) die "$manifest line $n: path '$path' is not a plain repo-relative path" ;;
        esac
        case "$ICON_PATHS" in
            *" $path "*) die "$manifest line $n: duplicate path '$path'" ;;
        esac
        ICON_PATHS="$ICON_PATHS$path "
        ICON_ENTRIES+=("$line")
    done < "$manifest"
    [ "$n" -ge 1 ] || die "$manifest is empty"
}

pull_icons_body() {
    vm_exists || die "VM $VM_NAME does not exist"
    check_no_mounts
    local dest entry kind width height path
    read_icon_manifest "$REPO_ROOT/scripts/icons/manifest.txt"
    mkdir -p "$OUT_DIR"
    dest="$OUT_DIR/$(utc_stamp)"
    mkdir "$dest"
    dest="$dest/icons"
    mkdir "$dest"
    for entry in "${ICON_ENTRIES[@]}"; do
        read -r kind width height path <<< "$entry"
        mkdir -p "$(dirname "$dest/$path")"
        multipass transfer "$VM_NAME:$GUEST_ICONS/$path" "$dest/$path"
        check_pulled_icon "$dest/$path" "$kind" "$width" "$height"
        printf 'ok %s %s\n' "$kind" "$dest/$path"
    done
    printf 'Not copied into the repo; review before using.\n'
    printf '\nsha256 of the files as received:\n'
    for entry in "${ICON_ENTRIES[@]}"; do
        read -r kind width height path <<< "$entry"
        ( cd "$dest" && shasum -a 256 "$path" )
    done
    printf 'Compare these with the sha256sum lines in the latest icons log in %s.\n' "$LOG_DIR"
    printf '\nNext steps:\n'
    printf '  1. Compare the checksums, then review the images in %s; they are untrusted VM output until you accept them.\n' "$dest"
    printf '  2. If you accept them, copy them over the same paths in the repo, commit, and push.\n'
    printf '  3. Run host.sh clone, then host.sh step icons check; it must report every file as "same".\n'
}

cmd_pull_icons() {
    require_multipass
    run_logged pull-icons pull_icons_body
}

cmd_exec() {
    [ "${1:-}" = "--" ] || die "usage: host.sh exec -- <command...> (the -- separator is required)"
    shift
    [ $# -ge 1 ] || die "usage: host.sh exec -- <command...> (no command given)"
    require_multipass
    vm_exists || die "VM $VM_NAME does not exist; run: host.sh create"
    run_logged exec multipass exec "$VM_NAME" -- "$@"
}

cmd_info() {
    require_multipass
    multipass info "$VM_NAME" 2>&1 | sanitize
}

cmd_stop() {
    require_multipass
    run_quiet "stopping VM $VM_NAME..." multipass stop "$VM_NAME"
}

cmd_destroy() {
    require_multipass
    if vm_exists; then
        printf '%s\n' "$VM_INFO" | sanitize
    else
        die "VM $VM_NAME does not exist; nothing to delete"
    fi
    if [ "${SHELLCUE_VM_CONFIRM:-}" != "destroy" ]; then
        printf 'This would delete the Multipass instance "%s" and everything inside it (tools, checkout, build output).\n' "$VM_NAME"
        die "set SHELLCUE_VM_CONFIRM=destroy to proceed"
    fi
    printf 'This deletes the Multipass instance "%s" and everything inside it (tools, checkout, build output).\n' "$VM_NAME"
    run_quiet "deleting VM $VM_NAME..." multipass delete "$VM_NAME" 3>/dev/null
    printf 'Deleted %s. Its name stays taken, and Multipass may keep its disk, until you run "multipass purge" yourself;\n' "$VM_NAME"
    printf 'purge removes ALL deleted instances, which is why this script does not run it.\n'
}

main() {
    if [ $# -eq 0 ] || [ "$1" = "--help" ] || [ "$1" = "-h" ]; then
        usage
        exit 0
    fi
    local cmd="$1"
    shift
    case "$cmd" in
        create) [ $# -eq 0 ] || die "create takes no arguments"; cmd_create ;;
        push) [ $# -eq 0 ] || die "push takes no arguments"; require_multipass; run_logged push cmd_push ;;
        step) cmd_step "$@" ;;
        clone) [ $# -eq 0 ] || die "clone takes no arguments"; cmd_clone ;;
        pull-lockfiles) [ $# -eq 0 ] || die "pull-lockfiles takes no arguments"; cmd_pull_lockfiles ;;
        pull-icons) [ $# -eq 0 ] || die "pull-icons takes no arguments"; cmd_pull_icons ;;
        exec) cmd_exec "$@" ;;
        info) [ $# -eq 0 ] || die "info takes no arguments"; cmd_info ;;
        stop) [ $# -eq 0 ] || die "stop takes no arguments"; require_multipass; run_logged stop cmd_stop ;;
        destroy) [ $# -eq 0 ] || die "destroy takes no arguments"; cmd_destroy ;;
        *) usage >&2; die "unknown command: '$cmd'" ;;
    esac
}

main "$@"
