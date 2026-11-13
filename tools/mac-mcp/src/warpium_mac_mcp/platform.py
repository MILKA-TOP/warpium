"""Thin interface over the macOS APIs the server needs.

Everything that touches the window server, synthetic input, or screen capture
goes through :class:`Platform`, so the guard logic in ``ui.py`` can be unit
tested with a fake on Linux. :class:`MacPlatform` is the real implementation;
it imports pyobjc lazily so the package imports fine without it.
"""

from __future__ import annotations

import re
import struct
import subprocess
from dataclasses import dataclass
from pathlib import Path
from typing import Protocol


@dataclass(frozen=True)
class WindowInfo:
    window_id: int
    pid: int
    owner: str
    title: str
    x: float
    y: float
    width: float
    height: float
    layer: int
    alpha: float = 1.0

    def contains(self, sx: float, sy: float) -> bool:
        return self.x <= sx < self.x + self.width and self.y <= sy < self.y + self.height

    def public(self) -> dict:
        return {
            "window_id": self.window_id,
            "pid": self.pid,
            "title": self.title,
            "bounds": {"x": self.x, "y": self.y, "width": self.width, "height": self.height},
        }


class Platform(Protocol):
    def frontmost_pid(self) -> int | None: ...

    def windows(self) -> list[WindowInfo]:
        """All on-screen windows of all apps, front-to-back."""
        ...

    def displays(self) -> list[tuple[float, float, float, float]]:
        """Bounds (x, y, width, height) of every active display, in the same global
        top-left-origin point space as window bounds. Empty if unknown."""
        ...

    def process_path(self, pid: int) -> str | None:
        """Executable path of ``pid`` (kernel record), or None if unknown."""
        ...

    def activate(self, pid: int) -> None: ...

    def post_mouse(self, kind: str, x: float, y: float, button: str, click_state: int) -> None: ...

    def post_scroll(self, x: float, y: float, dx: int, dy: int) -> None: ...

    def post_key(
        self, keycode: int, down: bool, modifiers: frozenset[str], pid: int | None
    ) -> None:
        """pid=None posts to the global HID tap (VM mode, target="screen")."""
        ...

    def post_unicode(self, text: str, pid: int | None) -> None: ...

    def capture_window(self, window_id: int, out: Path) -> None: ...

    def capture_screen(self, out: Path) -> None:
        """The whole main display (VM mode only)."""
        ...

    def resize_png(self, src: Path, dst: Path, max_width: int) -> None: ...

    def screen_recording_allowed(self) -> bool | None: ...

    def accessibility_allowed(self) -> bool | None: ...


def png_size(path: Path) -> tuple[int, int]:
    with open(path, "rb") as f:
        head = f.read(24)
    if len(head) < 24 or head[:8] != b"\x89PNG\r\n\x1a\n" or head[12:16] != b"IHDR":
        raise ValueError(f"{path} is not a PNG")
    w, h = struct.unpack(">II", head[16:24])
    return w, h


def parse_lsappinfo_pid(output: str) -> int | None:
    m = re.search(r'"pid"\s*=\s*(\d+)', output)
    return int(m.group(1)) if m else None


class MacPlatform:
    """Real implementation (macOS only)."""

    def __init__(self, key_delivery: str = "pid"):
        # "pid": keyboard events go to the Warpium process only
        # (CGEventPostToPid), so they cannot land in another app even if focus
        # changes between the frontmost check and the event. "hid": post to the
        # global HID tap like a physical keyboard (fallback if the app ignores
        # pid-targeted events).
        if key_delivery not in ("pid", "hid"):
            raise ValueError("key_delivery must be 'pid' or 'hid'")
        self._key_delivery = key_delivery
        import Quartz  # noqa: F401  (fail early with a clear ImportError)

    # --- focus -----------------------------------------------------------
    def frontmost_pid(self) -> int | None:
        # lsappinfo reflects the live window-server state. NSWorkspace's
        # frontmostApplication can be stale in a process without a running
        # main run loop, which is exactly what this server is.
        front = subprocess.run(
            ["/usr/bin/lsappinfo", "front"], capture_output=True, text=True, timeout=5
        )
        asn = front.stdout.strip()
        if front.returncode != 0 or not re.fullmatch(r"ASN:0x[0-9a-fA-F]+-0x[0-9a-fA-F]+:", asn):
            return None
        info = subprocess.run(
            ["/usr/bin/lsappinfo", "info", "-only", "pid", asn],
            capture_output=True,
            text=True,
            timeout=5,
        )
        return parse_lsappinfo_pid(info.stdout)

    def activate(self, pid: int) -> None:
        import AppKit

        app = AppKit.NSRunningApplication.runningApplicationWithProcessIdentifier_(int(pid))
        if app is not None:
            app.activateWithOptions_(AppKit.NSApplicationActivateIgnoringOtherApps)
        if self.frontmost_pid() != pid:
            # macOS 14+ may ignore activation requests from background
            # processes; System Events (Accessibility) still works.
            script = (
                'tell application "System Events" to set frontmost of '
                f"(first process whose unix id is {int(pid)}) to true"
            )
            subprocess.run(["/usr/bin/osascript", "-e", script], capture_output=True, timeout=10)

    # --- windows ---------------------------------------------------------
    def windows(self) -> list[WindowInfo]:
        import Quartz

        opts = Quartz.kCGWindowListOptionOnScreenOnly | Quartz.kCGWindowListExcludeDesktopElements
        infos = Quartz.CGWindowListCopyWindowInfo(opts, Quartz.kCGNullWindowID) or []
        out = []
        for w in infos:
            b = w.get(Quartz.kCGWindowBounds) or {}
            out.append(
                WindowInfo(
                    window_id=int(w.get(Quartz.kCGWindowNumber, 0)),
                    pid=int(w.get(Quartz.kCGWindowOwnerPID, 0)),
                    owner=str(w.get(Quartz.kCGWindowOwnerName, "") or ""),
                    title=str(w.get(Quartz.kCGWindowName, "") or ""),
                    x=float(b.get("X", 0)),
                    y=float(b.get("Y", 0)),
                    width=float(b.get("Width", 0)),
                    height=float(b.get("Height", 0)),
                    layer=int(w.get(Quartz.kCGWindowLayer, 0)),
                    alpha=float(w.get(Quartz.kCGWindowAlpha, 1.0)),
                )
            )
        return out

    def displays(self) -> list[tuple[float, float, float, float]]:
        import Quartz

        try:
            err, ids, count = Quartz.CGGetActiveDisplayList(32, None, None)
        except Exception:  # any pyobjc failure means "unknown" (fail closed)
            return []
        if err != 0 or not ids:
            return []
        out = []
        for did in list(ids)[: int(count)]:
            r = Quartz.CGDisplayBounds(did)
            out.append(
                (float(r.origin.x), float(r.origin.y), float(r.size.width), float(r.size.height))
            )
        return out

    def process_path(self, pid: int) -> str | None:
        from .app import executable_path

        return executable_path(pid)

    # --- input -----------------------------------------------------------
    def post_mouse(self, kind: str, x: float, y: float, button: str, click_state: int) -> None:
        import Quartz

        table = {
            "left": (
                Quartz.kCGEventLeftMouseDown,
                Quartz.kCGEventLeftMouseUp,
                Quartz.kCGMouseButtonLeft,
            ),
            "right": (
                Quartz.kCGEventRightMouseDown,
                Quartz.kCGEventRightMouseUp,
                Quartz.kCGMouseButtonRight,
            ),
            "middle": (
                Quartz.kCGEventOtherMouseDown,
                Quartz.kCGEventOtherMouseUp,
                Quartz.kCGMouseButtonCenter,
            ),
        }
        down_t, up_t, btn = table[button]
        if kind == "move":
            ev = Quartz.CGEventCreateMouseEvent(None, Quartz.kCGEventMouseMoved, (x, y), btn)
        else:
            ev = Quartz.CGEventCreateMouseEvent(
                None, down_t if kind == "down" else up_t, (x, y), btn
            )
            Quartz.CGEventSetIntegerValueField(ev, Quartz.kCGMouseEventClickState, click_state)
        Quartz.CGEventPost(Quartz.kCGHIDEventTap, ev)

    def post_scroll(self, x: float, y: float, dx: int, dy: int) -> None:
        import Quartz

        create2 = getattr(Quartz, "CGEventCreateScrollWheelEvent2", None)
        if create2 is not None:  # non-variadic variant (macOS 10.13+), safest from pyobjc
            ev = create2(None, Quartz.kCGScrollEventUnitLine, 2, dy, dx, 0)
        else:
            ev = Quartz.CGEventCreateScrollWheelEvent(
                None, Quartz.kCGScrollEventUnitLine, 2, dy, dx
            )
        Quartz.CGEventSetLocation(ev, (x, y))
        Quartz.CGEventPost(Quartz.kCGHIDEventTap, ev)

    def _flags(self, modifiers: frozenset[str]) -> int:
        import Quartz

        m = {
            "cmd": Quartz.kCGEventFlagMaskCommand,
            "ctrl": Quartz.kCGEventFlagMaskControl,
            "alt": Quartz.kCGEventFlagMaskAlternate,
            "shift": Quartz.kCGEventFlagMaskShift,
            "fn": Quartz.kCGEventFlagMaskSecondaryFn,
        }
        flags = 0
        for mod in modifiers:
            flags |= m[mod]
        return flags

    def _post_keyboard(self, ev, pid: int | None) -> None:
        import Quartz

        if self._key_delivery == "pid" and pid is not None:
            Quartz.CGEventPostToPid(int(pid), ev)
        else:
            Quartz.CGEventPost(Quartz.kCGHIDEventTap, ev)

    def post_key(
        self, keycode: int, down: bool, modifiers: frozenset[str], pid: int | None
    ) -> None:
        import Quartz

        ev = Quartz.CGEventCreateKeyboardEvent(None, keycode, down)
        Quartz.CGEventSetFlags(ev, self._flags(modifiers))
        self._post_keyboard(ev, pid)

    def post_unicode(self, text: str, pid: int | None) -> None:
        import Quartz

        units = len(text.encode("utf-16-le")) // 2
        for down in (True, False):
            ev = Quartz.CGEventCreateKeyboardEvent(None, 0, down)
            Quartz.CGEventSetFlags(ev, 0)
            Quartz.CGEventKeyboardSetUnicodeString(ev, units, text)
            self._post_keyboard(ev, pid)

    # --- capture ---------------------------------------------------------
    def capture_window(self, window_id: int, out: Path) -> None:
        # -l <id>: capture exactly this window (never the whole screen);
        # -o: no shadow; -x: no sound; -t png.
        r = subprocess.run(
            [
                "/usr/sbin/screencapture",
                "-x",
                "-o",
                "-t",
                "png",
                "-l",
                str(int(window_id)),
                str(out),
            ],
            capture_output=True,
            text=True,
            timeout=30,
        )
        if r.returncode != 0 or not out.exists():
            raise RuntimeError(
                f"screencapture failed ({r.returncode}): {r.stderr.strip()} "
                "- is Screen Recording granted to the terminal running the server?"
            )

    def capture_screen(self, out: Path) -> None:
        # -m: main display only. Only reachable in VM mode (ui.py checks it).
        r = subprocess.run(
            ["/usr/sbin/screencapture", "-x", "-m", "-t", "png", str(out)],
            capture_output=True,
            text=True,
            timeout=30,
        )
        if r.returncode != 0 or not out.exists():
            raise RuntimeError(
                f"screencapture failed ({r.returncode}): {r.stderr.strip()} "
                "- is Screen Recording granted to the terminal running the server?"
            )

    def resize_png(self, src: Path, dst: Path, max_width: int) -> None:
        r = subprocess.run(
            ["/usr/bin/sips", "--resampleWidth", str(int(max_width)), str(src), "--out", str(dst)],
            capture_output=True,
            text=True,
            timeout=30,
        )
        if r.returncode != 0:
            raise RuntimeError(f"sips failed: {r.stderr.strip()}")

    # --- permissions -------------------------------------------------------
    def screen_recording_allowed(self) -> bool | None:
        import Quartz

        fn = getattr(Quartz, "CGPreflightScreenCaptureAccess", None)
        return bool(fn()) if fn else None

    def accessibility_allowed(self) -> bool | None:
        import Quartz

        fn = getattr(Quartz, "CGPreflightPostEventAccess", None)
        return bool(fn()) if fn else None
