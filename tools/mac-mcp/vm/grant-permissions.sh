#!/bin/bash
# One-time manual step: grant Screen Recording and Accessibility to Terminal
# inside the VM (Terminal runs the MCP server there). macOS has no supported
# way to grant these without a click while SIP is on, so this opens the VM's
# screen in a window on your desktop for a minute. Do it before
# vm/snapshot.sh, so the golden copy and every reset keep the grants.
source "$(dirname "$0")/common.sh"

require_macos_arm64
require_tart
vm_exists "$VM_NAME" || die "VM $VM_NAME does not exist; run vm/setup.sh"

if vm_running "$VM_NAME"; then
  stop_forward
  stop_vm
fi

pw="(see $ADMIN_PASSWORD_FILE)"
[ -f "$ADMIN_PASSWORD_FILE" ] && pw="$(cat "$ADMIN_PASSWORD_FILE")"

cat <<EOF
${_B}A window with the VM's screen opens now.${_N} The VM logs in automatically, and the server
starts in a Terminal window. In the VM:

  1. Apple menu > System Settings > Privacy & Security > Screen & System Audio Recording:
     turn on Terminal (add it with + from /System/Applications/Utilities if it isn't listed).
  2. Privacy & Security > Accessibility: turn on Terminal the same way.
     If macOS asks for a password: user admin, password $pw
  3. Shut the VM down (Apple menu > Shut Down) or close the window.

Then: vm/start.sh, and check that the doctor tool reports both permissions as granted.
If Accessibility still says "not granted": in the VM run
  tccutil reset Accessibility com.apple.Terminal
and add Terminal again.
EOF
read -r -p "Press Return to open the VM window... " _
resolve_net_args
[ "$(vm_net_mode)" != "softnet" ] || require_softnet_ready
tart run ${NET_ARGS[@]+"${NET_ARGS[@]}"} "$VM_NAME"
ok "VM window closed. Next: vm/start.sh, then vm/snapshot.sh to save a clean golden copy."
