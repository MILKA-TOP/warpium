#!/bin/bash
# Stops the SSH forward and shuts the test VM down.
#   vm/stop.sh                 forward + VM
#   vm/stop.sh --forward-only  only the forward (VM keeps running)
# Shared VM mode (vm/common.sh): only the Warpium forward (127.0.0.1:17654) is
# closed; the VM, the other project's server and its forward keep running.
source "$(dirname "$0")/common.sh"

require_macos_arm64
require_tart

step "Stopping the SSH forward"
stop_forward
if [ "${1:-}" = "--forward-only" ]; then
  exit 0
fi
if [ "$SHARED_VM" = 1 ]; then
  ok "shared VM $VM_NAME left running (stop it with $OWNER_SCRIPTS)"
  exit 0
fi
if vm_running "$VM_NAME"; then
  stop_vm
  ok "VM $VM_NAME stopped"
else
  ok "VM $VM_NAME is not running"
fi
