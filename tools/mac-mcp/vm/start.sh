#!/bin/bash
# Starts the test VM headless, makes sure the MCP server runs inside it, and
# opens an SSH local forward host 127.0.0.1:17654 -> guest 127.0.0.1:17654, so
# the container keeps using http://host.docker.internal:17654/mcp unchanged.
# Idempotent: re-run it any time (e.g. after the forward died).
source "$(dirname "$0")/common.sh"

require_macos_arm64
require_tart
require_key
vm_exists "$VM_NAME" || die "VM $VM_NAME does not exist; run vm/setup.sh (or vm/reset.sh)"
mkdir -p "$STATE_DIR"
vm_net_mode >/dev/null # validates WARPIUM_VM_NET early

start_vm_headless
save_net_mode # only once the running VM matches the requested mode
step "Waiting for IP and SSH"
IP="$(vm_ip 180)" || die "no IP after 180 s; see $VM_LOG"
check_vm_route "$IP"
wait_for_key_ssh "$IP" 60 || die "SSH to $GUEST_USER@$IP fails (or run vm/setup.sh if the key was never installed).
$(ssh_unreachable_hint)"
ok "$VM_NAME is at $IP"

step "Checking the server inside the VM"
guest_code() {
  guest_ssh "$IP" "curl -s -o /dev/null -w '%{http_code}' -X POST http://127.0.0.1:$MCP_PORT/mcp" 2>/dev/null || true
}
code="$(guest_code)"
if [ "$code" != "401" ]; then
  echo "    not answering yet (HTTP '${code:-none}'); (re)starting the launchd agent"
  guest_ssh "$IP" "launchctl kickstart -k gui/\$(id -u)/$GUEST_AGENT_LABEL" ||
    die "could not start the guest agent; run vm/provision.sh"
  for _ in $(seq 1 60); do
    code="$(guest_code)"
    [ "$code" = "401" ] && break
    sleep 2
  done
  [ "$code" = "401" ] || die "guest server not answering; check ~/.config/warpium-mac-mcp/server.log in the guest (ssh: see README), or run vm/provision.sh"
fi
ok "guest server answers"

step "SSH forward 127.0.0.1:$MCP_PORT -> $VM_NAME:127.0.0.1:$MCP_PORT"
forward_cmd=""
[ -f "$FORWARD_PID_FILE" ] && forward_cmd="$(ps -p "$(cat "$FORWARD_PID_FILE")" -o command= 2>/dev/null || true)"
# Reuse only a forward to exactly this IP (the destination is the last argument).
if [ -n "$forward_cmd" ] && [ "${forward_cmd##* }" = "$GUEST_USER@$IP" ]; then
  ok "already running (pid $(cat "$FORWARD_PID_FILE"))"
else
  stop_forward
  listener="$(host_port_listener)"
  if [ -n "$listener" ]; then
    die "host port $MCP_PORT is taken ($listener). Is the host-mode server still running? Stop it (Ctrl+C) and re-run."
  fi
  # Binds the host's loopback only; never 0.0.0.0.
  nohup ssh -i "$SSH_KEY" -o IdentitiesOnly=yes -o UserKnownHostsFile="$KNOWN_HOSTS" \
    -o HostKeyAlias="$VM_NAME" -o StrictHostKeyChecking=accept-new -o BatchMode=yes \
    -o ExitOnForwardFailure=yes -o ServerAliveInterval=15 -o ServerAliveCountMax=3 \
    -N -L "127.0.0.1:$MCP_PORT:127.0.0.1:$MCP_PORT" "$GUEST_USER@$IP" \
    >>"$FORWARD_LOG" 2>&1 </dev/null &
  echo $! >"$FORWARD_PID_FILE"
  disown || true
  for _ in $(seq 1 20); do
    [ -n "$(host_port_listener)" ] && break
    sleep 0.5
  done
  [ -n "$(host_port_listener)" ] || die "forward did not come up; see $FORWARD_LOG"
  ok "forward running (pid $(cat "$FORWARD_PID_FILE"), log $FORWARD_LOG)"
fi

code="$(curl -s --noproxy '*' -o /dev/null -w '%{http_code}' -X POST "http://127.0.0.1:$MCP_PORT/mcp" || true)"
[ "$code" = "401" ] || die "host 127.0.0.1:$MCP_PORT answered '$code' instead of 401; see $FORWARD_LOG"
ok "host 127.0.0.1:$MCP_PORT reaches the guest server (HTTP 401 without token, as expected)"

cat <<EOF

${_B}Ready.${_N} The container connects to http://host.docker.internal:$MCP_PORT/mcp
(first time: vm/print-client-config.sh). Stop with vm/stop.sh.
EOF
