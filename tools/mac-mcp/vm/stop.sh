#!/bin/bash
# Stops the SSH forward and shuts the test VM down.
#   vm/stop.sh                 forward + VM
#   vm/stop.sh --forward-only  only the forward (VM keeps running)
source "$(dirname "$0")/common.sh"

require_macos_arm64
require_tart

step "Stopping the SSH forward"
stop_forward
if [ "${1:-}" = "--forward-only" ]; then
  exit 0
fi
if vm_running "$VM_NAME"; then
  stop_vm
  ok "VM $VM_NAME stopped"
else
  ok "VM $VM_NAME is not running"
fi
