#!/bin/bash
# Saves the current VM as the golden copy (default warpium-test-golden).
# vm/reset.sh later recreates warpium-test from it, so each test run can start
# from a clean, provisioned state. The clone shares unchanged blocks with the
# source (APFS copy-on-write), so it costs little disk until they diverge.
#   vm/snapshot.sh          refuses to overwrite an existing golden copy
#   vm/snapshot.sh --force  replaces it
source "$(dirname "$0")/common.sh"

require_macos_arm64
require_tart
vm_exists "$VM_NAME" || die "VM $VM_NAME does not exist; run vm/setup.sh"

if vm_exists "$GOLDEN_NAME"; then
  [ "${1:-}" = "--force" ] || die "$GOLDEN_NAME already exists; pass --force to replace it"
fi

stop_forward
stop_vm
for _ in $(seq 1 30); do
  vm_running "$VM_NAME" || break
  sleep 1
done
vm_running "$VM_NAME" && die "VM $VM_NAME is still running; stop it and retry"

if vm_exists "$GOLDEN_NAME"; then
  step "Deleting the old $GOLDEN_NAME"
  tart delete "$GOLDEN_NAME"
fi
step "Cloning $VM_NAME -> $GOLDEN_NAME"
tart clone "$VM_NAME" "$GOLDEN_NAME"
ok "golden copy saved. Reset to it any time with vm/reset.sh. Start again with vm/start.sh."
