"""Window observation and guarded synthetic input.

Invariants enforced here (unit-tested against a fake platform):

* Only windows owned by a target Warpium PID are listed or captured.
* Before **every** individual input event, the frontmost application must be
  a target PID; otherwise the call stops with :class:`FocusError`.
* Mouse events additionally require that the topmost on-screen window under
  the pointer belongs to a target PID (so a notification banner or another
  app's floating window can't receive the click). The one exception is a
  full-display backdrop window owned by a macOS system UI process (see
  :func:`is_system_backdrop`), which does not take clicks in normal use.

VM mode (``serve --vm-mode``, only inside a macOS VM guest) relaxes these for
the throwaway guest: the whole display can be captured, system dialogs
(:data:`SYSTEM_DIALOG_OWNERS`) can be listed, captured and sent input via
``window="id:<n>"``, and ``target="screen"`` sends input at screen points to
whatever is there. Warpium windows keep the guarded path in both modes.
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

# Owner names of macOS system UI processes that keep full-display windows
# above normal app windows without intercepting clicks in normal operation
# (e.g. the Dock's layer-20 desktop / Mission Control / app-launcher backdrop
# on recent macOS). Only exempted together with a system executable path and
# full-display bounds, see is_system_backdrop().
SYSTEM_UI_OWNERS = frozenset(
    {
        "Dock",
        "Window Server",
        "WindowServer",
        "Control Center",
        "ControlCenter",
        "SystemUIServer",
        "Notification Center",
        "NotificationCenter",
        "Spotlight",
        "WindowManager",
        "Wallpaper",
    }
)
# Owner names of macOS system UI processes whose windows (alerts, permission
# prompts, auth sheets, Settings) VM mode may list, capture and send input to.
# Only together with an executable under SYSTEM_EXECUTABLE_PREFIXES.
SYSTEM_DIALOG_OWNERS = frozenset(
    {
        "UserNotificationCenter",
        "SecurityAgent",
        "CoreServicesUIAgent",
        "universalAccessAuthWarn",
        "System Settings",
        "System Preferences",
        "coreautha",
        "Notification Center",
        "NotificationCenter",
        "loginwindow",
        "Software Update",
        "Installer",
    }
)
# Pseudo window id of the whole main display in VM mode (screenshot "screen").
SCREEN_WINDOW_ID = 0
# A backdrop must cover at least this fraction of a display's area. The Dock
# bar, the menu bar, menus, popovers and banners are far smaller, so they keep
# counting as occluders.
BACKDROP_MIN_DISPLAY_COVERAGE = 0.9
# The owning executable must live here, so a third-party app that merely names
# itself "Dock" is not exempted.
SYSTEM_EXECUTABLE_PREFIXES = ("/System/", "/usr/libexec/", "/usr/sbin/", "/usr/bin/")

Rect = tuple[float, float, float, float]


def _overlap(a0: float, a_len: float, b0: float, b_len: float) -> float:
    return max(0.0, min(a0 + a_len, b0 + b_len) - max(a0, b0))


def is_system_backdrop(w: WindowInfo, displays: list[Rect], exe_path: str | None) -> bool:
    """True if ``w`` is a full-display window of a macOS system UI process.

    Requires all of: a known system UI owner name, an executable under a
    system prefix, a positive layer (above normal app windows), and bounds
    covering at least ``BACKDROP_MIN_DISPLAY_COVERAGE`` of some active
    display. Unknown displays or an unknown executable path mean False, so the
    window stays an occluder (fail closed).
    """
    if w.owner not in SYSTEM_UI_OWNERS or w.layer <= 0:
        return False
    if not exe_path or not exe_path.startswith(SYSTEM_EXECUTABLE_PREFIXES):
        return False
    for dx, dy, dw, dh in displays:
        area = dw * dh
        if area <= 0:
            continue
        covered = _overlap(w.x, w.width, dx, dw) * _overlap(w.y, w.height, dy, dh)
        if covered / area >= BACKDROP_MIN_DISPLAY_COVERAGE:
            return True
    return False


class FocusError(PermissionError):
    pass


class InputDisabledError(PermissionError):
    pass


class VmModeRequiredError(PermissionError):
    pass


def _vm_only(what: str) -> VmModeRequiredError:
    return VmModeRequiredError(
        f"{what} is only available in VM mode (server started with --vm-mode inside a "
        "macOS VM guest); in host mode only Warpium windows can be captured or receive input"
    )


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
        if self.window.window_id == SCREEN_WINDOW_ID:
            how = (
                "Whole display. Pass target='screen' to click/type_text/press_keys; x/y are "
                "screen points. Divide screenshot pixel coordinates by "
                "image_px_per_window_point, or pass from_screenshot=true."
            )
        else:
            how = (
                "click/scroll take window-relative points. Divide screenshot pixel "
                "coordinates by image_px_per_window_point, or pass from_screenshot=true."
            )
        return {
            "window": self.window.public(),
            "image": {
                "width": self.image_width,
                "height": self.image_height,
                "path": str(self.path),
            },
            "image_px_per_window_point": round(self.image_px_per_point, 4),
            "coordinates": how,
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
        vm_mode: bool = False,
    ):
        self._p = platform
        self._target_pids = target_pids
        self._allow_input = allow_input
        self._shots = screenshots_dir
        self._sleep = sleep
        self._delay = event_delay_s
        self._vm = vm_mode
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
        if self._vm:
            win = self._resolve_vm(window, allow_screen=True)
        else:
            win = self.resolve_window(window)
        out_dir = self._shots or Path(tempfile.gettempdir())
        out_dir.mkdir(parents=True, exist_ok=True)
        stamp = time.strftime("%Y%m%d-%H%M%S")
        raw = out_dir / f"{stamp}-w{win.window_id}-raw.png"
        final = out_dir / f"{stamp}-w{win.window_id}.png"
        if win.window_id == SCREEN_WINDOW_ID:
            self._p.capture_screen(raw)
        else:
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

    # --- VM mode -----------------------------------------------------------
    @property
    def vm_mode(self) -> bool:
        return self._vm

    def list_system_dialogs(self) -> list[WindowInfo]:
        """On-screen windows of macOS system UI processes, front-to-back (VM mode only)."""
        if not self._vm:
            raise _vm_only("list_system_dialogs")
        out = []
        for w in self._p.windows():
            if w.owner not in SYSTEM_DIALOG_OWNERS or w.width <= 1 or w.height <= 1:
                continue
            exe = self._p.process_path(w.pid)
            if exe and exe.startswith(SYSTEM_EXECUTABLE_PREFIXES):
                out.append(w)
        return out

    def _screen_window(self) -> WindowInfo:
        displays = self._p.displays()
        if not displays:
            raise LookupError("no active display reported by macOS")
        x, y, w, h = displays[0]  # CGGetActiveDisplayList lists the main display first
        return WindowInfo(SCREEN_WINDOW_ID, 0, "screen", "main display", x, y, w, h, 0)

    def _resolve_vm(self, window: str | int, allow_screen: bool = False) -> WindowInfo:
        """VM mode: a Warpium window as in host mode, a system dialog by
        ``id:<n>``, or (``allow_screen``) the whole display as ``"screen"``."""
        if allow_screen and window == "screen":
            return self._screen_window()
        if isinstance(window, str) and window.startswith("id:") and window[3:].isdigit():
            wid = int(window[3:])
            for w in self.list_windows():
                if w.window_id == wid:
                    return w
            for w in self.list_system_dialogs():
                if w.window_id == wid:
                    return w
            raise LookupError(
                f"window id {wid} is neither a Warpium window nor a system dialog "
                "(see list_windows / list_system_dialogs)"
            )
        return self.resolve_window(window)

    def _activate_for_input(self, pid: int) -> bool:
        """VM mode: best-effort activation of a system dialog's process. Agent
        processes such as UserNotificationCenter may never report as frontmost,
        so this does not fail; the result says whether it became frontmost."""
        if self._p.frontmost_pid() == pid:
            return True
        self._p.activate(pid)
        for _ in range(10):
            if self._p.frontmost_pid() == pid:
                return True
            self._sleep(0.1)
        return False

    def _vm_input_target(self, window: str | int | None, target: str) -> WindowInfo | None:
        """The non-Warpium target of an input call: the screen pseudo-window for
        target="screen" or a system dialog for window="id:<n>" (VM mode only).
        None means the call takes the guarded Warpium path, as in host mode."""
        if target not in ("window", "screen"):
            raise ValidationError("target must be 'window' or 'screen'")
        if target == "screen":
            if not self._vm:
                raise _vm_only("target='screen'")
            return self._screen_window()
        if not self._vm or window in (None, "", "frontmost-warpium"):
            return None
        win = self._resolve_vm(window)
        return None if win.pid in self._target_pids() else win

    def _to_screen_vm(
        self, win: WindowInfo, x: float, y: float, from_screenshot: bool
    ) -> tuple[float, float]:
        if win.window_id != SCREEN_WINDOW_ID:
            return self._to_screen(win, x, y, from_screenshot)
        if from_screenshot:
            scale = self._last_scale.get(SCREEN_WINDOW_ID)
            if not scale:
                raise ValidationError(
                    "no screenshot of the screen yet; take screenshot(window='screen') first"
                )
            x, y = win.x + x / scale, win.y + y / scale
        if not win.contains(x, y):
            raise ValidationError(
                f"({x:.0f},{y:.0f}) is outside the display "
                f"({win.x:.0f},{win.y:.0f} {win.width:.0f}x{win.height:.0f} points)"
            )
        return x, y

    def _topmost_at(self, sx: float, sy: float) -> WindowInfo | None:
        for w in self._p.windows():  # front-to-back
            if w.contains(sx, sy):
                return w
        return None

    def _vm_click(
        self, win: WindowInfo, x: float, y: float, button: str, count: int, from_screenshot: bool
    ) -> dict:
        sx, sy = self._to_screen_vm(win, x, y, from_screenshot)
        hit = self._topmost_at(sx, sy)
        pid = win.pid if win.window_id != SCREEN_WINDOW_ID else (hit.pid if hit else None)
        frontmost = self._activate_for_input(pid) if pid else None
        for i in range(1, count + 1):
            for kind in ("down", "up"):
                self._p.post_mouse(kind, sx, sy, button, i)
                self._sleep(self._delay)
        return {
            "clicked": {"screen_x": sx, "screen_y": sy, "button": button, "count": count},
            "window_id": win.window_id,
            "vm_mode": True,
            "hit_window": None
            if hit is None
            else {"window_id": hit.window_id, "owner": hit.owner, "pid": hit.pid},
            "target_frontmost": frontmost,
        }

    def _vm_key_pid(self, win: WindowInfo) -> tuple[int | None, bool | None]:
        """Where VM-mode keyboard events go: the dialog's process (activated
        first), or the global HID tap (None) for target="screen"."""
        if win.window_id == SCREEN_WINDOW_ID:
            return None, None
        return win.pid, self._activate_for_input(win.pid)

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
        displays: list[Rect] | None = None
        # Transparent windows are not skipped: an alpha-0 window can still
        # receive mouse events, so it counts as covering the point. Warpium's
        # own windows at any layer (context menus, popups) never block.
        for w in self._p.windows():  # front-to-back
            if not w.contains(sx, sy):
                continue
            if w.pid in pids:
                return
            if w.owner in SYSTEM_UI_OWNERS:
                if displays is None:
                    displays = self._p.displays()
                # Not cached: a pid can be reused by another process, and the
                # lookup is one proc_pidpath call.
                if is_system_backdrop(w, displays, self._p.process_path(w.pid)):
                    continue
            raise FocusError(
                f"refusing input: point ({sx:.0f},{sy:.0f}) is covered by a window of "
                f"{w.owner or 'another app'!r} (pid {w.pid}, layer {w.layer}, alpha {w.alpha}). "
                "Invisible overlays count too, since they can still take clicks."
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
        target: str = "window",
    ) -> dict:
        self._require_input_enabled()
        validate_click(button, count)
        with self._lock:
            other = self._vm_input_target(window, target)
            if other is not None:
                return self._vm_click(other, x, y, button, count, from_screenshot)
            win = self.resolve_window(window)
            sx, sy = self._to_screen(win, x, y, from_screenshot)
            for i in range(1, count + 1):
                for kind in ("down", "up"):
                    # Both checks before every mouse event: an overlay that
                    # appears mid-click must not receive the rest of it.
                    self._check_frontmost()
                    self._check_point(sx, sy)
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
            self._check_point(sx, sy)
            self._p.post_scroll(sx, sy, dx, dy)
        return {"scrolled": {"screen_x": sx, "screen_y": sy, "dx": dx, "dy": dy}}

    def press_keys(
        self, combo: str, window: str | int | None = None, target: str = "window"
    ) -> dict:
        self._require_input_enabled()
        mods, key = parse_key_combo(combo)
        code = KEY_CODES[key]
        with self._lock:
            other = self._vm_input_target(window, target)
            if other is not None:
                kpid, frontmost = self._vm_key_pid(other)
                self._p.post_key(code, True, mods, kpid)
                self._sleep(self._delay)
                self._p.post_key(code, False, mods, kpid)
                return {
                    "pressed": combo,
                    "window_id": other.window_id,
                    "vm_mode": True,
                    "target_frontmost": frontmost,
                }
            pid = self._check_frontmost()
            self._p.post_key(code, True, mods, pid)
            self._sleep(self._delay)
            pid = self._check_frontmost()
            self._p.post_key(code, False, mods, pid)
        return {"pressed": combo}

    def type_text(
        self,
        text: str,
        chunk: int = 16,
        window: str | int | None = None,
        target: str = "window",
    ) -> dict:
        self._require_input_enabled()
        validate_text(text)
        sent = 0
        with self._lock:
            other = self._vm_input_target(window, target)
            if other is not None:
                kpid, frontmost = self._vm_key_pid(other)
                for piece in _split_text(text, chunk):
                    if piece in ("\n", "\t"):
                        code = KEY_CODES["return" if piece == "\n" else "tab"]
                        self._p.post_key(code, True, frozenset(), kpid)
                        self._p.post_key(code, False, frozenset(), kpid)
                    else:
                        self._p.post_unicode(piece, kpid)
                    sent += len(piece)
                    self._sleep(self._delay)
                return {
                    "typed_chars": sent,
                    "window_id": other.window_id,
                    "vm_mode": True,
                    "target_frontmost": frontmost,
                }
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
