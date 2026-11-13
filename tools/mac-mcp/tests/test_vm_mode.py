"""VM mode (serve --vm-mode): relaxed capture/input inside a macOS VM guest only,
and host mode unchanged."""

import argparse
import asyncio
from pathlib import Path

import pytest
from test_ui import OTHER, WARP, FakePlatform, make_png

from warpium_mac_mcp import cli, doctor
from warpium_mac_mcp.config import Config
from warpium_mac_mcp.platform import WindowInfo
from warpium_mac_mcp.server import build_services, create_server
from warpium_mac_mcp.ui import (
    SCREEN_WINDOW_ID,
    FocusError,
    UiController,
    VmModeRequiredError,
)
from warpium_mac_mcp.validation import ValidationError

ROOT = Path(__file__).resolve().parents[3]
VM_DIR = Path(__file__).resolve().parent.parent / "vm"
UNC = 300  # UserNotificationCenter
FAKE = 400  # a non-system app that names itself like a system dialog


class VmFakePlatform(FakePlatform):
    def __init__(self):
        super().__init__()
        self.wins = [
            # A permission alert in front of everything, partly over Warpium.
            WindowInfo(9, UNC, "UserNotificationCenter", "", 500, 400, 420, 180, 8),
            WindowInfo(8, FAKE, "SecurityAgent", "", 0, 0, 50, 50, 8),
            WindowInfo(1, WARP, "Warpium", "main", 100, 100, 800, 600, 0),
            WindowInfo(2, OTHER, "Other", "x", 0, 0, 2000, 2000, 0),
        ]
        self.exe_paths = {
            UNC: "/System/Library/CoreServices/UserNotificationCenter.app/Contents/MacOS/"
            "UserNotificationCenter",
            FAKE: "/Applications/Fake.app/Contents/MacOS/SecurityAgent",
        }
        self.captured = []

    def capture_window(self, window_id, out):
        self.captured.append(window_id)
        make_png(out, 840, 360)

    def capture_screen(self, out):
        self.captured.append("screen")
        make_png(out, 3456, 2234)


@pytest.fixture
def vfp():
    return VmFakePlatform()


def _ui(platform, tmp_path, vm):
    return UiController(
        platform, lambda: {WARP}, screenshots_dir=tmp_path, sleep=lambda s: None, vm_mode=vm
    )


# --- host mode: unchanged ------------------------------------------------------
def test_host_mode_refuses_everything_outside_warpium(vfp, tmp_path):
    ui = _ui(vfp, tmp_path, vm=False)
    with pytest.raises(VmModeRequiredError):
        ui.list_system_dialogs()
    with pytest.raises(ValidationError):
        ui.screenshot("screen")
    with pytest.raises(LookupError):
        ui.screenshot("id:9")
    with pytest.raises(VmModeRequiredError):
        ui.click(10, 10, target="screen")
    with pytest.raises(VmModeRequiredError):
        ui.press_keys("enter", target="screen")
    with pytest.raises(VmModeRequiredError):
        ui.type_text("x", target="screen")
    with pytest.raises(LookupError):
        ui.click(10, 10, window="id:9")
    # The alert covers Warpium at (600, 450): the guarded path still refuses.
    with pytest.raises(FocusError, match="UserNotificationCenter"):
        ui.click(500, 350)
    assert vfp.events == [] and vfp.captured == [] and vfp.activated == []


# --- VM mode ---------------------------------------------------------------------
def test_list_system_dialogs_requires_system_executable(vfp, tmp_path):
    ui = _ui(vfp, tmp_path, vm=True)
    assert [w.window_id for w in ui.list_system_dialogs()] == [9]


def test_screen_screenshot(vfp, tmp_path):
    ui = _ui(vfp, tmp_path, vm=True)
    shot = ui.screenshot("screen", max_width=4000)
    assert vfp.captured == ["screen"]
    assert shot.window.window_id == SCREEN_WINDOW_ID
    assert shot.image_px_per_point == 2.0
    assert "target='screen'" in shot.metadata()["coordinates"]


def test_dialog_screenshot(vfp, tmp_path):
    ui = _ui(vfp, tmp_path, vm=True)
    shot = ui.screenshot("id:9")
    assert vfp.captured == [9] and shot.window.owner == "UserNotificationCenter"
    with pytest.raises(LookupError):
        ui.screenshot("id:8")  # name-alike from /Applications is not a system dialog


def test_click_dialog_activates_and_uses_window_points(vfp, tmp_path):
    ui = _ui(vfp, tmp_path, vm=True)
    r = ui.click(20, 30, window="id:9")
    assert vfp.activated == [UNC]
    assert vfp.events == [
        ("mouse", "down", 520, 430, "left", 1),
        ("mouse", "up", 520, 430, "left", 1),
    ]
    assert r["vm_mode"] and r["hit_window"]["owner"] == "UserNotificationCenter"
    assert r["target_frontmost"] is True


def test_click_dialog_when_activation_is_ignored(vfp, tmp_path):
    vfp.activate = lambda pid: vfp.activated.append(pid)  # stays WARP frontmost
    ui = _ui(vfp, tmp_path, vm=True)
    r = ui.click(20, 30, window="id:9")
    assert r["target_frontmost"] is False and len(vfp.events) == 2


def test_click_screen_from_screenshot(vfp, tmp_path):
    ui = _ui(vfp, tmp_path, vm=True)
    with pytest.raises(ValidationError, match="no screenshot"):
        ui.click(10, 10, target="screen", from_screenshot=True)
    ui.screenshot("screen", max_width=4000)
    ui.click(1200, 900, target="screen", from_screenshot=True)
    assert vfp.events[0] == ("mouse", "down", 600, 450, "left", 1)
    assert vfp.activated == []  # target="screen" activates nothing
    with pytest.raises(ValidationError, match="outside the display"):
        ui.click(5000, 10, target="screen")


def test_keys_to_dialog_and_screen(vfp, tmp_path):
    ui = _ui(vfp, tmp_path, vm=True)
    ui.press_keys("enter", window="id:9")
    assert {e[-1] for e in vfp.events} == {UNC}
    vfp.events.clear()
    ui.type_text("ok\n", target="screen")
    assert vfp.events[0] == ("text", "ok", None)  # None = global HID tap
    assert vfp.events[1][0] == "key" and vfp.events[1][-1] is None


def test_click_dialog_refused_when_covered(vfp, tmp_path):
    # Another window stays on top of the dialog even after activation.
    vfp.wins.insert(0, WindowInfo(5, OTHER, "Other", "", 480, 380, 200, 200, 9))
    ui = _ui(vfp, tmp_path, vm=True)
    with pytest.raises(FocusError, match="covers it"):
        ui.click(20, 30, window="id:9")
    with pytest.raises(FocusError):
        ui.scroll(20, 30, window="id:9")
    assert vfp.events == []
    ui.click(520, 430, target="screen")  # the explicit escape hatch clicks what is there
    assert len(vfp.events) == 2


def test_scroll_dialog_and_screen(vfp, tmp_path):
    ui = _ui(vfp, tmp_path, vm=True)
    r = ui.scroll(20, 30, dy=-5, window="id:9")
    assert vfp.events == [("mouse", "move", 520, 430, "left", 0), ("scroll", 520, 430, 0, -5)]
    assert r["vm_mode"]
    ui.scroll(10, 10, target="screen")
    assert vfp.events[-1] == ("scroll", 10, 10, 0, -3)


def test_hid_delivery_refuses_keys_to_unactivated_dialog(vfp, tmp_path):
    vfp.key_delivery = "hid"
    vfp.activate = lambda pid: vfp.activated.append(pid)  # never becomes frontmost
    ui = _ui(vfp, tmp_path, vm=True)
    with pytest.raises(FocusError, match="HID tap"):
        ui.type_text("secret\n", window="id:9")
    assert vfp.events == []


def test_pid_delivery_keys_reach_unactivated_dialog_without_repolling(vfp, tmp_path):
    polls = []
    front = vfp.frontmost_pid
    vfp.frontmost_pid = lambda: polls.append(1) or front()
    vfp.activate = lambda pid: vfp.activated.append(pid)
    ui = _ui(vfp, tmp_path, vm=True)
    r = ui.press_keys("tab", window="id:9")
    first = len(polls)
    assert r["target_frontmost"] is False and {e[-1] for e in vfp.events} == {UNC}
    ui.press_keys("enter", window="id:9")
    assert len(polls) - first == 1  # no second poll loop for the same pid


def test_keys_to_non_frontmost_warpium_window_refused(vfp, tmp_path):
    vfp.wins = [w for w in vfp.wins if w.window_id != 9]
    vfp.wins.insert(2, WindowInfo(3, WARP, "Warpium", "second", 0, 0, 50, 50, 0))
    ui = _ui(vfp, tmp_path, vm=True)
    with pytest.raises(FocusError, match="not the frontmost Warpium window"):
        ui.press_keys("cmd+w", window="id:3")
    ui.press_keys("cmd+t", window="id:1")
    assert vfp.events and vfp.events[0][-1] == WARP


def test_warpium_path_stays_guarded_in_vm_mode(vfp, tmp_path):
    vfp.wins = [w for w in vfp.wins if w.window_id != 9]
    ui = _ui(vfp, tmp_path, vm=True)
    ui.click(10, 10, window="id:1")
    assert vfp.activated == []
    vfp.front = OTHER
    with pytest.raises(FocusError):
        ui.press_keys("cmd+t")
    with pytest.raises(FocusError):
        ui.press_keys("cmd+t", window="id:1")


def test_bad_target_rejected(vfp, tmp_path):
    with pytest.raises(ValidationError):
        _ui(vfp, tmp_path, vm=True).click(1, 1, target="desktop")


# --- detection and the --vm-mode flag ------------------------------------------
@pytest.mark.parametrize(
    ("signals", "expected"),
    [
        ({"hv_vmm_present": "1", "model": "VirtualMac2,1"}, True),
        ({"hv_vmm_present": "0", "model": "Mac14,2"}, False),
        ({"hv_vmm_present": "1", "model": "Mac14,2"}, False),
        ({"hv_vmm_present": "0", "model": "VirtualMac2,1"}, False),
        ({"hv_vmm_present": None, "model": "VirtualMac2,1"}, False),
        ({"hv_vmm_present": None, "model": None}, None),
    ],
)
def test_is_vm_needs_both_signals(signals, expected):
    assert doctor.is_vm(signals) is expected


def test_check_vm_mode():
    cli.check_vm_mode({"hv_vmm_present": "1", "model": "VirtualMac2,1"})
    with pytest.raises(SystemExit, match="refusing --vm-mode"):
        cli.check_vm_mode({"hv_vmm_present": "0", "model": "MacBookPro18,3"})


def test_serve_refuses_vm_mode_off_vm(monkeypatch, tmp_path):
    monkeypatch.setattr(doctor, "vm_signals", lambda: {"hv_vmm_present": None, "model": None})
    with pytest.raises(SystemExit, match="refusing --vm-mode"):
        cli.main(["serve", "--vm-mode", "--config-dir", str(tmp_path), "--port", "1"])
    assert not (tmp_path / "audit.log").exists()  # refused before touching state


def test_serve_flag_parsed_into_config():
    p = argparse.Namespace(host="127.0.0.1", port=1, workspace=None, config_dir=None, vm_mode=True)
    assert cli._config(p).vm_mode is True
    assert Config().vm_mode is False


# --- server-status (provision.sh's permission report) -------------------------
def test_format_server_status():
    rec = {"pid": 7, "exe": "/x/python", "checked_at": "t", "vm_mode": True}
    text, rc = cli.format_server_status({**rec, "screen_recording": True, "accessibility": True})
    assert rc == 0 and "Screen Recording: granted" in text and "VM mode:          on" in text
    text, rc = cli.format_server_status({**rec, "screen_recording": True, "accessibility": False})
    assert rc == 1 and "Accessibility:    NOT granted" in text
    _, rc = cli.format_server_status({**rec, "screen_recording": None, "accessibility": True})
    assert rc == 1
    assert cli.format_server_status(None)[1] == 2


def test_server_status_without_server(tmp_path, capsys):
    assert cli.main(["server-status", "--config-dir", str(tmp_path)]) == 2
    assert "no running server" in capsys.readouterr().out


def test_permission_record_carries_vm_mode(tmp_path):
    class P:
        def screen_recording_allowed(self):
            return True

        def accessibility_allowed(self):
            return True

    c = Config(config_dir=tmp_path / "c", workspace_dir=tmp_path / "w", vm_mode=True)
    assert doctor.record_server_permissions(c, P())["vm_mode"] is True


# --- tool surface -----------------------------------------------------------------
def _tools(tmp_path, vm):
    cfg = Config(
        workspace_dir=tmp_path / "ws",
        config_dir=tmp_path / "cfg",
        logs_dir=tmp_path / "logs",
        vm_mode=vm,
    )
    mcp = create_server(build_services(cfg, platform=None))
    return {t.name: t for t in asyncio.run(mcp.list_tools())}


def test_host_tool_surface_unchanged(tmp_path):
    tools = _tools(tmp_path, vm=False)
    assert "list_system_dialogs" not in tools
    for name in ("click", "scroll", "type_text", "press_keys"):
        assert "target" not in tools[name].inputSchema["properties"]
    assert "window" not in tools["press_keys"].inputSchema["properties"]
    assert "never the full screen" in tools["screenshot"].description


def test_vm_tool_surface(tmp_path):
    tools = _tools(tmp_path, vm=True)
    assert "list_system_dialogs" in tools
    for name in ("click", "scroll", "type_text", "press_keys"):
        props = tools[name].inputSchema["properties"]
        assert "target" in props and "window" in props
    assert "window='screen'" in tools["screenshot"].description


# --- scripts and CI ------------------------------------------------------------------
def test_guest_agent_passes_vm_mode():
    text = (VM_DIR / "provision.sh").read_text()
    assert "serve --host 127.0.0.1 --port $PORT --vm-mode" in text
    assert "server-status --wait" in text


def test_vnc_flag_and_url(tmp_path):
    common = (VM_DIR / "common.sh").read_text()
    assert "--vnc-experimental" in common and "tart run --no-graphics" in common
    import subprocess

    cfg = tmp_path / "cfg"
    state = cfg / "vm"
    state.mkdir(parents=True)
    log = state / "warpium-test.log"
    # The previous run died mid-line: no trailing newline must not leak its URL.
    log.write_text("VNC server is running at vnc://:old@127.0.0.1:5901")
    (state / "warpium-test.vnc-log-mark").write_text(str(log.stat().st_size))
    with log.open("a") as f:
        f.write("booting\nVNC server is running at vnc://:pw@127.0.0.1:61234\n")
    r = subprocess.run(
        ["bash", "-c", f'source "{VM_DIR}/common.sh"; vnc_url'],
        capture_output=True,
        text=True,
        env={"PATH": "/usr/bin:/bin", "HOME": str(tmp_path), "WARPIUM_MCP_CONFIG_DIR": str(cfg)},
    )
    assert r.returncode == 0, r.stderr
    assert r.stdout.strip() == "vnc://:pw@127.0.0.1:61234"


def test_watch_script_follows_audit_log():
    text = (VM_DIR / "watch.sh").read_text()
    assert "tail -n" in text and "audit.log" in text and "screenshots" in text


WORKFLOW = ROOT / ".github" / "workflows" / "warpium-build.yml"


@pytest.mark.skipif(not WORKFLOW.exists(), reason="not in a full checkout")
def test_build_workflow_ignores_tools():
    wf = WORKFLOW.read_text()
    assert wf.count("- 'tools/**'") == 2
