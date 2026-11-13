#!/bin/bash
# Throws away the test VM and recreates it from the golden copy (vm/snapshot.sh):
# a clean, provisioned macOS with the permissions already granted.
#   vm/reset.sh --yes   (required: this deletes everything inside warpium-test)
source "$(dirname "$0")/common.sh"

require_macos_arm64
require_tart
vm_exists "$GOLDEN_NAME" || die "no golden copy $GOLDEN_NAME; create one with vm/snapshot.sh"
[ "${1:-}" = "--yes" ] || die "this deletes everything inside $VM_NAME; re-run as: vm/reset.sh --yes"

stop_forward
if vm_running "$VM_NAME"; then
  stop_vm
  for _ in $(seq 1 30); do
    vm_running "$VM_NAME" || break
    sleep 1
  done
  vm_running "$VM_NAME" && die "VM $VM_NAME is still running; stop it and retry"
fi
if vm_exists "$VM_NAME"; then
  step "Deleting $VM_NAME"
  tart delete "$VM_NAME"
fi
step "Cloning $GOLDEN_NAME -> $VM_NAME"
tart clone "$GOLDEN_NAME" "$VM_NAME"
tart set "$VM_NAME" --cpu "$VM_CPU" --memory "$VM_MEMORY_MB"
ok "reset done. Start with vm/start.sh (then vm/provision.sh if the server code changed since the snapshot)."
