import base64
import json
import os
import shutil
import subprocess
import sys
from pathlib import Path

import pytest

from warpium_mac_mcp import doctor
from warpium_mac_mcp.config import Config
from warpium_mac_mcp.doctor import (
    Check,
    format_checks,
    load_server_permissions,
    record_server_permissions,
    run_checks,
    summarize,
)
from warpium_mac_mcp.vmtools import kcpassword, main

VM_DIR = Path(__file__).resolve().parent.parent / "vm"


@pytest.fixture
def cfg(tmp_path):
    return Config(
        workspace_dir=tmp_path / "ws", config_dir=tmp_path / "cfg", logs_dir=tmp_path / "logs"
    )


# --- doctor -----------------------------------------------------------------
def test_build_toolchain_does_not_affect_required(cfg, monkeypatch):
    # Nothing on PATH: git (core) fails, the build toolchain is only "not build ready".
    monkeypatch.setattr(shutil, "which", lambda name: None)
    checks = run_checks(cfg)
    by_name = {c.name: c for c in checks}
    for name in ("rustup", "cargo", "protoc"):
        assert by_name[name].category == "build"
        assert by_name[name].required is False
    assert by_name["git"].required is True
    s = summarize(checks)
    assert s["build_ready"] is False
    assert "bld " in format_checks(checks)


def test_summary_ok_without_build_toolchain():
    checks = [
        Check("git", True),
        Check("rustup", False, "not found", required=False, category="build"),
    ]
    assert summarize(checks) == {"all_required_ok": True, "build_ready": False}
    out = format_checks(checks)
    assert "[bld ] rustup" in out
    assert "required (artifacts, screenshots, input): all ok" in out


def test_timeout_reported_as_retry(monkeypatch):
    monkeypatch.setattr(shutil, "which", lambda name: f"/usr/bin/{name}")

    def slow(*a, **k):
        raise subprocess.TimeoutExpired(a[0], k.get("timeout"))

    monkeypatch.setattr(subprocess, "run", slow)
    c = doctor._build_tool("protoc", ["--version"], "brew install protobuf")
    assert not c.ok
    assert "timed out after 60 s" in c.detail
    assert "run doctor again" in c.fix
    assert c.required is False


def test_default_timeout_is_60s():
    assert doctor.TOOL_TIMEOUT_S >= 60


class FakePerms:
    def __init__(self, sr, ax):
        self.sr, self.ax = sr, ax

    def screen_recording_allowed(self):
        return self.sr

    def accessibility_allowed(self):
        return self.ax


def test_server_permissions_roundtrip(cfg):
    data = record_server_permissions(cfg, FakePerms(True, False))
    path = doctor.server_permissions_path(cfg)
    assert json.loads(path.read_text())["accessibility"] is False
    assert oct(path.stat().st_mode & 0o777) == "0o600"
    # Our own pid is ignored (this process is not "the running server").
    assert load_server_permissions(cfg) is None
    data["pid"] = 999999
    path.write_text(json.dumps(data))
    seen = []
    assert (
        load_server_permissions(cfg, alive=lambda pid, exe: seen.append((pid, exe)) or True)[
            "screen_recording"
        ]
        is True
    )
    assert seen == [(999999, data["exe"])]
    assert load_server_permissions(cfg, alive=lambda pid, exe: False) is None
    # Default check: a dead pid (or a reused one running another executable) is ignored.
    assert load_server_permissions(cfg) is None
    path.write_text("not json")
    assert load_server_permissions(cfg) is None


def test_clear_server_permissions_only_own_record(cfg):
    record_server_permissions(cfg, FakePerms(True, True))
    path = doctor.server_permissions_path(cfg)
    other = json.loads(path.read_text()) | {"pid": 999999}
    path.write_text(json.dumps(other))
    doctor.clear_server_permissions(cfg)
    assert path.exists()  # another server's record is left alone
    record_server_permissions(cfg, FakePerms(True, True))
    doctor.clear_server_permissions(cfg)
    assert not path.exists()


def test_unknown_permission_state_kept(cfg):
    data = record_server_permissions(cfg, FakePerms(None, True))
    assert data["screen_recording"] is None


def test_tool_timeout_override(cfg, monkeypatch):
    seen = []
    monkeypatch.setattr(shutil, "which", lambda name: f"/usr/bin/{name}")

    def fake_run(argv, **k):
        seen.append(k.get("timeout"))
        return subprocess.CompletedProcess(argv, 0, "v1\n", "")

    monkeypatch.setattr(subprocess, "run", fake_run)
    run_checks(cfg, tool_timeout_s=7)
    assert seen and set(seen) == {7}
    seen.clear()
    run_checks(cfg)
    assert set(seen) == {doctor.TOOL_TIMEOUT_S}


def test_server_report_wins_over_own_process(cfg, monkeypatch):
    monkeypatch.setattr(sys, "platform", "darwin")
    monkeypatch.setattr(doctor, "in_virtual_machine", lambda: True)
    monkeypatch.setattr(shutil, "which", lambda name: None)
    monkeypatch.setattr(doctor, "_run", lambda argv, timeout=60: (1, ""))
    report = {"pid": 4242, "checked_at": "t", "screen_recording": True, "accessibility": True}
    checks = run_checks(cfg, FakePerms(False, False), server_permissions=report)
    by_name = {c.name: c for c in checks}
    sr = by_name["Screen Recording permission"]
    assert sr.ok and "running server, pid 4242" in sr.detail
    assert by_name["Accessibility permission (synthetic input)"].ok
    assert "VM guest" in by_name["environment"].detail


# --- vmtools ------------------------------------------------------------------
def test_kcpassword_matches_cirruslabs_admin_blob():
    # vanilla-tahoe.pkr.hcl writes `1ced 3f4a bcbc ...` for "admin"; the bytes
    # after the NUL terminator are padding and may differ.
    enc = kcpassword("admin")
    assert len(enc) == 12
    assert enc[:6] == bytes.fromhex("1ced3f4abcbc")


def test_kcpassword_block_padding():
    assert len(kcpassword("a" * 11)) == 12
    assert len(kcpassword("a" * 12)) == 24  # the NUL needs a second block
    with pytest.raises(ValueError):
        kcpassword("")


def test_vmtools_cli(monkeypatch, capsys):
    import io

    monkeypatch.setattr(sys, "stdin", io.StringIO("admin\n"))
    assert main(["kcpassword"]) == 0
    assert base64.b64decode(capsys.readouterr().out.strip()) == kcpassword("admin")
    assert main([]) == 2


# --- shell scripts ------------------------------------------------------------
SCRIPTS = sorted(VM_DIR.glob("*.sh"))


def test_vm_scripts_exist_and_are_executable():
    names = {p.name for p in SCRIPTS}
    for n in (
        "common.sh",
        "setup.sh",
        "provision.sh",
        "start.sh",
        "stop.sh",
        "grant-permissions.sh",
        "snapshot.sh",
        "reset.sh",
        "print-client-config.sh",
        "gh-token.sh",
        "fix-subnet.sh",
    ):
        assert n in names
    for p in SCRIPTS:
        if p.name != "common.sh":
            assert os.access(p, os.X_OK), f"{p.name} is not executable"


@pytest.mark.parametrize("script", SCRIPTS, ids=lambda p: p.name)
def test_vm_script_syntax(script):
    r = subprocess.run(["bash", "-n", str(script)], capture_output=True, text=True)
    assert r.returncode == 0, r.stderr


@pytest.mark.skipif(shutil.which("shellcheck") is None, reason="shellcheck not installed")
def test_vm_scripts_shellcheck():
    r = subprocess.run(
        ["shellcheck", "-x", "-P", str(VM_DIR), *map(str, SCRIPTS)],
        capture_output=True,
        text=True,
        cwd=VM_DIR,
    )
    assert r.returncode == 0, r.stdout + r.stderr


def test_forward_binds_loopback_only():
    text = (VM_DIR / "start.sh").read_text()
    assert '-L "127.0.0.1:$MCP_PORT:127.0.0.1:$MCP_PORT"' in text
    assert text.count(" -L ") == 1  # no other (wildcard) forward


# --- vmnet route check and network modes ------------------------------------------
NETSTAT = """Routing tables

Internet:
Destination        Gateway            Flags               Netif Expire
default            link#30            UCSg                utun12
default            192.168.0.1        UGScIg                en0
10.8/14            10.0.0.1           UGSc               utun12
192.168.0          link#11            UCS                   en0      !
192.168.64         192.168.0.1        UGSc                  en0
192.168.64.7       aa:bb:cc:dd:ee:ff  UHLWIig          bridge100   1190
192.168.250.128/25 10.0.0.1           UGSc               utun12
"""

ROUTE_GET = """   route to: {ip}
destination: {ip}
       mask: 255.255.255.0
    gateway: 192.168.0.1
  interface: {iface}
"""


def _fake_bin(tmp_path, iface="en0", default_iface="utun12", plist_address=None):
    b = tmp_path / "bin"
    b.mkdir(exist_ok=True)
    (tmp_path / "netstat.txt").write_text(NETSTAT)
    (tmp_path / "route.txt").write_text(ROUTE_GET.format(ip="192.168.64.7", iface=iface))
    (tmp_path / "route_default.txt").write_text(ROUTE_GET.format(ip="default", iface=default_iface))
    plutil = (
        f'[ "$2" = Shared_Net_Address ] && echo {plist_address} && exit 0; exit 1'
        if plist_address
        else "exit 1"
    )
    for name, body in (
        ("netstat", f"cat {tmp_path / 'netstat.txt'}"),
        (
            "route",
            f'if [ "$3" = default ]; then cat {tmp_path / "route_default.txt"}; '
            f"else cat {tmp_path / 'route.txt'}; fi",
        ),
        ("plutil", plutil),
        ("tart", 'echo "tart $*"'),
    ):
        f = b / name
        f.write_text(f"#!/bin/sh\n{body}\n")
        f.chmod(0o755)
    return b


def _common(tmp_path, snippet, iface="en0", plist_address=None, **env):
    b = _fake_bin(tmp_path, iface, plist_address=plist_address)
    full_env = {
        k: v for k, v in os.environ.items() if not k.startswith(("WARPIUM_VM_", "WARPIUM_MCP_"))
    }
    full_env.update(
        PATH=f"{b}:{os.environ['PATH']}", WARPIUM_MCP_CONFIG_DIR=str(tmp_path / "cfg"), **env
    )
    return subprocess.run(
        ["bash", "-c", f'source "{VM_DIR}/common.sh"; {snippet}'],
        capture_output=True,
        text=True,
        env=full_env,
    )


@pytest.mark.parametrize(
    ("args", "expected"),
    [
        ("192.168.251.1 24 24", ""),
        ("192.168.250.1 24 24", "192.168.250.128/25"),
        ("10.9.0.1 24 24", ""),  # 10.8/14 is less specific than /24: no conflict
        ("192.168.64.7 32 1", "192.168.64 "),
    ],
)
def test_routes_overlapping(tmp_path, args, expected):
    r = _common(tmp_path, f"routes_overlapping {args} | without_bridge_routes")
    assert r.returncode == 0, r.stderr
    if expected:
        assert r.stdout.startswith(expected)
        assert len(r.stdout.splitlines()) == 1
    else:
        assert r.stdout == ""


def test_check_vm_route_fails_on_conflict(tmp_path):
    r = _common(tmp_path, "check_vm_route 192.168.64.7; echo reached")
    assert r.returncode == 1
    assert "reached" not in r.stdout
    assert "via en0" in r.stderr
    assert "192.168.64         192.168.0.1" in r.stderr
    assert (
        "defaults write /Library/Preferences/SystemConfiguration/com.apple.vmnet "
        "Shared_Net_Address -string 192.168.251.1" in r.stderr
    )
    assert "WARPIUM_VM_NET=bridged vm/setup.sh" in r.stderr
    assert "taken too" not in r.stderr
    assert "did not apply" not in r.stderr


def test_check_vm_route_flags_taken_suggestion(tmp_path):
    r = _common(tmp_path, "check_vm_route 192.168.64.7", WARPIUM_VM_SUGGEST_SUBNET="192.168.250.1")
    assert r.returncode == 1
    assert "192.168.250.0/24 is taken too" in r.stderr


def test_check_vm_route_detects_ignored_subnet_change(tmp_path):
    r = _common(tmp_path, "check_vm_route 192.168.64.7", plist_address="192.168.251.1")
    assert r.returncode == 1
    assert "macOS did not apply" in r.stderr
    assert "vmnet plist: 192.168.251.1/24" in r.stderr


def test_check_vm_route_passes_via_bridge(tmp_path):
    r = _common(tmp_path, "check_vm_route 192.168.64.7; echo reached", iface="bridge100")
    assert r.returncode == 0, r.stderr
    assert "reached" in r.stdout


def test_net_mode_defaults_to_nat_and_is_saved(tmp_path):
    r = _common(tmp_path, "vm_net_mode")
    assert r.stdout.strip() == "nat"
    r = _common(tmp_path, "save_net_mode; vm_net_mode", WARPIUM_VM_NET="bridged")
    assert r.returncode == 0, r.stderr
    assert (tmp_path / "cfg" / "vm" / "net_mode").read_text().strip() == "bridged"
    # A later run without the variable reuses the saved mode.
    r = _common(tmp_path, "vm_net_mode")
    assert r.stdout.strip() == "bridged"


def test_net_mode_rejects_unknown(tmp_path):
    r = _common(tmp_path, "vm_net_mode", WARPIUM_VM_NET="host")
    assert r.returncode == 1
    assert "unknown network mode" in r.stderr


def test_bridged_mode_skips_vpn_interface(tmp_path):
    # `route get default` says utun12 (the VPN); the physical default route is en0.
    r = _common(
        tmp_path,
        'resolve_net_args; echo "${NET_ARGS[*]}"; vm_ip 5',
        WARPIUM_VM_NET="bridged",
    )
    assert r.returncode == 0, r.stderr
    args, ip_cmd = r.stdout.splitlines()
    assert args == "--net-bridged=en0"
    assert "--resolver=arp" in ip_cmd


def test_bridged_route_check_expects_bridge_interface(tmp_path):
    r = _common(tmp_path, "check_vm_route 192.168.64.7", WARPIUM_VM_NET="bridged")
    assert r.returncode == 0, r.stderr
    r = _common(tmp_path, "check_vm_route 192.168.64.7", iface="utun12", WARPIUM_VM_NET="bridged")
    assert r.returncode == 1
    assert "local network" in r.stderr


def test_nat_mode_has_no_net_flags(tmp_path):
    r = _common(tmp_path, 'resolve_net_args; echo "[${NET_ARGS[*]:-}]"; vm_ip 5')
    assert r.returncode == 0, r.stderr
    flags, ip_cmd = r.stdout.splitlines()
    assert flags == "[]"
    assert "--resolver" not in ip_cmd


def test_subnet_check_command_escapes_dots_once(tmp_path):
    r = _common(tmp_path, "check_vm_route 192.168.64.7")
    assert "grep -E '^192\\.168\\.251([./ ]|$)'" in r.stderr


def test_bridge_interface_is_saved_with_the_mode(tmp_path):
    r = _common(tmp_path, "save_net_mode", WARPIUM_VM_NET="bridged", WARPIUM_VM_BRIDGE_IF="en7")
    assert r.returncode == 0, r.stderr
    r = _common(tmp_path, 'resolve_net_args; echo "${NET_ARGS[*]}"')
    assert r.stdout.strip() == "--net-bridged=en7"


def test_ssh_hint_mentions_skipped_route_check(tmp_path):
    r = _common(tmp_path, "ssh_unreachable_hint", WARPIUM_VM_SKIP_ROUTE_CHECK="1")
    assert "route check was skipped" in r.stdout
    r = _common(tmp_path, "ssh_unreachable_hint")
    assert "route check passed" in r.stdout
    assert "tart run warpium-test" in r.stdout
    assert "tart exec" in r.stdout
