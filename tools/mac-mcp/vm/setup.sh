#!/bin/bash
# One-time (idempotent) host setup for the isolated Warpium test VM.
#
#   1. installs Tart (brew install cirruslabs/cli/tart)
#   2. clones a vanilla macOS image into the VM "warpium-test" and sets CPU/RAM
#   3. creates a dedicated SSH key, installs it in the guest, replaces the
#      image's default admin/admin password and disables SSH password logins
#   4. runs vm/provision.sh (Homebrew, uv, git, gh, the MCP server)
#
# Re-running is safe: finished steps are detected and skipped. Settings: see
# vm/common.sh (WARPIUM_VM_CPU, WARPIUM_VM_MEMORY_MB, WARPIUM_VM_IMAGE, ...).
source "$(dirname "$0")/common.sh"

require_macos_arm64
mkdir -p "$CONF_DIR" "$STATE_DIR"
chmod 700 "$CONF_DIR"

# --- 1. Tart ---------------------------------------------------------------
step "Checking Tart"
if ! command -v tart >/dev/null 2>&1; then
  command -v brew >/dev/null 2>&1 || die "Homebrew is required: https://brew.sh"
  brew install cirruslabs/cli/tart
fi
ok "$(tart --version 2>/dev/null | head -1 || echo tart) at $(command -v tart)"

# --- 2. VM -----------------------------------------------------------------
IMAGE="${WARPIUM_VM_IMAGE:-$(default_image)}"
if vm_exists "$VM_NAME"; then
  ok "VM $VM_NAME exists (delete it with 'tart delete $VM_NAME' to start over)"
else
  step "Cloning $IMAGE into $VM_NAME"
  echo "    Download: ~24 GB compressed (cached in ~/.tart/cache). The VM disk is a 50 GB"
  echo "    sparse file that uses ~25-30 GB at first. Plan for ~60 GB free disk; clones made"
  echo "    with vm/snapshot.sh / vm/reset.sh share blocks (APFS copy-on-write)."
  df -h "$HOME" | tail -1 | awk '{print "    free on your home volume: " $4}'
  tart clone "$IMAGE" "$VM_NAME"
  # A new image has a new SSH host key; forget the old one for this VM name.
  [ -f "$KNOWN_HOSTS" ] && ssh-keygen -R "$VM_NAME" -f "$KNOWN_HOSTS" >/dev/null 2>&1 || true
  ok "cloned"
fi

if vm_running "$VM_NAME"; then
  warn "VM is running; CPU/RAM/disk settings apply on the next setup run while it is stopped"
else
  step "Setting resources: $VM_CPU vCPU, $VM_MEMORY_MB MiB RAM${VM_DISK_GB:+, disk ${VM_DISK_GB} GB}"
  tart set "$VM_NAME" --cpu "$VM_CPU" --memory "$VM_MEMORY_MB"
  if [ -n "$VM_DISK_GB" ]; then
    tart set "$VM_NAME" --disk-size "$VM_DISK_GB"
  fi
  ok "set"
fi

# --- 3. SSH key, password, sshd -------------------------------------------
if [ ! -f "$SSH_KEY" ]; then
  step "Creating dedicated SSH key $SSH_KEY"
  ssh-keygen -q -t ed25519 -N "" -C "warpium-mac-mcp-vm" -f "$SSH_KEY"
fi
chmod 600 "$SSH_KEY"

start_vm_headless
step "Waiting for the VM's IP address"
IP="$(vm_ip 180)" || die "no IP after 180 s; see $VM_LOG and 'tart list'"
ok "$VM_NAME is at $IP"

step "Checking key-based SSH"
if wait_for_key_ssh "$IP" 5; then
  ok "key login works"
else
  echo "    installing the key with the image's default password (first run only)"
  # Wait for sshd to come up before trying the password.
  for _ in $(seq 1 45); do
    nc -z -G 2 "$IP" 22 >/dev/null 2>&1 && break
    sleep 2
  done
  askpass="$(mktemp "$STATE_DIR/askpass.XXXXXX")"
  trap 'rm -f "$askpass"' EXIT
  printf '#!/bin/sh\necho %s\n' "$GUEST_DEFAULT_PASSWORD" >"$askpass"
  chmod 700 "$askpass"
  if ! SSH_ASKPASS="$askpass" SSH_ASKPASS_REQUIRE=force DISPLAY=none \
    ssh -o PubkeyAuthentication=no -o PreferredAuthentications=password,keyboard-interactive \
    -o UserKnownHostsFile="$KNOWN_HOSTS" -o HostKeyAlias="$VM_NAME" \
    -o StrictHostKeyChecking=accept-new -o ConnectTimeout=10 -o LogLevel=ERROR \
    -o NumberOfPasswordPrompts=1 "$GUEST_USER@$IP" \
    'umask 077; mkdir -p ~/.ssh; touch ~/.ssh/authorized_keys; k="$(cat)"; grep -qxF "$k" ~/.ssh/authorized_keys || echo "$k" >> ~/.ssh/authorized_keys' \
    <"$SSH_KEY.pub"; then
    die "could not log in with the key or with admin/admin. If this VM was set up with another key, restore $SSH_KEY or recreate the VM: tart delete $VM_NAME && vm/setup.sh"
  fi
  rm -f "$askpass"
  wait_for_key_ssh "$IP" 10 || die "key installed but key login still fails"
  ok "key installed"
fi

step "Hardening guest logins"
if guest_ssh "$IP" 'test -f ~/.warpium-hardened'; then
  ok "already done (password replaced, SSH password logins off)"
else
  NEW_PASSWORD="$(openssl rand -hex 16)"
  KC_B64="$(printf '%s\n' "$NEW_PASSWORD" | uv run --quiet --project "$MCP_DIR" python -m warpium_mac_mcp.vmtools kcpassword)" ||
    die "could not encode the auto-login password (is uv installed on the host?)"
  # Save first: if anything below fails you can still log in at the VM's screen.
  (umask 077 && printf '%s\n' "$NEW_PASSWORD" >"$ADMIN_PASSWORD_FILE")
  # Values travel on stdin, not in any process's argv.
  {
    printf 'OLD=%q\nNEW=%q\nKC=%q\n' "$GUEST_DEFAULT_PASSWORD" "$NEW_PASSWORD" "$KC_B64"
    cat <<'GUEST'
set -euo pipefail
# Passwordless sudo for admin is preconfigured in the cirruslabs images.
sudo -n true || { echo "passwordless sudo is not available in the guest" >&2; exit 1; }
sudo dscl . -passwd "/Users/$USER" "$NEW"
# Keep the login keychain unlockable at auto-login (no "update keychain" dialogs).
security set-keychain-password -o "$OLD" -p "$NEW" "$HOME/Library/Keychains/login.keychain-db" 2>/dev/null || true
# Auto-login reads the password from /etc/kcpassword: rewrite it for the new one.
printf '%s' "$KC" | base64 -D | sudo tee /etc/kcpassword >/dev/null
sudo chmod 600 /etc/kcpassword
sudo chown root:wheel /etc/kcpassword
sudo defaults write /Library/Preferences/com.apple.loginwindow autoLoginUser "$USER"
# Key-only SSH from now on.
printf 'PasswordAuthentication no\nKbdInteractiveAuthentication no\nPermitRootLogin no\n' |
  sudo tee /etc/ssh/sshd_config.d/100-warpium.conf >/dev/null
touch ~/.warpium-hardened
GUEST
  } | guest_ssh "$IP" 'bash -s' || die "guest hardening failed (password saved in $ADMIN_PASSWORD_FILE)"
  ok "admin password replaced (stored in $ADMIN_PASSWORD_FILE, mode 600); SSH is key-only"
fi

# --- 4. provision -----------------------------------------------------------
"$VM_DIR/provision.sh"

cat <<EOF

${_B}Setup finished.${_N} Next steps (README "VM setup"):
  1. One-time permissions:  vm/grant-permissions.sh
  2. Save a clean golden copy:  vm/snapshot.sh
  3. Start (VM + server + forward):  vm/start.sh
  4. Connect the container:  vm/print-client-config.sh
EOF
