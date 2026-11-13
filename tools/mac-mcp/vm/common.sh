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
#   WARPIUM_VM_SUGGEST_SUBNET  vmnet host address suggested when a VPN/LAN route
#                          hides the VM network (default 192.168.251.1)
#   WARPIUM_VM_SKIP_ROUTE_CHECK=1  skip that route check
#   WARPIUM_VM_NET         network mode: nat (default), bridged or softnet. Saved in
#                          $CONF_DIR/vm/net_mode, so later runs reuse it.
#   WARPIUM_VM_BRIDGE_IF   host interface for bridged mode (default: the physical
#                          interface of a default route, skipping VPN utun/ipsec/ppp).
#                          Saved in $CONF_DIR/vm/bridge_if like WARPIUM_VM_NET.
#   WARPIUM_VM_BRIDGED_FRESH_OK=1  allow a not-yet-hardened image (SSH password logins
#                          and Screen Sharing still on) to boot bridged without asking
#   WARPIUM_VM_CHANGE_PASSWORD=1   also replace the guest's admin/admin password with a
#                          random one (opt-in; see harden_guest below)

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
# A password WARPIUM_VM_CHANGE_PASSWORD=1 is applying; renamed to
# $ADMIN_PASSWORD_FILE once the guest uses it everywhere.
PENDING_PASSWORD_FILE="$ADMIN_PASSWORD_FILE.pending"
FORWARD_PID_FILE="$STATE_DIR/forward.pid"
VM_LOG="$STATE_DIR/$VM_NAME.log"
FORWARD_LOG="$STATE_DIR/forward.log"
VNC_LOG_MARK_FILE="$STATE_DIR/$VM_NAME.vnc-log-mark"

# vmnet's shared (NAT) network, used by Tart VMs. Unset keys mean 192.168.64.1/24.
# `defaults` takes the path *without* .plist: it appends the extension, so
# writing to ".../com.apple.vmnet.plist" creates com.apple.vmnet.plist.plist,
# which vmnet never reads. Reads go to the exact file with plutil.
VMNET_DOMAIN="/Library/Preferences/SystemConfiguration/com.apple.vmnet"
VMNET_PLIST="$VMNET_DOMAIN.plist"
VMNET_STRAY_PLIST="$VMNET_PLIST.plist"
VMNET_DEFAULT_ADDRESS="192.168.64.1"
VMNET_MASK="255.255.255.0"
VM_SUGGEST_SUBNET="${WARPIUM_VM_SUGGEST_SUBNET:-192.168.251.1}"
NET_MODE_FILE="$STATE_DIR/net_mode"
BRIDGE_IF_FILE="$STATE_DIR/bridge_if"
HARDENED_MARKER="$STATE_DIR/$VM_NAME.hardened"

GUEST_USER="admin"
# Default credentials of the cirruslabs images. The password stays (auto-login
# and the login keychain depend on it) unless WARPIUM_VM_CHANGE_PASSWORD=1;
# setup.sh closes the network logins that would accept it instead.
GUEST_DEFAULT_PASSWORD="admin"
SCREEN_SHARING_PORT=5900
ARD_PORT=3283
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
# In bridged mode the LAN router hands out the address, so the macOS DHCP
# leases don't know it; Tart then reads the host's ARP table instead.
vm_ip() {
  if [ "$(vm_net_mode)" = "bridged" ]; then
    tart ip --wait "${1:-120}" --resolver=arp "$VM_NAME" 2>/dev/null
  else
    tart ip --wait "${1:-120}" "$VM_NAME" 2>/dev/null
  fi
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

# Shown when sshd can't be reached after the route check.
ssh_unreachable_hint() {
  resolve_net_args
  if [ "${WARPIUM_VM_SKIP_ROUTE_CHECK:-0}" = "1" ]; then
    cat <<EOF
The route check was skipped (WARPIUM_VM_SKIP_ROUTE_CHECK=1): unset it to check whether a
VPN or LAN route hides the VM's address. Network: $NET_DESC.
EOF
  else
    cat <<EOF
The route check passed (the host routes the VM's address where the network mode puts it:
$NET_DESC), so the guest itself is probably still booting or stuck.
EOF
  fi
  cat <<EOF
Check the VM log: $VM_LOG
To look at the guest screen: vm/stop.sh, then 'tart run ${NET_ARGS[*]:+${NET_ARGS[*]} }$VM_NAME'
(opens a window; user admin, password admin, or the one in $ADMIN_PASSWORD_FILE if it exists).
The vanilla image has no Tart guest agent, so 'tart exec' does not work.
EOF
}

# ssh_with_password <ip> <password> [command...]: logs in with a password or
# keyboard-interactive only (no keys, no agent). stdin is passed on.
ssh_with_password() {
  local ip="$1" pw="$2" askpass rc=0
  shift 2
  mkdir -p "$STATE_DIR"
  # The helper holds no secret: the password reaches it in the environment of
  # this ssh process only, so nothing is left on disk if the run is killed.
  askpass="$(mktemp "$STATE_DIR/askpass.XXXXXX")"
  # shellcheck disable=SC2016  # expanded by the helper, not here
  printf '#!/bin/sh\nprintf "%%s\\n" "$WARPIUM_ASKPASS_PW"\n' >"$askpass"
  chmod 700 "$askpass"
  WARPIUM_ASKPASS_PW="$pw" SSH_ASKPASS="$askpass" SSH_ASKPASS_REQUIRE=force DISPLAY=none \
    ssh -o PubkeyAuthentication=no -o PreferredAuthentications=password,keyboard-interactive \
    -o UserKnownHostsFile="$KNOWN_HOSTS" -o HostKeyAlias="$VM_NAME" \
    -o StrictHostKeyChecking=accept-new -o ConnectTimeout=10 -o LogLevel=ERROR \
    -o NumberOfPasswordPrompts=1 "$GUEST_USER@$ip" "$@" || rc=$?
  rm -f "$askpass"
  return "$rc"
}

# --- guest hardening ----------------------------------------------------------
# The guest keeps the image's admin/admin password by default. Changing it has
# side effects: auto-login reads it from /etc/kcpassword, the login keychain
# stays encrypted with the old one (keychain dialogs would block UI tests) and
# the SecureToken is tied to it. Instead, harden_guest closes every network
# login that would accept it: SSH becomes key-only for admin, and Screen
# Sharing / Remote Management are turned off. The local console (`tart run`
# window, Virtualization.framework display, not VNC) still takes admin/admin.
# WARPIUM_VM_CHANGE_PASSWORD=1 replaces the password as well, updating
# /etc/kcpassword and the login keychain with it.

# The guest's sshd settings and service switches. Reads OLD, NEW (empty: keep
# the password), KC (base64 /etc/kcpassword for NEW) and KC_LEVEL (error, or
# warning when only re-checking an earlier change) from the lines before it.
guest_harden_script() {
  cat <<'GUEST'
set -euo pipefail
# Passwordless sudo for admin is preconfigured in the cirruslabs images.
sudo -n true || { echo "passwordless sudo is not available in the guest" >&2; exit 1; }

# 1. Key-only SSH for admin. sshd keeps the first value it reads, so the file
#    must sort before macOS's own 100-macos.conf. sshd starts per connection
#    (launchd), so new logins use it at once.
printf 'PasswordAuthentication no\nKbdInteractiveAuthentication no\nPermitRootLogin no\nAllowUsers %s\n' "$USER" |
  sudo tee /etc/ssh/sshd_config.d/000-warpium.conf >/dev/null
sudo rm -f /etc/ssh/sshd_config.d/100-warpium.conf
eff="$(sudo /usr/sbin/sshd -T 2>/dev/null)" ||
  { echo "error: 'sshd -T' failed; check /etc/ssh/sshd_config(.d)" >&2; exit 1; }
eff="$(printf '%s\n' "$eff" | tr 'A-Z' 'a-z')"
for opt in "passwordauthentication no" "kbdinteractiveauthentication no" \
  "permitrootlogin no" "allowusers $(printf '%s' "$USER" | tr 'A-Z' 'a-z')"; do
  if ! printf '%s\n' "$eff" | grep -qx "$opt"; then
    echo "error: sshd's effective config doesn't have '$opt'; check /etc/ssh/sshd_config(.d)" >&2
    exit 1
  fi
done

# 2. Screen Sharing (VNC, port 5900) and Remote Management (ARD, 3283) off,
#    also after reboots. The host checks both ports next.
sudo launchctl disable system/com.apple.screensharing
sudo launchctl bootout system/com.apple.screensharing 2>/dev/null || true
KS=/System/Library/CoreServices/RemoteManagement/ARDAgent.app/Contents/Resources/kickstart
if [ -x "$KS" ]; then
  sudo "$KS" -deactivate -stop >/dev/null 2>&1 || echo "    note: 'kickstart -deactivate -stop' failed" >&2
fi

# 3. Optional password change (WARPIUM_VM_CHANGE_PASSWORD=1). Resumable: every
#    step checks whether it is already done.
[ -n "$NEW" ] || exit 0
pw_ok() { dscl . -authonly "$USER" "$1" >/dev/null 2>&1; }
if ! pw_ok "$NEW"; then
  pw_ok "$OLD" || { echo "error: the admin password is neither the image's default nor the new one" >&2; exit 1; }
  # A change with the old password (not a reset) keeps the SecureToken in step.
  dscl . -passwd "/Users/$USER" "$OLD" "$NEW" >/dev/null 2>&1 ||
    sudo sysadminctl -resetPasswordFor "$USER" -newPassword "$NEW" \
      -adminUser "$USER" -adminPassword "$OLD" >/dev/null 2>&1 || true
  # sysadminctl can exit 0 on failure: check the result itself.
  pw_ok "$NEW" || { echo "error: could not change the admin password (dscl and sysadminctl failed)" >&2; exit 1; }
fi
# The login keychain must open with the new password at auto-login, or
# macOS shows "keychain password" dialogs.
KC_DB="$HOME/Library/Keychains/login.keychain-db"
if [ -f "$KC_DB" ] && ! security set-keychain-password -o "$OLD" -p "$NEW" "$KC_DB" >/dev/null 2>&1 &&
  ! security unlock-keychain -p "$NEW" "$KC_DB" >/dev/null 2>&1; then
  echo "${KC_LEVEL}: could not move the login keychain to the new password; if keychain" >&2
  echo "       dialogs appear, delete ~/Library/Keychains/login.keychain-db in the guest" >&2
  echo "       (it is recreated at login) and re-run" >&2
  [ "$KC_LEVEL" != error ] || exit 1
fi
# Auto-login reads the password from /etc/kcpassword.
printf '%s' "$KC" | base64 -D | sudo tee /etc/kcpassword >/dev/null
sudo chmod 600 /etc/kcpassword
sudo chown root:wheel /etc/kcpassword
sudo defaults write /Library/Preferences/com.apple.loginwindow autoLoginUser "$USER"
GUEST
}

# Prints which password the guest's admin account takes: pending, saved,
# default or unknown. Reads PENDING, SAVED and OLD like guest_harden_script.
guest_password_probe_script() {
  cat <<'GUEST'
pw_ok() { [ -n "$1" ] && dscl . -authonly "$USER" "$1" >/dev/null 2>&1; }
if pw_ok "$PENDING"; then echo pending
elif pw_ok "$SAVED"; then echo saved
elif pw_ok "$OLD"; then echo default
else echo unknown
fi
GUEST
}

# require_port_closed <ip> <port> <name>: dies unless <port> stays closed while
# SSH on the same address answers, so an unreachable guest never passes.
require_port_closed() {
  local ip="$1" port="$2" name="$3" i
  for i in 1 2 3 4 5; do
    if ! nc -z -G 3 "$ip" "$port" >/dev/null 2>&1; then
      nc -z -G 3 "$ip" 22 >/dev/null 2>&1 && {
        ok "port $port ($name) is closed"
        return 0
      }
    fi
    sleep 2
  done
  if nc -z -G 3 "$ip" 22 >/dev/null 2>&1; then
    die "$name still accepts connections on $ip:$port; re-run setup.sh, or turn it off in the guest's System Settings > General > Sharing"
  fi
  die "could not check port $port ($name): the guest stopped answering on $ip:22; re-run setup.sh"
}

# harden_guest <ip>: applies the hardening above over key SSH, then checks from
# the host that Screen Sharing is closed and SSH refuses passwords, and only
# then writes $HARDENED_MARKER. Idempotent; safe to re-run after any failure.
harden_guest() {
  local ip="$1" saved="" pending="" state new="" kc="" kc_level=error pw out methods
  [ -s "$ADMIN_PASSWORD_FILE" ] && saved="$(head -1 "$ADMIN_PASSWORD_FILE")"
  [ -s "$PENDING_PASSWORD_FILE" ] && pending="$(head -1 "$PENDING_PASSWORD_FILE")"
  state="$({
    printf 'PENDING=%q\nSAVED=%q\nOLD=%q\n' "$pending" "$saved" "$GUEST_DEFAULT_PASSWORD"
    guest_password_probe_script
  } | guest_ssh "$ip" 'bash -s')" || die "SSH to the guest failed while checking its password; re-run setup.sh"

  case "$state" in
    default)
      # Password files whose password the guest never took (an interrupted
      # earlier run) would only mislead: remove them.
      if [ -n "$saved" ]; then
        rm -f "$ADMIN_PASSWORD_FILE"
        saved=""
        warn "removed $ADMIN_PASSWORD_FILE: the guest never took that password (it still uses admin/admin)"
      fi
      if [ -n "$pending" ] && [ "${WARPIUM_VM_CHANGE_PASSWORD:-0}" != "1" ]; then
        rm -f "$PENDING_PASSWORD_FILE"
        pending=""
        warn "removed $PENDING_PASSWORD_FILE: that password change was never applied"
      fi
      if [ "${WARPIUM_VM_CHANGE_PASSWORD:-0}" = "1" ]; then
        if [ -n "$pending" ]; then
          new="$pending"
        else
          new="$(openssl rand -hex 16)" || die "openssl rand failed"
          # Only a pending file until the guest uses it everywhere.
          (umask 077 && printf '%s\n' "$new" >"$PENDING_PASSWORD_FILE")
          pending="$new"
        fi
      fi
      ;;
    pending)
      echo "    resuming the admin password change from $PENDING_PASSWORD_FILE"
      new="$pending"
      ;;
    saved)
      # Changed by an earlier run. Once this version recorded it, nothing is
      # left to do; after an older setup.sh, re-apply keychain and auto-login
      # (no-ops when already done), warning instead of failing on the keychain.
      [ -z "$pending" ] || { rm -f "$PENDING_PASSWORD_FILE" && pending=""; }
      if ! grep -qx "password=changed" "$HARDENED_MARKER" 2>/dev/null; then
        new="$saved"
        kc_level=warning
      fi
      ;;
    unknown)
      warn "the guest's admin password is neither admin nor a saved one; leaving it as it is"
      warn "(auto-login may fail; log in at the VM's screen to check)"
      ;;
    *) die "unexpected answer from the guest password check: '$state'" ;;
  esac

  if [ -n "$new" ]; then
    kc="$(printf '%s\n' "$new" | uv run --quiet --project "$MCP_DIR" python -m warpium_mac_mcp.vmtools kcpassword)" ||
      die "could not encode the auto-login password (is uv installed on the host?)"
  fi
  # The values reach the guest on stdin, not on the host's command lines.
  # Inside the throwaway guest, dscl/security get them as arguments briefly.
  {
    printf 'OLD=%q\nNEW=%q\nKC=%q\nKC_LEVEL=%q\n' "$GUEST_DEFAULT_PASSWORD" "$new" "$kc" "$kc_level"
    guest_harden_script
  } | guest_ssh "$ip" 'bash -s' || {
    [ -z "$pending" ] || warn "the new password is kept in $PENDING_PASSWORD_FILE; re-running continues with it"
    die "guest hardening failed (see above); re-run setup.sh"
  }
  if [ -n "$new" ] && [ -n "$pending" ]; then
    mv -f "$PENDING_PASSWORD_FILE" "$ADMIN_PASSWORD_FILE"
    chmod 600 "$ADMIN_PASSWORD_FILE"
    ok "admin password replaced (saved in $ADMIN_PASSWORD_FILE, mode 600); auto-login and keychain updated"
  fi
  ok "SSH is key-only for $GUEST_USER (sshd -T checked); Screen Sharing and Remote Management are disabled"

  # Check from the host, over the same network the LAN would use.
  require_port_closed "$ip" "$SCREEN_SHARING_PORT" "Screen Sharing"
  require_port_closed "$ip" "$ARD_PORT" "Remote Management"
  case "$state" in
    saved) pw="$saved" ;;
    *) pw="${new:-$GUEST_DEFAULT_PASSWORD}" ;;
  esac
  if out="$(ssh_with_password "$ip" "$pw" true 2>&1 </dev/null)"; then
    die "SSH still accepts the admin password from the host; check /etc/ssh/sshd_config(.d) in the guest"
  fi
  methods="$(printf '%s\n' "$out" | sed -n 's/.*Permission denied (\([^)]*\)).*/\1/p' | head -1)"
  case "$methods" in
    "") die "could not check that SSH refuses passwords: ${out:-no output}" ;;
    *password* | *keyboard-interactive*) die "SSH still offers '$methods' logins; check /etc/ssh/sshd_config(.d) in the guest" ;;
  esac
  ok "SSH refuses password logins from the host (server offers: $methods)"

  mkdir -p "$STATE_DIR"
  {
    echo "version=2"
    echo "ssh=key-only"
    echo "screen_sharing=off"
    if [ -s "$ADMIN_PASSWORD_FILE" ] && { [ -n "$new" ] || [ "$state" = saved ]; }; then
      echo "password=changed"
    else
      echo "password=$state" # default, or unknown
    fi
    echo "date=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  } >"$HARDENED_MARKER"
}

# --- network mode -----------------------------------------------------------
# nat:     vmnet shared NAT (Tart's default), VM in 192.168.64.0/24 unless moved.
# bridged: the VM joins the LAN of a physical interface and gets its address
#          from the LAN router; avoids a VPN route over the vmnet subnet, but
#          the guest is reachable from the LAN (README "Network modes").
# softnet: Tart's Softnet isolation; still on vmnet shared NAT, so it does
#          not avoid a route conflict.
vm_net_mode() {
  local m="${WARPIUM_VM_NET:-}"
  if [ -z "$m" ] && [ -s "$NET_MODE_FILE" ]; then
    m="$(head -1 "$NET_MODE_FILE")"
  fi
  m="${m:-nat}"
  case "$m" in
    nat | bridged | softnet) echo "$m" ;;
    *) die "unknown network mode '$m' (WARPIUM_VM_NET or $NET_MODE_FILE): use nat, bridged or softnet" ;;
  esac
}

# Saves WARPIUM_VM_NET and WARPIUM_VM_BRIDGE_IF, when given, for later runs
# (start.sh, provision.sh, ...). Call it after start_vm_headless accepted them.
save_net_mode() {
  local m
  mkdir -p "$STATE_DIR"
  if [ -n "${WARPIUM_VM_NET:-}" ]; then
    m="$(vm_net_mode)"
    if [ "$(cat "$NET_MODE_FILE" 2>/dev/null || true)" != "$m" ]; then
      printf '%s\n' "$m" >"$NET_MODE_FILE"
      ok "network mode $m saved in $NET_MODE_FILE"
    fi
  fi
  if [ -n "${WARPIUM_VM_BRIDGE_IF:-}" ] &&
    [ "$(cat "$BRIDGE_IF_FILE" 2>/dev/null || true)" != "$WARPIUM_VM_BRIDGE_IF" ]; then
    printf '%s\n' "$WARPIUM_VM_BRIDGE_IF" >"$BRIDGE_IF_FILE"
    ok "bridge interface $WARPIUM_VM_BRIDGE_IF saved in $BRIDGE_IF_FILE"
  fi
}

# The column of the Netif field in `netstat -rn -f inet` (it moved between
# macOS versions), then the interfaces of all default routes, in table order.
default_route_interfaces() {
  netstat -rn -f inet 2>/dev/null | awk '
    $1 == "Destination" { for (i = 1; i <= NF; i++) if ($i == "Netif") col = i; next }
    col && $1 == "default" { print $col }'
}

# The host interface to bridge the VM to: WARPIUM_VM_BRIDGE_IF, else the saved
# one, else the one the running VM uses, else the interface of the default
# route, else (when a VPN owns the default route) the first physical en*
# interface with a default route.
bridge_interface() {
  local i
  if [ -n "${WARPIUM_VM_BRIDGE_IF:-}" ]; then
    echo "$WARPIUM_VM_BRIDGE_IF"
    return 0
  fi
  if [ -s "$BRIDGE_IF_FILE" ]; then
    head -1 "$BRIDGE_IF_FILE"
    return 0
  fi
  i="$(vm_run_cmdline "$VM_NAME" | tr ' ' '\n' | sed -n 's/^--net-bridged=//p' | head -1)"
  if [ -n "$i" ]; then
    echo "$i"
    return 0
  fi
  i="$(route -n get default 2>/dev/null | awk '$1 == "interface:" { print $2; exit }')"
  case "$i" in
    en[0-9]*)
      echo "$i"
      return 0
      ;;
  esac
  i="$(default_route_interfaces | grep -E '^en[0-9]+$' | head -1 || true)"
  [ -n "$i" ] || die "bridged mode: no physical (en*) interface has a default route; set WARPIUM_VM_BRIDGE_IF (see 'tart run --net-bridged=list')"
  echo "$i"
}

# Sets NET_ARGS (tart run flags) and NET_DESC for the current mode.
NET_ARGS=()
NET_DESC=""
resolve_net_args() {
  local m bif
  m="$(vm_net_mode)"
  case "$m" in
    nat)
      NET_ARGS=()
      NET_DESC="NAT (vmnet shared)"
      ;;
    bridged)
      bif="$(bridge_interface)"
      NET_ARGS=("--net-bridged=$bif")
      NET_DESC="bridged to $bif (the guest is on your LAN)"
      ;;
    softnet)
      NET_ARGS=(--net-softnet)
      NET_DESC="Softnet (vmnet shared NAT with isolation)"
      ;;
  esac
}

# Softnet must be setuid root; Tart only asks for sudo on a terminal, and the
# headless start has none.
require_softnet_ready() {
  local sn
  sn="$(command -v softnet || true)"
  [ -n "$sn" ] || die "softnet is not installed (brew install openai/tools/softnet; needs macOS 26)"
  sn="$(realpath "$sn")"
  if [ "$(stat -f '%u' "$sn" 2>/dev/null)" != "0" ] || [ ! -u "$sn" ]; then
    die "softnet must be setuid root for the headless start: sudo chown root '$sn' && sudo chmod u+s '$sn'"
  fi
}

# --- vmnet route check ------------------------------------------------------
is_ipv4() {
  local IFS=. a b c d extra
  [[ "$1" =~ ^[0-9]{1,3}\.[0-9]{1,3}\.[0-9]{1,3}\.[0-9]{1,3}$ ]] || return 1
  read -r a b c d extra <<<"$1"
  [ "$a" -le 255 ] && [ "$b" -le 255 ] && [ "$c" -le 255 ] && [ "$d" -le 255 ]
}

# routes_overlapping <ipv4> <len> <min_route_len>: prints the IPv4 routing table
# lines (netstat -rn -f inet) whose destination overlaps <ipv4>/<len> and whose
# own prefix is at least <min_route_len> bits long ("default" is /0). netstat
# abbreviates destinations: "192.168.64" is /24, "10.8/14" has an explicit length.
routes_overlapping() {
  netstat -rn -f inet 2>/dev/null | awk -v ip="$1" -v len="$2" -v minlen="$3" '
    function toint(a,   p, n, i, v) {
      n = split(a, p, ".")
      v = 0
      for (i = 1; i <= 4; i++) v = v * 256 + (i <= n ? p[i] + 0 : 0)
      return v
    }
    $1 ~ /^[0-9]+(\.[0-9]+)*(\/[0-9]+)?$/ {
      d = $1
      l = -1
      if (index(d, "/")) { split(d, s, "/"); d = s[1]; l = s[2] + 0 }
      if (l < 0) l = 8 * split(d, o, ".")
      if (l < minlen) next
      m = (l < len) ? l : len
      if (int(toint(d) / 2 ^ (32 - m)) == int(toint(ip) / 2 ^ (32 - m))) print
    }'
}

# Drops routing table lines that belong to a vmnet bridge.
without_bridge_routes() {
  awk '{ for (i = 2; i <= NF; i++) if ($i ~ /^bridge[0-9]+$/) next; print }'
}

# vmnet_setting <key> <default>: the value in the vmnet plist file itself.
vmnet_setting() {
  plutil -extract "$1" raw -o - "$VMNET_PLIST" 2>/dev/null || echo "$2"
}

# The /24 network part ("192.168.251") of a dotted address.
net24() {
  echo "${1%.*}"
}

# check_vm_route <ip>: fails fast when the VM's address is not routed where
# the network mode puts it: a vmnet bridge (nat/softnet) or the bridged LAN
# interface. Typical cause: a VPN or LAN route covering the vmnet subnet. The
# host's own route for the VM on bridge100 is interface-scoped, so an unscoped
# route on en0/utun* wins and nothing reaches the guest.
check_vm_route() {
  local ip="$1" mode out iface want target current conflicts
  if [ "${WARPIUM_VM_SKIP_ROUTE_CHECK:-0}" = "1" ]; then
    warn "route check skipped (WARPIUM_VM_SKIP_ROUTE_CHECK=1)"
    return 0
  fi
  mode="$(vm_net_mode)"
  if [ "$mode" = "bridged" ]; then
    want="$(bridge_interface)"
    target="$want"
  else
    want="bridge*"
    target="the vmnet bridge"
  fi
  out="$(route -n get "$ip" 2>&1 || true)"
  iface="$(printf '%s\n' "$out" | awk '$1 == "interface:" { print $2; exit }')"
  # shellcheck disable=SC2254  # $want is a glob on purpose
  case "$iface" in
    $want)
      ok "$ip is routed via $iface"
      return 0
      ;;
  esac
  conflicts="$(routes_overlapping "$ip" 32 1 | without_bridge_routes)"
  {
    printf '%serror:%s the VM at %s is not reachable: the host routes it via %s, not %s.\n' \
      "$_R" "$_N" "$ip" "${iface:-no interface}" "$target"
    echo
    echo "  route -n get $ip:"
    printf '%s\n' "$out" | grep -E '^ *(destination|mask|gateway|interface|flags):' | sed 's/^/    /' || true
    echo "  Conflicting routes (netstat -rn -f inet):"
    if [ -n "$conflicts" ]; then
      printf '%s\n' "$conflicts" | sed 's/^/    /'
    else
      echo "    (none found; check 'netstat -rn -f inet' yourself)"
    fi
    echo
    if [ "$mode" = "bridged" ]; then
      cat <<EOF
  Likely cause: a VPN routes your LAN ($ip) through the tunnel. Allow "local network
  access" in the VPN client, pick another interface with WARPIUM_VM_BRIDGE_IF=<if>
  (list: tart run --net-bridged=list), or go back to NAT: WARPIUM_VM_NET=nat vm/setup.sh
EOF
    else
      current="$(vmnet_setting Shared_Net_Address "$VMNET_DEFAULT_ADDRESS")"
      echo "  Likely cause: a VPN or LAN route overlaps the vmnet subnet (vmnet plist: $current/24)."
      if [ -e "$VMNET_STRAY_PLIST" ]; then
        echo "  Found $VMNET_STRAY_PLIST: an earlier 'defaults write' used the path with"
        echo "  .plist, so the setting went to that file, which vmnet ignores. Fix B below writes"
        echo "  the right file; vm/fix-subnet.sh also removes the stray one."
      fi
      if [ "$(net24 "$current")" != "$(net24 "$ip")" ]; then
        echo "  The plist already names another subnet, but the VM got $ip: macOS did not apply"
        echo "  the change (it may not honour it for Tart's NAT, or 'tart ip' read a stale lease"
        echo "  in /var/db/dhcpd_leases). Use fix A."
      fi
      cat <<EOF

  Fix A (recommended): bridged networking. The VM gets an address from your LAN router,
  outside the VPN route. The guest is then reachable from the LAN (setup.sh makes SSH
  key-only and turns Screen Sharing off; the MCP server is reachable only through the SSH
  forward; see README "Network modes").
       vm/stop.sh
       WARPIUM_VM_NET=bridged vm/setup.sh     # saved; vm/start.sh reuses it

EOF
      subnet_fix_help
    fi
  } >&2
  exit 1
}

# Fix B of the route-conflict error: move the vmnet shared subnet.
subnet_fix_help() {
  local suggest="$VM_SUGGEST_SUBNET" net taken
  if ! is_ipv4 "$suggest"; then
    echo "  Fix B: move the vmnet subnet. WARPIUM_VM_SUGGEST_SUBNET='$suggest' is not an IPv4"
    echo "  address; pick a free one like 192.168.251.1."
    return 0
  fi
  net="$(net24 "$suggest")"
  echo "  Fix B: move the vmnet shared subnet to a free range (macOS 26 may not honour it;"
  echo "  vm/fix-subnet.sh checks)."
  taken="$(routes_overlapping "$suggest" 24 24 | without_bridge_routes)"
  if [ -n "$taken" ]; then
    echo "  The suggested $net.0/24 is taken too:"
    printf '%s\n' "$taken" | sed 's/^/    /'
    echo "  Pick another free range and re-run with WARPIUM_VM_SUGGEST_SUBNET=<a.b.c.1>."
  fi
  cat <<EOF
    1. Stop all VMs:  vm/stop.sh   (and any other VM in 'tart list')
    2. Check that $net.0/24 is free (must print nothing):
         netstat -rn -f inet | grep -E '^$(printf '%s' "$net" | sed 's/\./\\./g')([./ ]|\$)'
    3. Move the subnet:
         sudo defaults write $VMNET_DOMAIN Shared_Net_Address -string $suggest
         sudo defaults write $VMNET_DOMAIN Shared_Net_Mask -string $VMNET_MASK
       (no .plist at the end: defaults adds it). Check the file itself:
         plutil -p $VMNET_PLIST
    4. Re-run vm/setup.sh (or vm/start.sh).
    vm/fix-subnet.sh $suggest does steps 1-3, restarts the VM and checks that the new
    subnet took effect (it asks before changing anything).
EOF
}

# The command lines of all running `tart run ... <name>` processes.
tart_run_cmdlines() {
  ps -axo command= 2>/dev/null | awk '$1 ~ /(^|\/)tart$/ && $2 == "run"' || true
}

# Names of all running Tart VMs, one per line.
running_tart_vms() {
  tart_run_cmdlines | awk '{ print $NF }' | sort -u
}

# The `tart run` command line of a running VM ("" if none).
vm_run_cmdline() {
  tart_run_cmdlines | awk -v n="$1" '$NF == n' | head -1
}

start_vm_headless() {
  local cmd want
  mkdir -p "$STATE_DIR"
  resolve_net_args
  if vm_running "$VM_NAME"; then
    # A VM started in another network mode keeps it until it restarts.
    cmd="$(vm_run_cmdline "$VM_NAME")"
    want="${NET_ARGS[*]:-}"
    case "$cmd" in
      *--net-*) [ -n "$want" ] && [[ "$cmd" == *" $want "* ]] ;;
      *) [ -z "$want" ] ;;
    esac || die "VM $VM_NAME is running with other network settings ('$cmd'); run vm/stop.sh and re-run to use $NET_DESC"
    if vnc_requested && [[ "$cmd" != *" --vnc-experimental "* ]]; then
      die "VM $VM_NAME is running without VNC; run vm/stop.sh, then WARPIUM_VM_VNC=1 vm/start.sh"
    fi
    ok "VM $VM_NAME is already running ($NET_DESC)"
    return 0
  fi
  [ "$(vm_net_mode)" != "softnet" ] || require_softnet_ready
  local vnc_args=()
  if vnc_requested; then
    vnc_args=(--vnc-experimental)
    # The URL tart prints for this run is the first one after this mark.
    wc -l <"$VM_LOG" 2>/dev/null | tr -d ' ' >"$VNC_LOG_MARK_FILE" || echo 0 >"$VNC_LOG_MARK_FILE"
  fi
  step "Starting VM $VM_NAME headless, network $NET_DESC${vnc_args[*]:+, VNC on} (log: $VM_LOG)"
  # --vnc-experimental: Virtualization.framework's own VNC server on the host (no
  # guest service). With --no-graphics, tart prints "VNC server is running at
  # vnc://..." to stdout (the log) instead of opening it, and shows no window.
  nohup tart run --no-graphics ${vnc_args[@]+"${vnc_args[@]}"} ${NET_ARGS[@]+"${NET_ARGS[@]}"} \
    "$VM_NAME" >>"$VM_LOG" 2>&1 </dev/null &
  disown || true
  sleep 3
  vm_running "$VM_NAME" || die "tart run exited immediately; see $VM_LOG"
}

vnc_requested() { [ "${WARPIUM_VM_VNC:-0}" = "1" ]; }

# The vnc:// URL tart printed for the current run (empty if none yet).
vnc_url() {
  local mark=0
  [ -s "$VNC_LOG_MARK_FILE" ] && mark="$(cat "$VNC_LOG_MARK_FILE")"
  [ -f "$VM_LOG" ] || return 0
  tail -n "+$((mark + 1))" "$VM_LOG" | grep -o 'vnc://[^ ]*' | tail -1 | sed 's/\.\.\.$//' || true
}

# Waits up to $1 seconds for vnc_url.
wait_for_vnc_url() {
  local url
  for _ in $(seq 1 "$1"); do
    url="$(vnc_url)"
    if [ -n "$url" ]; then
      echo "$url"
      return 0
    fi
    sleep 1
  done
  return 1
}

# Stops the VM and waits (up to 30 s) for its `tart run` process to exit.
stop_vm() {
  if vm_running "$VM_NAME"; then
    step "Stopping VM $VM_NAME"
    tart stop --timeout 60 "$VM_NAME" || warn "tart stop failed; check 'tart list'"
    for _ in $(seq 1 30); do
      vm_running "$VM_NAME" || return 0
      sleep 1
    done
    warn "VM $VM_NAME still runs 30 s after 'tart stop'"
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
