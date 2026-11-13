#!/bin/bash
# Moves vmnet's shared (NAT) subnet, which Tart VMs use, to a free range. Use it
# when a VPN or LAN route overlaps the default 192.168.64.0/24 and setup.sh /
# start.sh report that the VM is not routed via the vmnet bridge.
#
#   vm/fix-subnet.sh [--yes] [--force] [--no-verify] [address]
#
#   address      new host address of the vmnet network, a.b.c.1 (the subnet
#                is a.b.c.0/24). Default: $WARPIUM_VM_SUGGEST_SUBNET or 192.168.251.1
#   --yes        don't ask for confirmation
#   --force      go ahead although other Tart VMs are running (they keep the
#                old subnet until they are all stopped)
#   --no-verify  don't restart the VM to check that the change took effect
#
# It stops the warpium-test VM (and its SSH forward) only, checks that the new
# subnet has no route, writes the vmnet plist with sudo, then starts the VM
# again and checks that it got an address in the new subnet. It also removes a
# stray com.apple.vmnet.plist.plist left by a `defaults write` that named the
# path with .plist. If macOS doesn't apply the change, use bridged mode
# (README "Network modes").
source "$(dirname "$0")/common.sh"

usage() {
  sed -n '6,14s/^# \{0,1\}//p' "$0"
  exit "${1:-0}"
}

ASSUME_YES=0
FORCE=0
VERIFY=1
ADDRESS=""
while [ $# -gt 0 ]; do
  case "$1" in
    --yes | -y) ASSUME_YES=1 ;;
    --force) FORCE=1 ;;
    --no-verify) VERIFY=0 ;;
    -h | --help) usage 0 ;;
    -*)
      echo "unknown option: $1" >&2
      usage 2
      ;;
    *)
      [ -z "$ADDRESS" ] || die "only one address, please"
      ADDRESS="$1"
      ;;
  esac
  shift
done
ADDRESS="${ADDRESS:-$VM_SUGGEST_SUBNET}"

require_macos_arm64
require_tart
is_ipv4 "$ADDRESS" || die "'$ADDRESS' is not an IPv4 address (expected something like 192.168.251.1)"
case "$ADDRESS" in
  *.1) ;;
  *) die "use the .1 address of the /24 you want (e.g. $(net24 "$ADDRESS").1): it becomes the host's address on the VM network" ;;
esac
case "$ADDRESS" in
  10.* | 192.168.*) ;;
  172.*)
    second="$(echo "$ADDRESS" | cut -d. -f2)"
    [ "$second" -ge 16 ] && [ "$second" -le 31 ] ||
      die "$ADDRESS is not a private address; use 10.x, 172.16-31.x or 192.168.x"
    ;;
  *) die "$ADDRESS is not a private address; use 10.x, 172.16-31.x or 192.168.x" ;;
esac
NET="$(net24 "$ADDRESS")"
MODE="$(vm_net_mode)"
if [ "$MODE" = "bridged" ]; then
  warn "the saved network mode is bridged, which doesn't use the vmnet subnet. The change"
  warn "matters only after switching back: WARPIUM_VM_NET=nat vm/start.sh"
  VERIFY=0
fi

OLD_ADDRESS="$(vmnet_setting Shared_Net_Address "")"
OLD_MASK="$(vmnet_setting Shared_Net_Mask "")"
show_old() {
  echo "    Shared_Net_Address: ${OLD_ADDRESS:-(unset, default $VMNET_DEFAULT_ADDRESS)}"
  echo "    Shared_Net_Mask:    ${OLD_MASK:-(unset, default $VMNET_MASK)}"
}

step "Current vmnet settings ($VMNET_PLIST)"
show_old
STRAY=0
if [ -e "$VMNET_STRAY_PLIST" ]; then
  STRAY=1
  warn "found $VMNET_STRAY_PLIST, written by 'defaults write' with the .plist path;"
  warn "vmnet ignores it. It will be removed."
  plutil -p "$VMNET_STRAY_PLIST" 2>/dev/null | sed 's/^/      /' || true
fi
ALREADY=0
if [ "$OLD_ADDRESS" = "$ADDRESS" ] && [ "$OLD_MASK" = "$VMNET_MASK" ]; then
  ALREADY=1
  ok "already set to $ADDRESS/$VMNET_MASK"
  if [ "$STRAY" = 0 ] && [ "$VERIFY" = 0 ]; then
    exit 0
  fi
fi

if [ "$ALREADY" = 0 ]; then
  step "Checking that $NET.0/24 is free"
  taken="$(routes_overlapping "$ADDRESS" 24 24 | without_bridge_routes)"
  if [ -n "$taken" ]; then
    printf '%s\n' "$taken" | sed 's/^/    /' >&2
    die "$NET.0/24 overlaps the routes above; pick another range: vm/fix-subnet.sh <a.b.c.1>"
  fi
  ok "no route overlaps $NET.0/24"
fi

# Other VMs keep the vmnet bridge (and its old subnet) alive, which would also
# make the verification below fail.
step "Checking running VMs"
others=""
for name in $(running_tart_vms); do
  [ "$name" = "$VM_NAME" ] || others="$others $name"
done
if [ -n "$others" ]; then
  if [ "$FORCE" = 1 ]; then
    warn "other Tart VMs are running:$others. They are left alone; the new subnet takes"
    warn "effect only after all of them are stopped, so the check below may fail."
  else
    die "other Tart VMs are running:$others. Stop them first ('tart stop <name>'), or pass --force to leave them running (the new subnet then applies only once they are all stopped)"
  fi
else
  ok "no other Tart VMs running"
fi
echo "    Other apps that use vmnet shared networking (UTM, Lima, ...) must be stopped too."

echo
echo "This will:"
echo "  - stop $VM_NAME and its SSH forward (if running)"
if [ "$ALREADY" = 0 ]; then
  echo "  - run: sudo defaults write $VMNET_DOMAIN Shared_Net_Address -string $ADDRESS"
  echo "         sudo defaults write $VMNET_DOMAIN Shared_Net_Mask -string $VMNET_MASK"
fi
[ "$STRAY" = 0 ] || echo "  - run: sudo rm $VMNET_STRAY_PLIST"
[ "$VERIFY" = 0 ] || echo "  - start $VM_NAME again and check its address"
if [ "$ASSUME_YES" != 1 ]; then
  [ -t 0 ] || die "not a terminal; re-run with --yes to confirm"
  printf 'Continue? [y/N] '
  read -r answer
  case "$answer" in
    y | Y | yes | YES) ;;
    *) die "aborted; nothing was changed" ;;
  esac
fi

stop_forward
stop_vm
vm_running "$VM_NAME" && die "$VM_NAME is still running; stop it ('tart stop $VM_NAME') and re-run"

if [ "$STRAY" = 1 ]; then
  sudo rm -f "$VMNET_STRAY_PLIST"
  ok "removed $VMNET_STRAY_PLIST"
fi

if [ "$ALREADY" = 0 ]; then
  step "Writing the vmnet settings (sudo)"
  # The domain without .plist; defaults appends it.
  sudo defaults write "$VMNET_DOMAIN" Shared_Net_Address -string "$ADDRESS"
  sudo defaults write "$VMNET_DOMAIN" Shared_Net_Mask -string "$VMNET_MASK"

  echo "    before:"
  show_old
  echo "    after:"
  echo "    Shared_Net_Address: $(vmnet_setting Shared_Net_Address "(unreadable)")"
  echo "    Shared_Net_Mask:    $(vmnet_setting Shared_Net_Mask "(unreadable)")"
  echo "    plutil -p $VMNET_PLIST:"
  sudo plutil -p "$VMNET_PLIST" | sed 's/^/      /' || true
  [ "$(vmnet_setting Shared_Net_Address "")" = "$ADDRESS" ] ||
    die "$VMNET_PLIST does not contain Shared_Net_Address $ADDRESS after the write"
fi

# Each vmnet bridge's IPv4 address, e.g. "bridge100=192.168.251.1".
bridge_addresses() {
  local b a
  for b in $(ifconfig -l); do
    case "$b" in bridge*) ;; *) continue ;; esac
    a="$(ifconfig "$b" | awk '$1 == "inet" { print $2; exit }')"
    [ -z "$a" ] || printf '%s=%s ' "$b" "$a"
  done
}

if [ "$VERIFY" = 1 ] && vm_exists "$VM_NAME"; then
  step "Starting $VM_NAME to check that the new subnet took effect"
  start_vm_headless
  ip="$(vm_ip 180)" || die "no IP after 180 s; see $VM_LOG"
  bridges="$(bridge_addresses)"
  echo "    VM address: $ip; vmnet bridges: ${bridges:-none}"
  if [ "$(net24 "$ip")" != "$NET" ]; then
    case " $bridges" in
      *"=$NET."*)
        # The bridge moved, but 'tart ip' read an old lease for the VM's MAC.
        die "a vmnet bridge is in $NET.0/24 now, but 'tart ip' reports $ip: probably a stale
entry in /var/db/dhcpd_leases. Clear it and restart the VM:
  vm/stop.sh   (and stop all other VMs)
  sudo rm -f /var/db/dhcpd_leases
  vm/fix-subnet.sh $ADDRESS      # verifies again
If it still fails, use bridged networking (README \"Network modes\"):
  WARPIUM_VM_NET=bridged vm/setup.sh"
        ;;
    esac
    die "the change did not take effect: the plist says $ADDRESS but the VM got $ip and no
vmnet bridge is in $NET.0/24. macOS may not honour the vmnet plist for Tart's NAT network.
Use bridged networking (the VM gets an address from your LAN router; README \"Network modes\"):
  vm/stop.sh
  WARPIUM_VM_NET=bridged vm/setup.sh
(Rebooting the Mac once and re-running vm/fix-subnet.sh $ADDRESS is worth one try too.)"
  fi
  ok "the VM got $ip, inside $NET.0/24"
  check_vm_route "$ip"
elif [ "$VERIFY" = 1 ]; then
  echo "    VM $VM_NAME does not exist yet; vm/setup.sh will create it."
fi

cat <<EOF

${_B}Done.${_N} Re-run vm/setup.sh (or vm/start.sh). The VM should get an address in
$NET.0/24; if it gets one in the old range, see the README "VM not reachable / VPN
route conflict".
EOF
