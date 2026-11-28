#!/bin/bash
# Runs on the owner's Mac and drives the Ubuntu desktop VM over SSH; must stay compatible with macOS /bin/bash 3.2.
# Never runs anything the VM produced, never copies a file out of the VM (the package stays there), writes only to ../vm-logs and ../vm-linux/known_hosts.
# Change together with scripts/vm/mac-host.sh: the two differ only in names and help text.

set -euo pipefail

GUEST_FILE="shellcue-guest.sh"
GUEST_STEPS="tools clone fetch web specs rust-check rust-test build bundle info"

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
OUT_PARENT="$(cd "$REPO_ROOT/.." && pwd)"
LOG_DIR="$OUT_PARENT/vm-logs"
STATE_DIR="$OUT_PARENT/vm-linux"
TARGET_FILE="$STATE_DIR/target"
PUBKEY_FILE="$STATE_DIR/id.pub"
KNOWN_HOSTS="$STATE_DIR/known_hosts"
AGENT_SOCK="$HOME/Library/Group Containers/2BUA8C4S2C.com.1password/t/agent.sock"

QUIET=0
VM_USER=""
VM_HOST=""
SSH_OPTS=()

usage() {
    cat <<EOF
Usage: linux-host.sh <command>

Drives the Ubuntu desktop VM (UTM) over SSH: it builds, packages and trials ShellCue for Linux. Logs go to
$LOG_DIR (outside the repo), named <timestamp>-linux-<step>.log. Only text comes back; no file is copied out of
the VM. step and clone print only the lines starting ==> or -->, plus the pushing line (and the last log lines if
the step fails); the log has everything. Long steps therefore sit quiet here while they work. step info, trust
and push print everything. To see every line of a step here too:
  LINUX_HOST_VERBOSE=1 scripts/vm/linux-host.sh step rust-test

Commands:
  trust <fingerprint>     record the VM's SSH host key in vm-linux/known_hosts, only if the key the VM presents has
                          exactly this fingerprint (SHA256:...), which you read in the VM's own window
  push                    copy scripts/vm/guest.sh from this working tree into the VM, logged as "linux-push"
  step <name> [args...]   push, then run guest.sh <name> in the VM over the same connection, logged;
                          steps: $GUEST_STEPS
  clone                   step clone <current branch> <HEAD sha>; refuses if tracked files are uncommitted or the
                          commit is not what the local copy of origin/<branch> points at

Settings, all in $STATE_DIR:
  target                  one line: <user>@<host>, the VM's login user and its address or name
  id.pub                  one line: the public half of the SSH key kept in 1Password for this VM
  known_hosts             written by "trust"; never edited by hand (delete it to pin again)

Every ssh connection ignores ~/.ssh/config, offers only the key in id.pub (signed by the 1Password agent),
accepts only the host key in known_hosts, and switches off agent forwarding, port forwarding and X11.
1Password asks you to approve the key's use; that prompt is expected, once per command unless 1Password is set
to remember the approval. A prompt that is dismissed or times out shows up here as exit status 255.
Nothing sent over SSH uses sudo with a password: anything that needs it is typed by you in the VM's own window.

One-time setup. Run every linux-host.sh command from the repo root: $REPO_ROOT

 A. On this Mac:
      mkdir -p "$STATE_DIR"
    In 1Password, create an SSH key item (type Ed25519) for this VM and copy its "public key" field. Then:
      pbpaste > "$STATE_DIR/id.pub"
      ssh-keygen -lf "$STATE_DIR/id.pub"
    The last command prints the key's fingerprint; keep it for step B3.

 B. In the VM's own window (Ubuntu 24.04 desktop, created in UTM with no shared directory), in its Terminal:
    1. Install the SSH server (sudo asks for this VM user's password and shows nothing while you type):
         sudo apt install openssh-server
    2. Put the public key line into the VM. Paste it if UTM shares the clipboard, or type it:
         mkdir -p ~/.ssh && chmod 700 ~/.ssh
         nano ~/.ssh/authorized_keys        (one line: the whole line from id.pub; save with Ctrl-O, Ctrl-X)
         chmod 600 ~/.ssh/authorized_keys
    3. Check it arrived intact; this must print the same SHA256:... as in step A:
         ssh-keygen -lf ~/.ssh/authorized_keys
    4. Switch off password logins over SSH, so only that key gets in:
         echo 'PasswordAuthentication no' | sudo tee /etc/ssh/sshd_config.d/10-shellcue.conf
         sudo systemctl restart ssh
    5. Note the user name and the VM's name; the name keeps working when the VM's address changes:
         whoami
         hostname                           (write it in lower case and add .local, for example shellcue-linux.local)
       If the name does not connect later, use the address instead: hostname -I   (the first one, of the form 192.168.x.x)
    6. Read the VM's host key fingerprint; it is the second word, starting SHA256:
         ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub
    7. Keep the VM awake during builds: Settings > Power: Screen Blank "Never", Automatic Suspend off.
    8. Check the VM reaches the network: wget -q --spider https://github.com && echo ok   should print ok.
       If it does not, check that UTM's Network Mode for this VM is Shared Network.

 C. On this Mac, with the user and name from B5 (this example assumes user "builder"):
      printf '%s\n' 'builder@shellcue-linux.local' > "$STATE_DIR/target"

If the VM's address or name changes: write the new one into target, delete known_hosts, and run trust again
with the same fingerprint (the fingerprint only changes if the VM is rebuilt).

Typical first session (replace the whole word after "trust" with the SHA256:... word from B6, no brackets):
  scripts/vm/linux-host.sh trust SHA256:xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx
  scripts/vm/linux-host.sh step info
  scripts/vm/linux-host.sh step tools
The first tools run stops when system packages are missing: it writes ~/shellcue-apt.sh in the VM and asks you to
run, in the VM's own Terminal,   sudo bash ~/shellcue-apt.sh   and then step tools again. After that:
  scripts/vm/linux-host.sh clone
  scripts/vm/linux-host.sh step fetch
  scripts/vm/linux-host.sh step web
  scripts/vm/linux-host.sh step rust-check
  scripts/vm/linux-host.sh step rust-test

To build the programs and the Debian package. The package stays in the VM, in ~/shellcue/build/linux. fetch and
web above must have succeeded at the same commit; after every new clone repeat fetch, web, specs, build and bundle:
  scripts/vm/linux-host.sh step specs
  scripts/vm/linux-host.sh step build
  scripts/vm/linux-host.sh step bundle
The bundle step ends by printing the command that installs the package; you type it in the VM's own Terminal.
Take a UTM snapshot before installing, and do not build again until the snapshot is restored.
EOF
}


die() {
    printf 'linux-host.sh: ERROR: %s\n' "$*" >&2
    exit 1
}

# VM output is untrusted: drop control bytes (ESC, CR, ...) but keep tab, newline and bytes >= 0x80.
sanitize() {
    LC_ALL=C tr -u -d '\000-\010\013-\037\177'
}

utc_stamp() {
    date -u +%Y%m%dT%H%M%SZ
}

# Prints the file's single line; an empty file or a second line is an error (callers reject an empty line).
read_one_line() {
    local file="$1" what="$2" line="" extra=""
    [ -f "$file" ] && [ ! -L "$file" ] || die "$what not found (or is a symlink): $file"
    {
        IFS= read -r line || [ -n "$line" ] || die "$file is empty"
        if IFS= read -r extra || [ -n "$extra" ]; then
            die "$file must hold exactly one line"
        fi
    } < "$file"
    printf '%s' "$line"
}

load_target() {
    local target lower user_re='^[a-z_][a-z0-9_-]{0,31}$' host_re='^[A-Za-z0-9]([A-Za-z0-9.-]{0,251}[A-Za-z0-9])?$'
    case "$STATE_DIR" in
        *'"'*|*$'\n'*) die "the path $STATE_DIR contains a quote or newline, which ssh options cannot carry" ;;
    esac
    [ -d "$STATE_DIR" ] || die "settings folder not found: $STATE_DIR (see: linux-host.sh --help)"
    target="$(read_one_line "$TARGET_FILE" "target file")"
    case "$target" in
        *@*@*|@*|*@) die "$TARGET_FILE must hold <user>@<host>, got '$target'" ;;
        *@*) ;;
        *) die "$TARGET_FILE must hold <user>@<host>, got '$target'" ;;
    esac
    VM_USER="${target%@*}"
    VM_HOST="${target#*@}"
    [[ "$VM_USER" =~ $user_re ]] || die "user name in $TARGET_FILE is not a plain login name: '$VM_USER'"
    [[ "$VM_HOST" =~ $host_re ]] || die "host in $TARGET_FILE is not a plain name or IPv4 address: '$VM_HOST'"
    # ssh and ssh-keyscan write host names in lower case, and the pinned line is compared exactly.
    lower="$(printf '%s' "$VM_HOST" | LC_ALL=C tr 'A-Z' 'a-z')"
    [ "$lower" = "$VM_HOST" ] || die "host in $TARGET_FILE has capital letters; write it in lower case: '$lower'"
}

load_ssh() {
    local pub pub_re='^ssh-ed25519 [A-Za-z0-9+/]{68}( [A-Za-z0-9 ._@:+=-]{0,200})?$'
    load_target
    pub="$(read_one_line "$PUBKEY_FILE" "public key file")"
    [[ "$pub" =~ $pub_re ]] || die "$PUBKEY_FILE must hold one ssh-ed25519 PUBLIC key line"
    [ -S "$AGENT_SOCK" ] || die "1Password SSH agent socket not found: $AGENT_SOCK (is the agent switched on in 1Password > Settings > Developer?)"
    [ -f "$KNOWN_HOSTS" ] && [ ! -L "$KNOWN_HOSTS" ] || die "no pinned host key yet; run: linux-host.sh trust <fingerprint>"
    command -v ssh >/dev/null 2>&1 || die "ssh not found on PATH"
    SSH_OPTS=(
        -F none -T -a -x
        -o "IdentityAgent=\"$AGENT_SOCK\""
        -o "IdentityFile=\"$PUBKEY_FILE\""
        -o IdentitiesOnly=yes
        -o "UserKnownHostsFile=\"$KNOWN_HOSTS\""
        -o GlobalKnownHostsFile=/dev/null
        -o StrictHostKeyChecking=yes
        -o UpdateHostKeys=no
        -o HostKeyAlgorithms=ssh-ed25519
        -o CheckHostIP=no
        -o PreferredAuthentications=publickey
        -o PubkeyAuthentication=yes
        -o PasswordAuthentication=no
        -o KbdInteractiveAuthentication=no
        -o BatchMode=yes
        -o ForwardAgent=no
        -o ForwardX11=no
        -o ForwardX11Trusted=no
        -o ClearAllForwardings=yes
        -o Tunnel=no
        -o PermitLocalCommand=no
        -o ControlMaster=no
        -o ControlPath=none
        -o EscapeChar=none
        -o RequestTTY=no
        -o ConnectTimeout=20
        -o ServerAliveInterval=30
        -o ServerAliveCountMax=6
        -l "$VM_USER"
    )
}

# The remote login shell re-parses the command line, so only words that need no quoting may be sent.
require_plain_word() {
    local word="$1" word_re='^[A-Za-z0-9._/-]+$'
    [[ "$word" =~ $word_re ]] || die "argument '$word' has characters outside A-Z a-z 0-9 . _ / -, which this script will not send to the VM"
    case "$word" in
        -*) die "argument '$word' starts with a dash, which this script will not send to the VM" ;;
    esac
}

# The log always gets every line; with QUIET=1 the terminal gets only the step, part and verdict lines.
terminal_filter() {
    if [ "$QUIET" -eq 1 ]; then
        LC_ALL=C grep --line-buffered -E '^(==> |--> |pushing )' || true
    else
        cat
    fi
}

run_logged() {
    local label="$1" ts log rc tr_rc tee_rc statuses cmdline
    shift
    mkdir -p "$LOG_DIR"
    ts="$(utc_stamp)"
    log="$LOG_DIR/$ts-$label.log"
    [ ! -e "$log" ] || die "log file already exists: $log"
    printf '==> linux-host.sh %s at %s (host repo HEAD %s)\n' "$label" "$ts" \
        "$(git -C "$REPO_ROOT" rev-parse HEAD 2>/dev/null || printf 'unknown')" | tee "$log"
    cmdline="$(printf '%q ' "$@")"
    printf 'command: %s\n' "${cmdline% }" | tee -a "$log"
    set +e
    ( set -e; "$@" ) 2>&1 | sanitize | tee -a "$log" | terminal_filter
    statuses=("${PIPESTATUS[@]}")
    set -e
    rc="${statuses[0]}"
    tr_rc="${statuses[1]}"
    tee_rc="${statuses[2]}"
    if [ "$tr_rc" -ne 0 ] || [ "$tee_rc" -ne 0 ]; then
        printf 'linux-host.sh: WARNING: sanitize exited %s, tee exited %s; the output and log may be incomplete\n' "$tr_rc" "$tee_rc" >&2
        if [ "$rc" -eq 0 ]; then
            if [ "$tr_rc" -ne 0 ]; then rc="$tr_rc"; else rc="$tee_rc"; fi
        fi
    fi
    if [ "$QUIET" -eq 1 ] && [ "$rc" -ne 0 ]; then
        printf 'last lines of the log (when a step has several parts, an earlier part'"'"'s error may be higher up in the log):\n'
        tail -n 20 "$log" | sed 's/^/  | /'
    fi
    printf 'log: %s\nexit status: %s\n' "$log" "$rc" | tee -a "$log"
    ssh_hint "$rc"
    return "$rc"
}

# Writes known_hosts only when the key the VM presents matches the fingerprint the owner read inside the VM.
trust_body() {
    local want="$1" scan count line got tmp key_re='^ssh-ed25519 [A-Za-z0-9+/]{68}$'
    command -v ssh-keyscan >/dev/null 2>&1 || die "ssh-keyscan not found on PATH"
    command -v ssh-keygen >/dev/null 2>&1 || die "ssh-keygen not found on PATH"
    scan="$(ssh-keyscan -T 15 -t ed25519 "$VM_HOST" 2>/dev/null | LC_ALL=C grep -v '^#' || true)"
    [ -n "$scan" ] || die "no ed25519 host key received from $VM_HOST (is the VM running, with the SSH server installed?)"
    count="$(printf '%s\n' "$scan" | wc -l | tr -d ' ')"
    [ "$count" -eq 1 ] || die "expected one host key line from $VM_HOST, got $count"
    line="$scan"
    case "$line" in
        "$VM_HOST "*) ;;
        *) die "host key line from ssh-keyscan does not start with '$VM_HOST '" ;;
    esac
    [[ "${line#"$VM_HOST" }" =~ $key_re ]] || die "host key line from $VM_HOST is not a plain ssh-ed25519 key"
    got="$(printf '%s\n' "$line" | ssh-keygen -l -E sha256 -f - | awk '{print $2}')"
    printf 'fingerprint presented by %s: %s\n' "$VM_HOST" "$got"
    printf 'fingerprint you supplied:      %s\n' "$want"
    if [ "$got" != "$want" ]; then
        die "the fingerprints differ; nothing was written. A typing slip gives this message too: compare the two lines above character by character against the VM's own window and run trust again. If the VM's window really shows a different fingerprint from the one presented, stop: something else is answering at $VM_HOST"
    fi
    if [ -e "$KNOWN_HOSTS" ] || [ -L "$KNOWN_HOSTS" ]; then
        [ -f "$KNOWN_HOSTS" ] && [ ! -L "$KNOWN_HOSTS" ] || die "$KNOWN_HOSTS exists but is not a regular file"
        if [ "$(cat "$KNOWN_HOSTS")" = "$line" ]; then
            printf 'already pinned: %s\n' "$KNOWN_HOSTS"
            return 0
        fi
        die "$KNOWN_HOSTS already pins a different key or host. If the VM's address or name changed, or you rebuilt the VM on purpose, delete that file yourself and run trust again"
    fi
    tmp="$KNOWN_HOSTS.tmp.$$"
    ( umask 077; printf '%s\n' "$line" > "$tmp" )
    mv -f -- "$tmp" "$KNOWN_HOSTS"
    printf 'pinned the host key of %s in %s\n' "$VM_HOST" "$KNOWN_HOSTS"
}

cmd_trust() {
    [ $# -eq 1 ] || die "trust takes one word: the SHA256:... word (the second word) that ssh-keygen -lf printed in the VM's own window"
    local want="$1" fp_re='^SHA256:[A-Za-z0-9+/]{43}$'
    [[ "$want" =~ $fp_re ]] || die "the fingerprint must be SHA256: followed by exactly 43 letters, digits, + or /, got '$want'"
    load_target
    run_logged linux-trust trust_body "$want"
}

# One connection per command, so 1Password asks once: cat reads the script to end of input, which leaves the step no stdin.
push_body() {
    local src="$SCRIPT_DIR/guest.sh" sum remote
    [ -f "$src" ] || die "guest script not found: $src"
    sum="$(shasum -a 256 "$src" | awk '{print $1}')"
    remote="/bin/cat > ./$GUEST_FILE.tmp && /bin/mv -f ./$GUEST_FILE.tmp ./$GUEST_FILE"
    [ $# -eq 0 ] || remote="$remote && /bin/bash ./$GUEST_FILE $*"
    printf 'pushing %s -> %s:~/%s (sha256 %s); approve the 1Password prompt if one appears\n' "$src" "$VM_HOST" "$GUEST_FILE" "$sum"
    ssh "${SSH_OPTS[@]}" "$VM_HOST" "$remote" < "$src"
}

# ssh reports its own failures as 255, with only a terse message.
ssh_hint() {
    [ "$1" -eq 255 ] || return 0
    printf '\nlinux-host.sh: ssh itself failed (exit status 255). Usual causes, by the message above:\n' >&2
    printf '  Permission denied (publickey)   the 1Password prompt was dismissed or timed out, or the key is not in the\n' >&2
    printf '                                  VM'"'"'s ~/.ssh/authorized_keys (setup step B2)\n' >&2
    printf '  timed out / No route to host    the VM is off, asleep or has a new address, or its SSH server is not running\n' >&2
    printf '  Could not resolve hostname      the name in %s is wrong; use the address instead\n' "$TARGET_FILE" >&2
    printf '  Host key verification failed    the address or name changed, or the VM was rebuilt: see --help\n' >&2
    printf 'If a step had already started, it may still be running in the VM; the next step refuses to start until it ends.\n' >&2
}

cmd_step() {
    [ $# -ge 1 ] || die "usage: linux-host.sh step <name> [args...]; steps: $GUEST_STEPS"
    local name="$1" word
    case " $GUEST_STEPS " in
        *" $name "*) ;;
        *) die "unknown step '$name'; valid steps: $GUEST_STEPS" ;;
    esac
    for word in "$@"; do
        require_plain_word "$word"
    done
    load_ssh
    # info exists for its detail lines, so it is never shortened.
    if [ "$name" != "info" ] && [ "${LINUX_HOST_VERBOSE:-0}" != "1" ]; then QUIET=1; fi
    run_logged "linux-$name" push_body "$@"
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

cmd_push() {
    load_ssh
    run_logged linux-push push_body
}

main() {
    if [ $# -eq 0 ] || [ "$1" = "--help" ] || [ "$1" = "-h" ]; then
        usage
        exit 0
    fi
    local cmd="$1"
    shift
    case "$cmd" in
        trust) cmd_trust "$@" ;;
        push) [ $# -eq 0 ] || die "push takes no arguments"; cmd_push ;;
        step) cmd_step "$@" ;;
        clone) [ $# -eq 0 ] || die "clone takes no arguments"; cmd_clone ;;
        *) usage >&2; die "unknown command: '$cmd'" ;;
    esac
}

main "$@"
