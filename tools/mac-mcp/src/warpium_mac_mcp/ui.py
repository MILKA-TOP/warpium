"""Window observation and guarded synthetic input.

Invariants enforced here (unit-tested against a fake platform):

* Only windows owned by a target Warpium PID are listed or captured.
* Before **every** individual input event, the frontmost application must be
  a target PID; otherwise the call stops with :class:`FocusError`.
* Mouse events additionally require that the topmost on-screen window under
  the pointer belongs to a target PID (so a notification banner or another
  app's floating window can't receive the click).
"""

from __future__ import annotations

import tempfile
import threading
import time
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path

from .platform import Platform, WindowInfo, png_size
from .validation import KEY_CODES, ValidationError, parse_key_combo, validate_click, validate_text


class FocusError(PermissionError):
    pass


class InputDisabledError(PermissionError):
    pass


@dataclass
class Screenshot:
    path: Path
    window: WindowInfo
    image_width: int
    image_height: int

    @property
    def image_px_per_point(self) -> float:
        return self.image_width / self.window.width if self.window.width else 1.0

    def metadata(self) -> dict:
        return {
            "window": self.window.public(),
            "image": {
                "width": self.image_width,
                "height": self.image_height,
                "path": str(self.path),
            },
            "image_px_per_window_point": round(self.image_px_per_point, 4),
            "coordinates": (
                "click/scroll take window-relative points. Divide screenshot pixel "
                "coordinates by image_px_per_window_point, or pass from_screenshot=true."
            ),
        }


class UiController:
    def __init__(
        self,
        platform: Platform,
        target_pids: Callable[[], set[int]],
        *,
        allow_input: bool = True,
        screenshots_dir: Path | None = None,
        sleep: Callable[[float], None] = time.sleep,
        event_delay_s: float = 0.01,
    ):
        self._p = platform
        self._target_pids = target_pids
        self._allow_input = allow_input
        self._shots = screenshots_dir
        self._sleep = sleep
        self._delay = event_delay_s
        self._last_scale: dict[int, float] = {}
        self._lock = threading.Lock()  # serialize input sequences

    # --- observation -------------------------------------------------------
    def list_windows(self) -> list[WindowInfo]:
        pids = self._target_pids()
        if not pids:
            return []
        return [
            w
            for w in self._p.windows()
            if w.pid in pids and w.layer == 0 and w.width > 1 and w.height > 1
        ]

    def resolve_window(self, window: str | int = "frontmost-warpium") -> WindowInfo:
        wins = self.list_windows()
        if not wins:
            raise LookupError("no on-screen Warpium window (is the app running? see app_status)")
        if window in ("frontmost-warpium", "", None):
            return wins[0]
        if isinstance(window, int) and not isinstance(window, bool):
            if 0 <= window < len(wins):
                return wins[window]
            raise LookupError(f"window index {window} out of range (0..{len(wins) - 1})")
        if isinstance(window, str) and window.startswith("id:") and window[3:].isdigit():
            wid = int(window[3:])
            for w in wins:
                if w.window_id == wid:
                    return w
            raise LookupError(f"window id {wid} is not a Warpium window")
        if isinstance(window, str) and window.isdigit():
            return self.resolve_window(int(window))
        raise ValidationError("window must be 'frontmost-warpium', an index, or 'id:<window_id>'")

    def screenshot(
        self, window: str | int = "frontmost-warpium", max_width: int = 1600
    ) -> Screenshot:
        if not 200 <= max_width <= 4000:
            raise ValidationError("max_width must be between 200 and 4000")
        win = self.resolve_window(window)
        out_dir = self._shots or Path(tempfile.gettempdir())
        out_dir.mkdir(parents=True, exist_ok=True)
        stamp = time.strftime("%Y%m%d-%H%M%S")
        raw = out_dir / f"{stamp}-w{win.window_id}-raw.png"
        final = out_dir / f"{stamp}-w{win.window_id}.png"
        self._p.capture_window(win.window_id, raw)
        w, _h = png_size(raw)
        if w > max_width:
            self._p.resize_png(raw, final, max_width)
            raw.unlink(missing_ok=True)
        else:
            raw.rename(final)
        iw, ih = png_size(final)
        shot = Screenshot(final, win, iw, ih)
        self._last_scale[win.window_id] = shot.image_px_per_point
        return shot

    # --- guards ------------------------------------------------------------
    def _require_input_enabled(self) -> None:
        if not self._allow_input:
            raise InputDisabledError("input tools are disabled (server started with --no-input)")

    def _check_frontmost(self) -> int:
        """Returns the frontmost PID if it is a target; raises otherwise.
        Called immediately before every single event."""
        pids = self._target_pids()
        if not pids:
            raise FocusError("no Warpium process is running under this server")
        front = self._p.frontmost_pid()
        if front not in pids:
            raise FocusError(
                f"refusing input: frontmost app (pid {front}) is not Warpium {sorted(pids)}; "
                "call focus_app first"
            )
        return front

    def _check_point(self, sx: float, sy: float) -> None:
        pids = self._target_pids()
        for w in self._p.windows():  # front-to-back
            if w.alpha <= 0 or not w.contains(sx, sy):
                continue
            if w.pid in pids:
                return
            raise FocusError(
                f"refusing input: point ({sx:.0f},{sy:.0f}) is covered by another app's window"
            )
        raise FocusError(f"refusing input: no window under point ({sx:.0f},{sy:.0f})")

    def _to_screen(
        self, win: WindowInfo, x: float, y: float, from_screenshot: bool
    ) -> tuple[float, float]:
        if from_screenshot:
            scale = self._last_scale.get(win.window_id)
            if not scale:
                raise ValidationError("no screenshot of this window yet; take one or pass points")
            x, y = x / scale, y / scale
        if not (0 <= x < win.width and 0 <= y < win.height):
            raise ValidationError(
                f"({x:.0f},{y:.0f}) is outside the window ({win.width:.0f}x{win.height:.0f} points)"
            )
        return win.x + x, win.y + y

    # --- input -------------------------------------------------------------
    def focus(self) -> dict:
        pids = self._target_pids()
        if not pids:
            raise FocusError("no Warpium process is running under this server")
        wins = self.list_windows()
        pid = wins[0].pid if wins else min(pids)
        self._p.activate(pid)
        for _ in range(20):
            if self._p.frontmost_pid() in pids:
                return {"frontmost": True, "pid": pid}
            self._sleep(0.1)
        return {
            "frontmost": False,
            "pid": pid,
            "hint": "macOS refused activation; click the Warpium window once manually",
        }

    def click(
        self,
        x: float,
        y: float,
        button: str = "left",
        count: int = 1,
        window: str | int = "frontmost-warpium",
        from_screenshot: bool = False,
    ) -> dict:
        self._require_input_enabled()
        validate_click(button, count)
        with self._lock:
            win = self.resolve_window(window)
            sx, sy = self._to_screen(win, x, y, from_screenshot)
            self._check_frontmost()
            self._check_point(sx, sy)
            for i in range(1, count + 1):
                for kind in ("down", "up"):
                    self._check_frontmost()
                    self._p.post_mouse(kind, sx, sy, button, i)
                    self._sleep(self._delay)
        return {
            "clicked": {"screen_x": sx, "screen_y": sy, "button": button, "count": count},
            "window_id": win.window_id,
        }

    def scroll(
        self,
        x: float,
        y: float,
        dx: int = 0,
        dy: int = -3,
        window: str | int = "frontmost-warpium",
        from_screenshot: bool = False,
    ) -> dict:
        self._require_input_enabled()
        for v in (dx, dy):
            if not isinstance(v, int) or isinstance(v, bool) or not -50 <= v <= 50:
                raise ValidationError("dx/dy must be integers in [-50, 50] (lines)")
        with self._lock:
            win = self.resolve_window(window)
            sx, sy = self._to_screen(win, x, y, from_screenshot)
            self._check_frontmost()
            self._check_point(sx, sy)
            self._p.post_mouse("move", sx, sy, "left", 0)
            self._check_frontmost()
            self._p.post_scroll(sx, sy, dx, dy)
        return {"scrolled": {"screen_x": sx, "screen_y": sy, "dx": dx, "dy": dy}}

    def press_keys(self, combo: str) -> dict:
        self._require_input_enabled()
        mods, key = parse_key_combo(combo)
        code = KEY_CODES[key]
        with self._lock:
            pid = self._check_frontmost()
            self._p.post_key(code, True, mods, pid)
            self._sleep(self._delay)
            pid = self._check_frontmost()
            self._p.post_key(code, False, mods, pid)
        return {"pressed": combo}

    def type_text(self, text: str, chunk: int = 16) -> dict:
        self._require_input_enabled()
        validate_text(text)
        sent = 0
        with self._lock:
            for piece in _split_text(text, chunk):
                pid = self._check_frontmost()
                if piece in ("\n", "\t"):
                    code = KEY_CODES["return" if piece == "\n" else "tab"]
                    self._p.post_key(code, True, frozenset(), pid)
                    pid = self._check_frontmost()
                    self._p.post_key(code, False, frozenset(), pid)
                else:
                    self._p.post_unicode(piece, pid)
                sent += len(piece)
                self._sleep(self._delay)
        return {"typed_chars": sent}


def _split_text(text: str, chunk: int) -> list[str]:
    """Splits into unicode chunks of <= ``chunk`` UTF-16 units; newline and tab
    become their own pieces (sent as Return / Tab key presses)."""
    text = text.replace("\r\n", "\n").replace("\r", "\n")
    out: list[str] = []
    buf = ""
    units = 0
    for ch in text:
        if ch in ("\n", "\t"):
            if buf:
                out.append(buf)
                buf, units = "", 0
            out.append(ch)
            continue
        u = 2 if ord(ch) > 0xFFFF else 1
        if units + u > chunk:
            out.append(buf)
            buf, units = "", 0
        buf += ch
        units += u
    if buf:
        out.append(buf)
    return out
