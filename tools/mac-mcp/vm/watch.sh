#!/bin/bash
# Follows what the MCP server in the VM does, live, from the host:
#   - every audit log record (tool calls, arguments, results, errors)
#   - every new screenshot the server writes, with an scp command to fetch it
# Ctrl+C stops it. Read-only: nothing in the guest changes.
#
#   vm/watch.sh            last 20 audit records, then follow
#   vm/watch.sh 100        last 100 audit records, then follow
#
# The same by hand:
#   ssh -i ~/.config/warpium-mac-mcp/vm_ed25519 admin@<ip> tail -f ~/.config/warpium-mac-mcp/audit.log
# (<ip>: `tart ip warpium-test`). For the guest screen itself: WARPIUM_VM_VNC=1 vm/start.sh.
source "$(dirname "$0")/common.sh"

RECORDS="${1:-20}"
case "$RECORDS" in '' | *[!0-9]*) die "usage: vm/watch.sh [number of past audit records]" ;; esac

require_macos_arm64
require_tart
require_key
vm_running "$VM_NAME" || die "VM $VM_NAME is not running; run vm/start.sh"
IP="$(vm_ip 30)" || die "no IP for $VM_NAME; see $VM_LOG"

step "Watching $GUEST_USER@$IP (audit log + new screenshots); Ctrl+C to stop"
echo "    fetch a screenshot: scp -i $SSH_KEY -o UserKnownHostsFile=$KNOWN_HOSTS -o HostKeyAlias=$VM_NAME $GUEST_USER@$IP:<path> ."

# -tt: a remote tty, so Ctrl+C (SIGINT/SIGHUP) also ends tail and the poll
# loop in the guest instead of leaving them running.
guest_ssh "$IP" -tt "RECORDS=$RECORDS bash -c '$(
  cat <<'GUEST'
audit="$HOME/.config/warpium-mac-mcp/audit.log"
shots="$HOME/warpium-mac-test/artifacts/screenshots"
seen="$(mktemp -t warpium-watch)"
trap "rm -f $seen; kill 0" EXIT INT TERM HUP
tail -n "$RECORDS" -F "$audit" 2>/dev/null | sed -u "s/^/[audit] /" &
mkdir -p "$shots"
ls -1 "$shots" 2>/dev/null | sort >"$seen"
while sleep 2; do
  now="$(ls -1 "$shots" 2>/dev/null | sort)"
  printf "%s\n" "$now" | comm -13 "$seen" - | grep -v -e "^$" -e "-raw\.png$" |
    while read -r f; do echo "[screenshot] $shots/$f"; done
  printf "%s\n" "$now" >"$seen"
done
GUEST
)'"
