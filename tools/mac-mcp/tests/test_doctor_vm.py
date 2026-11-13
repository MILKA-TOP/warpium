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
    assert load_server_permissions(cfg, alive=lambda pid: True)["screen_recording"] is True
    assert load_server_permissions(cfg, alive=lambda pid: False) is None
    path.write_text("not json")
    assert load_server_permissions(cfg) is None


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
