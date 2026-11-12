import struct
import zlib
from pathlib import Path

import pytest

from warpium_mac_mcp.platform import WindowInfo, parse_lsappinfo_pid, png_size
from warpium_mac_mcp.ui import FocusError, InputDisabledError, UiController, _split_text
from warpium_mac_mcp.validation import ValidationError

WARP = 100
OTHER = 200


def make_png(path: Path, w: int, h: int) -> None:
    def chunk(t, d):
        return struct.pack(">I", len(d)) + t + d + struct.pack(">I", zlib.crc32(t + d))

    raw = b"".join(b"\x00" + b"\x00\x00\x00" * w for _ in range(h))
    path.write_bytes(
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(raw))
        + chunk(b"IEND", b"")
    )


class FakePlatform:
    def __init__(self):
        self.front = WARP
        self.wins = [
            WindowInfo(1, WARP, "Warpium", "main", 100, 100, 800, 600, 0),
            WindowInfo(2, OTHER, "Other", "x", 0, 0, 2000, 2000, 0),
        ]
        self.events = []
        self.activated = []
        # Hook to change state between events (simulates focus stolen mid-sequence).
        self.after_event = None

    def frontmost_pid(self):
        return self.front

    def windows(self):
        return list(self.wins)

    def activate(self, pid):
        self.activated.append(pid)
        self.front = pid

    def _record(self, e):
        self.events.append(e)
        if self.after_event:
            self.after_event(self)

    def post_mouse(self, kind, x, y, button, click_state):
        self._record(("mouse", kind, x, y, button, click_state))

    def post_scroll(self, x, y, dx, dy):
        self._record(("scroll", x, y, dx, dy))

    def post_key(self, keycode, down, modifiers, pid):
        self._record(("key", keycode, down, tuple(sorted(modifiers)), pid))

    def post_unicode(self, text, pid):
        self._record(("text", text, pid))

    def capture_window(self, window_id, out):
        assert window_id in {w.window_id for w in self.wins if w.pid == WARP}
        make_png(out, 1600 * 2 if window_id == 1 else 100, 1200)

    def resize_png(self, src, dst, max_width):
        w, h = png_size(src)
        make_png(dst, max_width, int(h * max_width / w))

    def screen_recording_allowed(self):
        return True

    def accessibility_allowed(self):
        return True


@pytest.fixture
def fp():
    return FakePlatform()


@pytest.fixture
def ui(fp, tmp_path):
    return UiController(fp, lambda: {WARP}, screenshots_dir=tmp_path, sleep=lambda s: None)


def test_list_windows_only_target(ui):
    wins = ui.list_windows()
    assert [w.window_id for w in wins] == [1]


def test_no_target_no_windows(fp, tmp_path):
    ui = UiController(fp, lambda: set(), sleep=lambda s: None)
    assert ui.list_windows() == []
    with pytest.raises(FocusError):
        ui.press_keys("cmd+t")


def test_click_when_frontmost(ui, fp):
    ui.click(10, 20)
    assert fp.events == [
        ("mouse", "down", 110, 120, "left", 1),
        ("mouse", "up", 110, 120, "left", 1),
    ]


def test_double_click_click_states(ui, fp):
    ui.click(10, 20, count=2)
    assert [e[5] for e in fp.events] == [1, 1, 2, 2]


def test_click_refused_when_other_app_frontmost(ui, fp):
    fp.front = OTHER
    with pytest.raises(FocusError):
        ui.click(10, 20)
    assert fp.events == []


def test_focus_stolen_mid_sequence_stops_events(ui, fp):
    def steal(p):
        p.front = OTHER

    fp.after_event = steal
    with pytest.raises(FocusError):
        ui.click(10, 20, count=2)
    assert len(fp.events) == 1  # only the first mouse-down went out


def test_typing_stops_when_focus_lost(ui, fp):
    def steal(p):
        if len(p.events) == 1:
            p.front = OTHER

    fp.after_event = steal
    with pytest.raises(FocusError):
        ui.type_text("a" * 40)
    assert len(fp.events) == 1


def test_click_refused_when_covered(ui, fp):
    # Another app's window on top of the Warpium window at that point.
    fp.wins.insert(0, WindowInfo(3, OTHER, "Banner", "", 150, 110, 100, 50, 0))
    with pytest.raises(FocusError):
        ui.click(60, 20)  # screen (160, 120) -> covered
    assert fp.events == []
    ui.click(10, 20)  # screen (110, 120) -> not covered
    assert len(fp.events) == 2


def test_click_refused_under_overlay_layer(ui, fp):
    fp.wins.insert(0, WindowInfo(4, 1, "Window Server", "Menubar", 0, 0, 3000, 125, 24))
    with pytest.raises(FocusError):
        ui.click(10, 20)


def test_transparent_window_still_blocks(ui, fp):
    # alpha-0 windows can still receive mouse events, so they count as covering.
    fp.wins.insert(0, WindowInfo(5, OTHER, "Ghost", "", 0, 0, 3000, 3000, 0, alpha=0.0))
    with pytest.raises(FocusError):
        ui.click(10, 20)
    assert fp.events == []


def test_overlay_appearing_mid_click_stops_events(ui, fp):
    def banner(p):
        if len(p.events) == 1:
            p.wins.insert(0, WindowInfo(6, OTHER, "Banner", "", 100, 100, 400, 100, 0))

    fp.after_event = banner
    with pytest.raises(FocusError):
        ui.click(10, 20, count=3)
    assert len(fp.events) == 1  # mouse-up never reached the banner


def test_click_outside_window_rejected(ui, fp):
    with pytest.raises(ValidationError):
        ui.click(900, 20)
    with pytest.raises(ValidationError):
        ui.click(-1, 20)


def test_input_disabled(fp):
    ui = UiController(fp, lambda: {WARP}, allow_input=False, sleep=lambda s: None)
    for call in (
        lambda: ui.click(1, 1),
        lambda: ui.type_text("x"),
        lambda: ui.press_keys("cmd+t"),
        lambda: ui.scroll(1, 1),
    ):
        with pytest.raises(InputDisabledError):
            call()
    assert fp.events == []


def test_press_keys(ui, fp):
    ui.press_keys("cmd+shift+t")
    assert fp.events == [
        ("key", 0x11, True, ("cmd", "shift"), WARP),
        ("key", 0x11, False, ("cmd", "shift"), WARP),
    ]


def test_type_text_newline_is_return(ui, fp):
    ui.type_text("ls\npwd")
    assert fp.events == [
        ("text", "ls", WARP),
        ("key", 0x24, True, (), WARP),
        ("key", 0x24, False, (), WARP),
        ("text", "pwd", WARP),
    ]


def test_split_text_chunks_and_surrogates():
    assert _split_text("a" * 20, 16) == ["a" * 16, "a" * 4]
    emoji = "\U0001f600"
    parts = _split_text(emoji * 9, 16)
    assert parts == [emoji * 8, emoji]
    assert _split_text("a\r\nb\tc", 16) == ["a", "\n", "b", "\t", "c"]


def test_scroll(ui, fp):
    ui.scroll(10, 10, dy=-5)
    assert fp.events[0][0:2] == ("mouse", "move")
    assert fp.events[1] == ("scroll", 110, 110, 0, -5)
    with pytest.raises(ValidationError):
        ui.scroll(10, 10, dy=1000)


def test_screenshot_downscales_and_maps(ui, fp, tmp_path):
    shot = ui.screenshot()
    assert shot.image_width == 1600
    assert shot.window.window_id == 1
    assert shot.image_px_per_point == 2.0
    meta = shot.metadata()
    assert meta["window"]["bounds"]["width"] == 800
    # from_screenshot converts image pixels to window points
    ui.click(200, 100, from_screenshot=True)
    assert fp.events[0][2:4] == (200, 150)


def test_screenshot_only_warpium_windows(ui):
    with pytest.raises(LookupError):
        ui.screenshot("id:2")  # the other app's window
    with pytest.raises(LookupError):
        ui.screenshot(5)
    with pytest.raises(ValidationError):
        ui.screenshot("everything")


def test_from_screenshot_requires_prior_screenshot(ui):
    with pytest.raises(ValidationError):
        ui.click(1, 1, from_screenshot=True)


def test_focus(ui, fp):
    fp.front = OTHER
    assert ui.focus()["frontmost"] is True
    assert fp.activated == [WARP]


def test_lsappinfo_parse():
    assert parse_lsappinfo_pid('"pid"=1234\n') == 1234
    assert parse_lsappinfo_pid("") is None
