#!/bin/bash
# Installs or updates the MCP server inside the running VM. Safe to re-run;
# run it after `git pull` to push the new server code into the guest.
#
#   - installs Homebrew (if missing), uv, git and gh in the guest
#   - copies this checkout's tools/mac-mcp into the guest (~/warpium-mac-mcp)
#     with rsync over SSH (no clone or credentials inside the guest)
#   - installs a launchd agent in the guest's GUI session that opens the
#     server in Terminal.app at login (Terminal holds the Screen Recording /
#     Accessibility grants, see vm/grant-permissions.sh)
#   - (re)starts the server and waits until it answers
#
# WARPIUM_VM_REUSE_HOST_TOKEN=1 copies the host's bearer token
# (~/.config/warpium-mac-mcp/token) into the guest, so an existing
# `claude mcp add warpium-mac ...` keeps working unchanged.
source "$(dirname "$0")/common.sh"

require_macos_arm64
require_tart
require_key
vm_running "$VM_NAME" || die "VM $VM_NAME is not running; run vm/start.sh (or vm/setup.sh)"
IP="$(vm_ip 60)" || die "no IP for $VM_NAME; see $VM_LOG"
wait_for_key_ssh "$IP" 30 || die "key SSH to $GUEST_USER@$IP fails; run vm/setup.sh"

step "Installing guest tools (Homebrew, uv, git, gh); the first run takes a few minutes"
{
  printf 'GUEST_PATH=%q\n' "$GUEST_PATH"
  cat <<'GUEST'
set -euo pipefail
export PATH="$GUEST_PATH" HOMEBREW_NO_AUTO_UPDATE=1 HOMEBREW_NO_INSTALL_CLEANUP=1 HOMEBREW_NO_ENV_HINTS=1
if ! command -v brew >/dev/null 2>&1; then
  echo "    installing Homebrew (also installs the Command Line Tools)"
  NONINTERACTIVE=1 /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"
fi
for f in uv git gh; do
  if brew list --formula "$f" >/dev/null 2>&1; then
    echo "    $f: installed"
  else
    brew install --quiet "$f"
  fi
done
grep -q 'brew shellenv' ~/.zprofile 2>/dev/null || echo 'eval "$(/opt/homebrew/bin/brew shellenv)"' >> ~/.zprofile
GUEST
} | guest_ssh "$IP" 'bash -s' || die "installing guest tools failed (output above)"
ok "guest tools ready"

step "Copying $MCP_DIR to the guest (~/$GUEST_DIR)"
rsync -a --delete \
  --exclude '.venv/' --exclude '__pycache__/' --exclude '.pytest_cache/' \
  --exclude '.ruff_cache/' --exclude '.mcp.json' --exclude '*.token' \
  -e "$(rsync_ssh_cmd)" "$MCP_DIR/" "$GUEST_USER@$IP:$GUEST_DIR/" ||
  die "rsync failed"
ok "copied"

if [ "${WARPIUM_VM_REUSE_HOST_TOKEN:-0}" = "1" ]; then
  step "Copying the host's bearer token into the guest"
  [ -f "$CONF_DIR/token" ] || die "no host token at $CONF_DIR/token"
  guest_ssh "$IP" 'umask 077; mkdir -p ~/.config/warpium-mac-mcp && chmod 700 ~/.config/warpium-mac-mcp && cat > ~/.config/warpium-mac-mcp/token' \
    <"$CONF_DIR/token" || die "copying the token failed"
  ok "guest uses the same token as the host server"
fi

step "Installing the server's launchd agent and starting the server"
{
  printf 'GUEST_PATH=%q\nGUEST_DIR=%q\nLABEL=%q\nPORT=%q\n' \
    "$GUEST_PATH" "$GUEST_DIR" "$GUEST_AGENT_LABEL" "$MCP_PORT"
  cat <<'GUEST'
set -euo pipefail
export PATH="$GUEST_PATH"
cd "$HOME/$GUEST_DIR"
uv sync --frozen --quiet
mkdir -p "$HOME/.config/warpium-mac-mcp" "$HOME/Library/LaunchAgents"
chmod 700 "$HOME/.config/warpium-mac-mcp"

# Terminal.app runs the server, so the TCC grants belong to Terminal (a stable
# bundle id) rather than to a uv-managed python path that changes on upgrades.
cat > "$HOME/$GUEST_DIR/serve.command" <<EOF
#!/bin/zsh
# Started at login by ~/Library/LaunchAgents/$LABEL.plist. Restarts any old copy.
export PATH="$GUEST_PATH"
pkill -f "warpium-mac-mcp serve --host 127.0.0.1 --port $PORT" 2>/dev/null && sleep 1
cd "\$HOME/$GUEST_DIR"
exec uv run --frozen warpium-mac-mcp serve --host 127.0.0.1 --port $PORT 2>&1 | tee -a "\$HOME/.config/warpium-mac-mcp/server.log"
EOF
chmod 755 "$HOME/$GUEST_DIR/serve.command"

PLIST="$HOME/Library/LaunchAgents/$LABEL.plist"
cat > "$PLIST" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>$LABEL</string>
  <key>ProgramArguments</key>
  <array>
    <string>/usr/bin/open</string><string>-a</string><string>Terminal</string>
    <string>$HOME/$GUEST_DIR/serve.command</string>
  </array>
  <key>RunAtLoad</key><true/>
  <key>LimitLoadToSessionType</key><string>Aqua</string>
</dict>
</plist>
EOF

uid="$(id -u)"
console_user="$(stat -f %Su /dev/console)"
if [ "$console_user" != "$USER" ]; then
  echo "error: no GUI session for $USER (console user: $console_user). Auto-login should" >&2
  echo "       provide one; reboot the VM (vm/stop.sh && vm/start.sh) and re-run." >&2
  exit 1
fi
launchctl bootout "gui/$uid/$LABEL" 2>/dev/null || true
launchctl bootstrap "gui/$uid" "$PLIST"
launchctl kickstart -k "gui/$uid/$LABEL"

for i in $(seq 1 90); do
  code="$(curl -s -o /dev/null -w '%{http_code}' -X POST "http://127.0.0.1:$PORT/mcp" || true)"
  if [ "$code" = "401" ]; then
    echo "    server answers on 127.0.0.1:$PORT (HTTP 401 without token, as expected)"
    exit 0
  fi
  sleep 2
done
echo "error: server did not answer within 180 s; see ~/.config/warpium-mac-mcp/server.log in the guest" >&2
tail -20 "$HOME/.config/warpium-mac-mcp/server.log" >&2 || true
exit 1
GUEST
} | guest_ssh "$IP" 'bash -s' || die "starting the guest server failed (output above)"
ok "server running in the guest"

step "Guest doctor"
guest_ssh "$IP" "export PATH='$GUEST_PATH'; cd ~/$GUEST_DIR && uv run --frozen warpium-mac-mcp doctor --no-port-check" ||
  warn "doctor reports missing requirements (see FAIL lines). Permissions: vm/grant-permissions.sh"
