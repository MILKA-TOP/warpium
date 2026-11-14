"""app_reset_data: VM mode only, confirm required, deletes only Warpium's user data."""

import subprocess
from pathlib import Path

import pytest

from warpium_mac_mcp.app import MAX_KEYCHAIN_ITEMS, AppManager
from warpium_mac_mcp.config import Config
from warpium_mac_mcp.jobs import JobManager
from warpium_mac_mcp.pathjail import PathJail
from warpium_mac_mcp.ui import VmModeRequiredError
from warpium_mac_mcp.validation import ValidationError
from warpium_mac_mcp.workspace import Workspace, WorkspaceError


class FakeRun:
    """Records `defaults`/`security` calls; each keychain service holds `items[svc]` items."""

    def __init__(self, items=None, defaults_rc=0, defaults_err=""):
        self.calls = []
        self.items = dict(items or {})
        self.defaults_rc = defaults_rc
        self.defaults_err = defaults_err

    def __call__(self, argv, **kwargs):
        assert isinstance(argv, list) and "shell" not in kwargs
        assert kwargs.get("timeout")
        self.calls.append(argv)
        if argv[0] == "security":
            svc = argv[-1]
            if self.items.get(svc, 0) > 0:
                self.items[svc] -= 1
                return subprocess.CompletedProcess(argv, 0, "", "")
            return subprocess.CompletedProcess(argv, 44, "", "not found")
        return subprocess.CompletedProcess(argv, self.defaults_rc, "", self.defaults_err)


def _manager(tmp_path, vm=True, procs=None, run=None):
    home = tmp_path / "home"
    cfg = Config(
        workspace_dir=tmp_path / "ws",
        config_dir=tmp_path / "cfg",
        logs_dir=home / "Library" / "Logs",
        vm_mode=vm,
    )
    cfg.ensure_dirs()
    jail = PathJail([cfg.workspace_dir])
    ws = Workspace(cfg, JobManager(cfg.jobs_dir), jail)
    am = AppManager(
        cfg, ws, jail, processes=lambda: list(procs or []), home=home, run=run or FakeRun()
    )
    return am, home


def _populate(home: Path) -> dict[str, Path]:
    lib = home / "Library"
    p = {
        "config": home / ".warp-oss",
        "config_profile": home / ".warp-oss-mcp-test",
        "support": lib / "Application Support" / "dev.milka.Warpium",
        "support_profile": lib / "Application Support" / "dev.milka.Warpium-mcp-test",
        "plist": lib / "Preferences" / "dev.milka.Warpium.plist",
        "log": lib / "Logs" / "warp-oss.log",
        "log_old": lib / "Logs" / "warp-oss.log.old.0",
        "log_chunk": lib / "Logs" / "warp-oss.log.in_session.0",
        "log_old_chunk": lib / "Logs" / "warp-oss.log.old.1.in_session.2",
        "group": lib
        / "Group Containers"
        / "2BBY89MBSN.dev.warp"
        / "Library"
        / "Application Support"
        / "dev.milka.Warpium",
    }
    for key in ("config", "config_profile", "support", "support_profile", "group"):
        (p[key] / "sub").mkdir(parents=True)
        (p[key] / "sub" / "f").write_text("x")
    for key in ("plist", "log", "log_old", "log_chunk", "log_old_chunk"):
        p[key].parent.mkdir(parents=True, exist_ok=True)
        p[key].write_text("x")
    return p


def _keep(home: Path) -> list[Path]:
    """Data that must survive: other apps, Warp itself, the server's own state."""
    lib = home / "Library"
    keep = [
        home / ".warp",
        lib / "Application Support" / "dev.warp.Warp-Stable",
        lib / "Preferences" / "dev.warp.Warp-Stable.plist",
        lib / "Preferences" / "dev.milka.Warpium.other.plist",
        lib / "Logs" / "warp.log",
        lib / "Logs" / "warp-oss-other.log",
        lib
        / "Group Containers"
        / "2BBY89MBSN.dev.warp"
        / "Library"
        / "Application Support"
        / "dev.warp.Warp-Stable"
        / "warp.sqlite",
        home / ".config" / "warpium-mac-mcp" / "token",
    ]
    for k in keep:
        k.parent.mkdir(parents=True, exist_ok=True)
        k.write_text("keep")
    return keep


def test_refused_in_host_mode(tmp_path):
    run = FakeRun()
    am, home = _manager(tmp_path, vm=False, run=run)
    p = _populate(home)
    with pytest.raises(VmModeRequiredError):
        am.reset_data(confirm=True)
    assert all(path.exists() for path in p.values())
    assert run.calls == []


@pytest.mark.parametrize("confirm", [False, None, "true", 1])
def test_requires_confirm_true(tmp_path, confirm):
    run = FakeRun()
    am, home = _manager(tmp_path, run=run)
    p = _populate(home)
    with pytest.raises(ValidationError):
        am.reset_data(confirm=confirm)
    assert all(path.exists() for path in p.values())
    assert run.calls == []


def test_deletes_warpium_data_only(tmp_path):
    run = FakeRun(items={"dev.milka.Warpium": 3, "dev.milka.Warpium-mcp-test": 1})
    am, home = _manager(tmp_path, run=run)
    p = _populate(home)
    keep = _keep(home)
    out = am.reset_data(confirm=True)
    assert not any(path.exists() for path in p.values())
    assert all(k.exists() for k in keep)
    assert set(out["removed"]) == {str(path) for path in p.values()}
    assert out["errors"] == {}
    assert out["ok"] is True
    assert out["defaults_delete"] == {
        "dev.milka.Warpium": "deleted",
        "dev.milka.Warpium-mcp-test": "deleted",
    }
    assert out["keychain_items_deleted"] == {
        "dev.milka.Warpium": 3,
        "dev.milka.Warpium-mcp-test": 1,
    }
    assert ["defaults", "delete", "dev.milka.Warpium"] in run.calls
    assert ["security", "delete-generic-password", "-s", "dev.milka.Warpium"] in run.calls
    # Idempotent: a second run finds nothing.
    again = am.reset_data(confirm=True)
    assert again["removed"] == []


def test_symlink_is_removed_not_followed(tmp_path):
    am, home = _manager(tmp_path)
    outside = tmp_path / "precious"
    outside.mkdir()
    (outside / "f").write_text("keep")
    home.mkdir(parents=True, exist_ok=True)
    (home / ".warp-oss").symlink_to(outside)
    out = am.reset_data(confirm=True)
    assert str(home / ".warp-oss") in out["removed"]
    assert not (home / ".warp-oss").is_symlink()
    assert (outside / "f").read_text() == "keep"


def test_refuses_while_other_warpium_runs(tmp_path):
    run = FakeRun()
    am, home = _manager(
        tmp_path, procs=[(4242, "/Applications/Warpium.app/Contents/MacOS/warp-oss")], run=run
    )
    p = _populate(home)
    with pytest.raises(WorkspaceError, match="4242"):
        am.reset_data(confirm=True)
    assert all(path.exists() for path in p.values())
    assert run.calls == []


def test_refusal_leaves_app_under_test_running(tmp_path):
    from warpium_mac_mcp.app import Launched

    am, _ = _manager(tmp_path, procs=[(4242, "/Applications/Warpium.app/Contents/MacOS/warp-oss")])
    proc = subprocess.Popen(["sleep", "60"])
    try:
        am._launched = Launched(proc, "build", Path("/x"), 0.0, tmp_path / "o.log", {}, [])
        with pytest.raises(WorkspaceError, match="4242"):
            am.reset_data(confirm=True)
        assert proc.poll() is None
    finally:
        proc.kill()
        proc.wait()


def test_defaults_absent_vs_error(tmp_path):
    missing = "Domain (dev.milka.Warpium) not found.\nDefaults have not been changed."
    am, _ = _manager(tmp_path, run=FakeRun(defaults_rc=1, defaults_err=missing))
    out = am.reset_data(confirm=True)
    assert out["defaults_delete"]["dev.milka.Warpium"] == "absent"
    assert out["ok"] is True

    failed = "Could not write domain dev.milka.Warpium; exiting"
    am, _ = _manager(tmp_path, run=FakeRun(defaults_rc=1, defaults_err=failed))
    out = am.reset_data(confirm=True)
    assert out["defaults_delete"]["dev.milka.Warpium"].startswith("error (exit 1)")
    assert out["ok"] is False
    assert "defaults delete dev.milka.Warpium" in out["errors"]


def test_keychain_error_is_reported(tmp_path):
    def run(argv, **kwargs):
        if argv[0] == "security":
            return subprocess.CompletedProcess(argv, 51, "", "user interaction is not allowed")
        return subprocess.CompletedProcess(argv, 0, "", "")

    am, _ = _manager(tmp_path, run=run)
    out = am.reset_data(confirm=True)
    assert "exit 51" in out["keychain_items_deleted"]["dev.milka.Warpium"]
    assert out["ok"] is False


def test_quits_app_under_test_first(tmp_path):
    am, home = _manager(tmp_path)
    proc = subprocess.Popen(["sleep", "60"])
    try:
        from warpium_mac_mcp.app import Launched

        am._launched = Launched(proc, "build", Path("/x"), 0.0, tmp_path / "o.log", {}, [])
        out = am.reset_data(confirm=True)
        assert out["quit"]["quit"] == [proc.pid]
        assert proc.poll() is not None
    finally:
        if proc.poll() is None:
            proc.kill()
            proc.wait()


def test_keychain_loop_is_bounded(tmp_path):
    run = FakeRun(items={"dev.milka.Warpium": 10_000})
    am, _ = _manager(tmp_path, run=run)
    out = am.reset_data(confirm=True)
    assert "limit" in out["keychain_items_deleted"]["dev.milka.Warpium"]
    calls = [c for c in run.calls if c[0] == "security" and c[-1] == "dev.milka.Warpium"]
    assert len(calls) == MAX_KEYCHAIN_ITEMS


def test_keychain_timeout_is_reported(tmp_path):
    def run(argv, **kwargs):
        if argv[0] == "security":
            raise subprocess.TimeoutExpired(argv, kwargs["timeout"])
        return subprocess.CompletedProcess(argv, 1, "", "Domain x does not exist")

    am, _ = _manager(tmp_path, run=run)
    out = am.reset_data(confirm=True)
    assert out["defaults_delete"]["dev.milka.Warpium"] == "absent"
    assert out["keychain_items_deleted"]["dev.milka.Warpium"].startswith("error after 0")
    assert out["ok"] is False
