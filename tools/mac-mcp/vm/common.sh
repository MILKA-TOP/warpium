# shellcheck shell=bash
# shellcheck disable=SC2034  # settings below are used by the scripts that source this
# Shared settings and helpers for the Tart VM scripts. Sourced, not executed.
# Written for the bash 3.2 that ships with macOS (no mapfile, no assoc arrays).
#
# Every setting can be overridden with an environment variable:
#   WARPIUM_VM_NAME        VM name (default warpium-test)
#   WARPIUM_VM_GOLDEN      golden snapshot name (default <name>-golden)
#   WARPIUM_VM_IMAGE       Tart image to clone (default: picked from the host macOS version)
#   WARPIUM_VM_CPU         vCPUs (default 4)
#   WARPIUM_VM_MEMORY_MB   RAM in MiB (default 8192)
#   WARPIUM_VM_DISK_GB     grow the disk to this size in GB (default: keep the image's 50 GB)
#   WARPIUM_MCP_PORT       MCP port, the same on host and guest (default 17654)
#   WARPIUM_MCP_CONFIG_DIR host config dir (default ~/.config/warpium-mac-mcp)

set -euo pipefail

VM_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MCP_DIR="$(cd "$VM_DIR/.." && pwd)"

VM_NAME="${WARPIUM_VM_NAME:-warpium-test}"
GOLDEN_NAME="${WARPIUM_VM_GOLDEN:-${VM_NAME}-golden}"
VM_CPU="${WARPIUM_VM_CPU:-4}"
VM_MEMORY_MB="${WARPIUM_VM_MEMORY_MB:-8192}"
VM_DISK_GB="${WARPIUM_VM_DISK_GB:-}"
MCP_PORT="${WARPIUM_MCP_PORT:-17654}"
CONF_DIR="${WARPIUM_MCP_CONFIG_DIR:-$HOME/.config/warpium-mac-mcp}"
STATE_DIR="$CONF_DIR/vm"
SSH_KEY="$CONF_DIR/vm_ed25519"
KNOWN_HOSTS="$CONF_DIR/vm_known_hosts"
ADMIN_PASSWORD_FILE="$CONF_DIR/vm_admin_password"
FORWARD_PID_FILE="$STATE_DIR/forward.pid"
VM_LOG="$STATE_DIR/$VM_NAME.log"
FORWARD_LOG="$STATE_DIR/forward.log"

GUEST_USER="admin"
# Default credentials of the cirruslabs images. setup.sh replaces the password
# with a random one and disables SSH password logins right after installing
# the key, so this is only used once, on a fresh image.
GUEST_DEFAULT_PASSWORD="admin"
GUEST_DIR="warpium-mac-mcp" # relative to the guest's home
GUEST_AGENT_LABEL="dev.milka.warpium-mac-mcp.vm"
# Non-login SSH shells don't read ~/.zprofile; Homebrew lives here on arm64.
GUEST_PATH='/opt/homebrew/bin:/opt/homebrew/sbin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin'

# --- output ---------------------------------------------------------------
if [ -t 1 ]; then
  _B=$'\033[1m' _G=$'\033[32m' _Y=$'\033[33m' _R=$'\033[31m' _N=$'\033[0m'
else
  _B="" _G="" _Y="" _R="" _N=""
fi
step() { printf '%s==> %s%s\n' "$_B" "$*" "$_N"; }
ok() { printf '%s    ok:%s %s\n' "$_G" "$_N" "$*"; }
warn() { printf '%s    warning:%s %s\n' "$_Y" "$_N" "$*" >&2; }
die() {
  printf '%serror:%s %s\n' "$_R" "$_N" "$*" >&2
  exit 1
}

# --- checks ---------------------------------------------------------------
require_macos_arm64() {
  [ "$(uname -s)" = "Darwin" ] || die "run this on the Mac host, not in the container"
  if [ "$(uname -m)" != "arm64" ]; then
    if [ "$(sysctl -n hw.optional.arm64 2>/dev/null)" = "1" ]; then
      die "this shell runs under Rosetta (uname -m is $(uname -m)); open a native arm64 terminal and re-run"
    fi
    die "Tart needs an Apple Silicon Mac (uname -m is $(uname -m))"
  fi
}

require_tart() {
  command -v tart >/dev/null 2>&1 || die "tart is not installed; run vm/setup.sh first"
}

require_key() {
  [ -f "$SSH_KEY" ] || die "no VM SSH key at $SSH_KEY; run vm/setup.sh first"
}

host_macos_major() {
  sw_vers -productVersion | cut -d. -f1
}

# Guest images must not be a newer major macOS than the host.
default_image() {
  local major
  major="$(host_macos_major)"
  if [ "$major" -ge 26 ]; then
    echo "ghcr.io/cirruslabs/macos-tahoe-vanilla:latest"
  elif [ "$major" -ge 15 ]; then
    echo "ghcr.io/cirruslabs/macos-sequoia-vanilla:latest"
  else
    die "host macOS $(sw_vers -productVersion) is too old; Tart guests need macOS 15 or newer here"
  fi
}

vm_exists() {
  tart list --source local -q 2>/dev/null | grep -qx "$1"
}

vm_running() {
  # Matches `tart run [flags] <name>` exactly (not <name>-golden).
  pgrep -f "tart run( .*)? $1\$" >/dev/null 2>&1
}

# --- ssh ------------------------------------------------------------------
# The host key is pinned per VM name (HostKeyAlias), not per IP: the DHCP
# address can change between boots, the key only changes with a new image.
vm_ip() {
  tart ip --wait "${1:-120}" "$VM_NAME" 2>/dev/null
}

# guest_ssh <ip> [remote command...]: runs with the dedicated key only.
guest_ssh() {
  local ip="$1"
  shift
  ssh -i "$SSH_KEY" -o IdentitiesOnly=yes -o UserKnownHostsFile="$KNOWN_HOSTS" \
    -o HostKeyAlias="$VM_NAME" -o StrictHostKeyChecking=accept-new -o ConnectTimeout=10 \
    -o BatchMode=yes -o LogLevel=ERROR "$GUEST_USER@$ip" "$@"
}

# The rsync -e value (rsync splits it on spaces, so paths are %q-quoted).
rsync_ssh_cmd() {
  printf 'ssh -i %q -o IdentitiesOnly=yes -o UserKnownHostsFile=%q -o HostKeyAlias=%q -o StrictHostKeyChecking=accept-new -o ConnectTimeout=10 -o BatchMode=yes -o LogLevel=ERROR' \
    "$SSH_KEY" "$KNOWN_HOSTS" "$VM_NAME"
}

wait_for_key_ssh() {
  local ip="$1" tries="${2:-45}" i
  for ((i = 0; i < tries; i++)); do
    if guest_ssh "$ip" true 2>/dev/null; then
      return 0
    fi
    sleep 2
  done
  return 1
}

# Starts the VM headless in the background (no window on your desktop).
start_vm_headless() {
  mkdir -p "$STATE_DIR"
  if vm_running "$VM_NAME"; then
    ok "VM $VM_NAME is already running"
    return 0
  fi
  step "Starting VM $VM_NAME headless (log: $VM_LOG)"
  nohup tart run --no-graphics "$VM_NAME" >>"$VM_LOG" 2>&1 </dev/null &
  disown || true
  sleep 3
  vm_running "$VM_NAME" || die "tart run exited immediately; see $VM_LOG"
}

stop_vm() {
  if vm_running "$VM_NAME"; then
    step "Stopping VM $VM_NAME"
    tart stop --timeout 60 "$VM_NAME" || warn "tart stop failed; check 'tart list'"
  fi
}

stop_forward() {
  if [ -f "$FORWARD_PID_FILE" ]; then
    local pid
    pid="$(cat "$FORWARD_PID_FILE")"
    if [ -n "$pid" ] && ps -p "$pid" -o command= 2>/dev/null | grep -q "127.0.0.1:$MCP_PORT:127.0.0.1:$MCP_PORT"; then
      kill "$pid" 2>/dev/null || true
      ok "stopped SSH forward (pid $pid)"
    fi
    rm -f "$FORWARD_PID_FILE"
  fi
}

# Who listens on the host port, if anyone ("" when free).
host_port_listener() {
  lsof -nP -iTCP:"$MCP_PORT" -sTCP:LISTEN -Fpc 2>/dev/null | tr '\n' ' ' || true
}
