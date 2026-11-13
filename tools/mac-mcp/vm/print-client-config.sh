#!/bin/bash
# Prints the `claude mcp add ...` command for the server inside the VM. The
# bearer token is generated in the guest; this fetches it over SSH.
#
# The output contains the token. Run the printed command inside the container
# and never paste it into a shared chat, issue or commit.
#
# To keep using an existing `warpium-mac` entry unchanged instead, provision
# with the host token: WARPIUM_VM_REUSE_HOST_TOKEN=1 vm/provision.sh
source "$(dirname "$0")/common.sh"

require_macos_arm64
require_tart
require_key
vm_running "$VM_NAME" || die "VM $VM_NAME is not running; run vm/start.sh"
IP="$(vm_ip 60)" || die "no IP for $VM_NAME"

echo "# If the container already has a 'warpium-mac' entry with another token, remove it first:"
echo "#   claude mcp remove warpium-mac --scope user"
guest_ssh "$IP" "export PATH='$GUEST_PATH'; cd ~/$GUEST_DIR && uv run --frozen --quiet warpium-mac-mcp client-config --port $MCP_PORT" ||
  die "could not read the guest's client config; run vm/provision.sh"
