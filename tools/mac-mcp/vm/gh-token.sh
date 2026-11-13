#!/bin/bash
# Logs gh in inside the VM with a fine-grained, read-only token, so the
# install_artifact tool can download CI artifacts there. Your own gh login
# (full account access) never enters the VM.
#
# Create the token at https://github.com/settings/personal-access-tokens/new :
#   Resource owner: MILKA-TOP; Repository access: Only select repositories ->
#   MILKA-TOP/warpium; Permissions: Actions = Read-only (Metadata = Read-only is
#   added automatically); a short expiry, e.g. 30-90 days.
# Then run this script and paste the token at the prompt (it is not echoed).
source "$(dirname "$0")/common.sh"

require_macos_arm64
require_tart
require_key
vm_running "$VM_NAME" || die "VM $VM_NAME is not running; run vm/start.sh"
IP="$(vm_ip 60)" || die "no IP for $VM_NAME"

read -r -s -p "Fine-grained token (input hidden): " token
echo
case "$token" in
  github_pat_*) ;;
  gh[opsu]_*) warn "this looks like a classic or OAuth token; a fine-grained github_pat_ token limited to MILKA-TOP/warpium is safer" ;;
  "") die "no token entered" ;;
  *) warn "unexpected token format; trying anyway" ;;
esac

step "Logging gh in inside the VM"
printf '%s\n' "$token" | guest_ssh "$IP" "export PATH='$GUEST_PATH'; gh auth login --hostname github.com --with-token && gh auth status" ||
  die "gh auth login failed in the guest (output above)"
unset token
ok "gh is logged in inside the VM. Rotate by re-running this script; revoke the token on GitHub when done."
