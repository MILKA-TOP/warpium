#!/bin/bash
# One-time (idempotent) host setup for the isolated Warpium test VM.
#
#   1. installs Tart (brew install openai/tools/tart) unless tart is on PATH
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
# Picks the guest image and rejects hosts older than macOS 15 before
# anything is installed.
IMAGE="${WARPIUM_VM_IMAGE:-$(default_image)}"

# --- 1. Tart ---------------------------------------------------------------
step "Checking Homebrew"
BREW=()
BREW_PREFIX=""
if command -v brew >/dev/null 2>&1; then
  if ! BREW_PREFIX="$(brew --prefix 2>/dev/null)"; then
    BREW_PREFIX="unknown"
    warn "'brew --prefix' failed; Homebrew looks broken (try 'brew doctor')"
  else
    BREW=(brew)
  fi
  case "$BREW_PREFIX" in
    /opt/homebrew) ok "$(command -v brew), prefix $BREW_PREFIX (arm64, native)" ;;
    /usr/local)
      echo "    $(command -v brew), prefix $BREW_PREFIX (x86_64: an Intel/Rosetta install)"
      warn "this Mac is Apple Silicon but Homebrew is the Intel one in /usr/local. Continuing;"
      warn "brew installs run via 'arch -x86_64 brew' (Homebrew refuses /usr/local installs"
      warn "natively on arm64). Tart is an arm64 binary and runs natively either way. A native"
      warn "Homebrew in /opt/homebrew is recommended: https://brew.sh"
      BREW=(arch -x86_64 brew)
      ;;
    unknown) ;;
    *) ok "$(command -v brew), prefix $BREW_PREFIX (custom prefix, architecture unknown)" ;;
  esac
else
  echo "    Homebrew not found (only needed to install Tart): https://brew.sh"
fi

step "Checking Tart"
if ! command -v tart >/dev/null 2>&1; then
  [ "${#BREW[@]}" -gt 0 ] || die "Tart is not installed and no working Homebrew was found: install Homebrew from https://brew.sh (or Tart manually: https://tart.run/quick-start/) and re-run vm/setup.sh"
  if [ "${BREW[0]}" = "arch" ] && ! arch -x86_64 /usr/bin/true 2>/dev/null; then
    die "Rosetta 2 is not installed, so the /usr/local Homebrew can't run. Install it with 'softwareupdate --install-rosetta' or install a native Homebrew in /opt/homebrew (https://brew.sh)"
  fi
  # Tart moved from cirruslabs/cli to openai/tools. Its dependency softnet
  # (only used by 'tart run --net-softnet') requires macOS 26, so skip it on
  # older hosts.
  BREW_ARGS=()
  if [ "$(host_macos_major)" -lt 26 ]; then
    warn "openai/tools/softnet requires macOS 26; installing Tart without it (only 'tart run --net-softnet' needs it)"
    BREW_ARGS=(--ignore-dependencies)
  fi
  step "Installing Tart: ${BREW[*]} install ${BREW_ARGS[*]:+${BREW_ARGS[*]} }openai/tools/tart"
  "${BREW[@]}" install ${BREW_ARGS[@]+"${BREW_ARGS[@]}"} openai/tools/tart ||
    die "'brew install openai/tools/tart' failed (see the output above). Manual install: https://tart.run/quick-start/"
  hash -r
  if ! command -v tart >/dev/null 2>&1; then
    die "Tart was installed but 'tart' is not on PATH; add $BREW_PREFIX/bin to PATH and re-run vm/setup.sh"
  fi
fi
if ! TART_VERSION="$(tart --version 2>/dev/null)"; then
  TART_ERR="$(tart --version 2>&1 | head -5)" || true
  die "'$(command -v tart) --version' failed: ${TART_ERR:-no output}. Reinstall Tart ('brew reinstall tart', or uninstall it and re-run vm/setup.sh) or remove the broken binary"
fi
ok "Tart ${TART_VERSION%%$'\n'*} at $(command -v tart)"

# --- 2. VM -----------------------------------------------------------------
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

step "Waiting for SSH on $IP:22"
for _ in $(seq 1 60); do
  nc -z -G 2 "$IP" 22 >/dev/null 2>&1 && break
  sleep 2
done
nc -z -G 2 "$IP" 22 >/dev/null 2>&1 || die "sshd in the VM is not reachable after 120 s; see $VM_LOG, then re-run setup.sh"

step "Checking key-based SSH"
if wait_for_key_ssh "$IP" 20; then
  ok "key login works"
else
  echo "    installing the key with the image's default password (first run only)"
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
    die "could not log in with the key or with admin/admin. If the guest was still booting, just re-run setup.sh. If this VM was set up with another key, restore $SSH_KEY; as a last resort recreate the VM: tart delete $VM_NAME && vm/setup.sh"
  fi
  rm -f "$askpass"
  wait_for_key_ssh "$IP" 10 || die "key installed but key login still fails"
  ok "key installed"
fi

step "Hardening guest logins"
hardened="$(guest_ssh "$IP" 'if test -f ~/.warpium-hardened; then echo yes; else echo no; fi')" ||
  die "SSH to the guest failed while checking the hardening state; re-run setup.sh"
if [ "$hardened" = "yes" ]; then
  ok "already done (password replaced, SSH password logins off)"
else
  # Reuse a password saved by an earlier, interrupted run: the guest may
  # already use it, and generating a new one would lose it.
  if [ -s "$ADMIN_PASSWORD_FILE" ]; then
    NEW_PASSWORD="$(head -1 "$ADMIN_PASSWORD_FILE")"
    echo "    resuming with the password saved in $ADMIN_PASSWORD_FILE"
  else
    NEW_PASSWORD="$(openssl rand -hex 16)"
    # Save first: if anything below fails you can still log in at the VM's screen.
    (umask 077 && printf '%s\n' "$NEW_PASSWORD" >"$ADMIN_PASSWORD_FILE")
  fi
  KC_B64="$(printf '%s\n' "$NEW_PASSWORD" | uv run --quiet --project "$MCP_DIR" python -m warpium_mac_mcp.vmtools kcpassword)" ||
    die "could not encode the auto-login password (is uv installed on the host?)"
  # The values reach the guest on stdin, not on the host's command lines.
  # Inside the throwaway guest, dscl/security get them as arguments briefly.
  {
    printf 'OLD=%q\nNEW=%q\nKC=%q\n' "$GUEST_DEFAULT_PASSWORD" "$NEW_PASSWORD" "$KC_B64"
    cat <<'GUEST'
set -euo pipefail
# Passwordless sudo for admin is preconfigured in the cirruslabs images.
sudo -n true || { echo "passwordless sudo is not available in the guest" >&2; exit 1; }
sudo dscl . -passwd "/Users/$USER" "$NEW"
# Keep the login keychain unlockable at auto-login (no "update keychain" dialogs).
# After an interrupted earlier run it may already use NEW, which is fine.
KC_DB="$HOME/Library/Keychains/login.keychain-db"
if [ -f "$KC_DB" ]; then
  if ! security set-keychain-password -o "$OLD" -p "$NEW" "$KC_DB" 2>/dev/null; then
    if security unlock-keychain -p "$NEW" "$KC_DB" 2>/dev/null; then
      echo "    login keychain already uses the new password"
    else
      echo "warning: could not update the login keychain password; if a keychain dialog" >&2
      echo "         appears at login, delete ~/Library/Keychains/login.keychain-db in the guest" >&2
    fi
  fi
fi
# Auto-login reads the password from /etc/kcpassword: rewrite it for the new one.
printf '%s' "$KC" | base64 -D | sudo tee /etc/kcpassword >/dev/null
sudo chmod 600 /etc/kcpassword
sudo chown root:wheel /etc/kcpassword
sudo defaults write /Library/Preferences/com.apple.loginwindow autoLoginUser "$USER"
# Key-only SSH from now on. sshd keeps the first value it reads, so the file
# must sort before macOS's own 100-macos.conf.
printf 'PasswordAuthentication no\nKbdInteractiveAuthentication no\nPermitRootLogin no\n' |
  sudo tee /etc/ssh/sshd_config.d/000-warpium.conf >/dev/null
sudo rm -f /etc/ssh/sshd_config.d/100-warpium.conf
eff="$(sudo /usr/sbin/sshd -T 2>/dev/null | tr 'A-Z' 'a-z')"
for opt in "passwordauthentication no" "kbdinteractiveauthentication no"; do
  if ! printf '%s\n' "$eff" | grep -qx "$opt"; then
    echo "error: sshd's effective config doesn't have '$opt'; check /etc/ssh/sshd_config(.d)" >&2
    exit 1
  fi
done
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
