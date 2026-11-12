"""Argument validators. Every tool argument that reaches a subprocess, the
filesystem, or the app goes through one of these first."""

from __future__ import annotations

import re
from collections.abc import Mapping, Sequence


class ValidationError(ValueError):
    pass


_BRANCH_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._/-]{0,199}$")


def validate_branch(branch: str) -> str:
    """Conservative subset of git's check-ref-format rules."""
    if not isinstance(branch, str) or not _BRANCH_RE.match(branch):
        raise ValidationError(f"invalid branch name: {branch!r}")
    if (
        ".." in branch
        or "//" in branch
        or branch.endswith((".", "/", ".lock"))
        or "/." in branch
        or "@{" in branch
        or branch == "HEAD"
    ):
        raise ValidationError(f"invalid branch name: {branch!r}")
    return branch


PROFILES = ("debug", "release")

# Binaries defined in app/Cargo.toml that make sense to build here.
BUILD_BINS = ("warp-oss",)

_FEATURE_RE = re.compile(r"^[a-z0-9][a-z0-9_]{0,63}$")

# Cargo features the dev build gets by default: `gui` (what ./script/run
# passes) and `warp_control_cli` (enables the local-control server/warpctrl,
# which the oss channel does not turn on otherwise).
DEFAULT_BUILD_FEATURES = ("gui", "warp_control_cli")


def validate_profile(profile: str) -> str:
    if profile not in PROFILES:
        raise ValidationError(f"profile must be one of {PROFILES}")
    return profile


def validate_bin(bin_name: str) -> str:
    if bin_name not in BUILD_BINS:
        raise ValidationError(f"bin must be one of {BUILD_BINS}")
    return bin_name


def validate_features(features: Sequence[str], known: set[str] | None = None) -> list[str]:
    if isinstance(features, str):
        raise ValidationError("features must be a list of strings")
    if len(features) > 32:
        raise ValidationError("too many features")
    out: list[str] = []
    for f in features:
        if not isinstance(f, str) or not _FEATURE_RE.match(f):
            raise ValidationError(f"invalid cargo feature name: {f!r}")
        if known is not None and f not in known:
            raise ValidationError(f"unknown cargo feature for the app crate: {f!r}")
        if f not in out:
            out.append(f)
    return out


_TEST_NAME_RE = re.compile(r"^[a-z][a-z0-9_]{0,127}$")


def validate_test_name(name: str) -> str:
    if not isinstance(name, str) or not _TEST_NAME_RE.match(name):
        raise ValidationError(f"invalid integration test name: {name!r}")
    return name


def validate_run_id(run_id: str | int) -> str:
    s = str(run_id)
    if not re.fullmatch(r"[0-9]{1,20}", s):
        raise ValidationError(f"run_id must be numeric: {run_id!r}")
    return s


# Explicit allowlist. A WARP_* prefix rule is not safe: the app reads
# variables that name an executable to spawn (WARP_SHELL_PATH,
# WARP_RECORDING_PROCESS_PATH), redirect data (WARP_DATA_PROFILE), carry
# secrets (WARP_API_KEY, ...) or enable test hooks (WARP_TEST_*, WARP_MOCK_*).
# Logging/backtrace controls are what bug repros actually need.
ALLOWED_ENV = frozenset({"RUST_LOG", "RUST_BACKTRACE", "RUST_LIB_BACKTRACE"})


def validate_env(env: Mapping[str, str] | None) -> dict[str, str]:
    if not env:
        return {}
    if len(env) > len(ALLOWED_ENV):
        raise ValidationError("too many environment variables")
    out: dict[str, str] = {}
    for k, v in env.items():
        if not isinstance(k, str) or k not in ALLOWED_ENV:
            raise ValidationError(
                f"environment variable {k!r} is not allowed (allowed: {sorted(ALLOWED_ENV)})"
            )
        if not isinstance(v, str) or len(v) > 1024 or "\x00" in v or "\n" in v:
            raise ValidationError(f"invalid value for {k}")
        out[k] = v
    return out


_ARG_RE = re.compile(r"^--?[A-Za-z0-9][A-Za-z0-9_-]{0,63}(=[A-Za-z0-9_./:@,+-]{0,256})?$")
# Hidden flags that switch the binary into a different program.
_ARG_DENY_PREFIXES = ("--warpctrl",)


def validate_app_args(args: Sequence[str] | None) -> list[str]:
    """Only ``--flag`` / ``--flag=value`` style options are accepted.

    Positional arguments are refused so the binary cannot be switched into its
    non-GUI subcommands (e.g. the agent CLI), and ``--warpctrl`` is refused
    because it turns the binary into the control CLI.
    """
    if not args:
        return []
    if isinstance(args, str):
        raise ValidationError("args must be a list of strings")
    if len(args) > 16:
        raise ValidationError("too many args")
    out = []
    for a in args:
        if not isinstance(a, str) or not _ARG_RE.match(a):
            raise ValidationError(f"argument not allowed: {a!r} (only --flag or --flag=value)")
        if a.startswith(_ARG_DENY_PREFIXES):
            raise ValidationError(f"argument not allowed: {a!r}")
        out.append(a)
    return out


def validate_grep(pattern: str | None) -> str | None:
    if pattern is None or pattern == "":
        return None
    if not isinstance(pattern, str) or len(pattern) > 200 or "\x00" in pattern:
        raise ValidationError("grep must be a plain substring of at most 200 chars")
    return pattern


def validate_tail(tail: int, maximum: int = 5000) -> int:
    if not isinstance(tail, int) or isinstance(tail, bool) or tail < 1:
        raise ValidationError("tail must be a positive integer")
    return min(tail, maximum)


_SINCE_REL_RE = re.compile(r"^(\d{1,6})([smhd])$")


def parse_since(since: str | None, now: float) -> float | None:
    """Accepts relative ("90s", "15m", "2h", "1d") or ISO-8601 timestamps.
    Returns a POSIX timestamp or None."""
    if since is None or since == "":
        return None
    m = _SINCE_REL_RE.match(since)
    if m:
        mult = {"s": 1, "m": 60, "h": 3600, "d": 86400}[m.group(2)]
        return now - int(m.group(1)) * mult
    from datetime import datetime

    try:
        dt = datetime.fromisoformat(since.replace("Z", "+00:00"))
    except ValueError as e:
        raise ValidationError(
            f"since must be like '15m' or an ISO-8601 timestamp: {since!r}"
        ) from e
    if dt.tzinfo is None:
        dt = dt.astimezone()
    return dt.timestamp()


# Keys accepted by press_keys. Values are macOS virtual key codes (ANSI layout,
# from HIToolbox/Events.h).
KEY_CODES: dict[str, int] = {
    "a": 0x00,
    "s": 0x01,
    "d": 0x02,
    "f": 0x03,
    "h": 0x04,
    "g": 0x05,
    "z": 0x06,
    "x": 0x07,
    "c": 0x08,
    "v": 0x09,
    "b": 0x0B,
    "q": 0x0C,
    "w": 0x0D,
    "e": 0x0E,
    "r": 0x0F,
    "y": 0x10,
    "t": 0x11,
    "1": 0x12,
    "2": 0x13,
    "3": 0x14,
    "4": 0x15,
    "6": 0x16,
    "5": 0x17,
    "=": 0x18,
    "9": 0x19,
    "7": 0x1A,
    "-": 0x1B,
    "8": 0x1C,
    "0": 0x1D,
    "]": 0x1E,
    "o": 0x1F,
    "u": 0x20,
    "[": 0x21,
    "i": 0x22,
    "p": 0x23,
    "l": 0x25,
    "j": 0x26,
    "'": 0x27,
    "k": 0x28,
    ";": 0x29,
    "\\": 0x2A,
    ",": 0x2B,
    "/": 0x2C,
    "n": 0x2D,
    "m": 0x2E,
    ".": 0x2F,
    "`": 0x32,
    "enter": 0x24,
    "return": 0x24,
    "tab": 0x30,
    "space": 0x31,
    "delete": 0x33,
    "backspace": 0x33,
    "escape": 0x35,
    "esc": 0x35,
    "forwarddelete": 0x75,
    "home": 0x73,
    "end": 0x77,
    "pageup": 0x74,
    "pagedown": 0x79,
    "left": 0x7B,
    "right": 0x7C,
    "down": 0x7D,
    "up": 0x7E,
    "f1": 0x7A,
    "f2": 0x78,
    "f3": 0x63,
    "f4": 0x76,
    "f5": 0x60,
    "f6": 0x61,
    "f7": 0x62,
    "f8": 0x64,
    "f9": 0x65,
    "f10": 0x6D,
    "f11": 0x67,
    "f12": 0x6F,
}

MODIFIERS = {
    "cmd": "cmd",
    "command": "cmd",
    "meta": "cmd",
    "ctrl": "ctrl",
    "control": "ctrl",
    "alt": "alt",
    "option": "alt",
    "opt": "alt",
    "shift": "shift",
}

_ARROWS = frozenset({"left", "right", "up", "down"})
_FKEYS = frozenset(f"f{i}" for i in range(1, 13))


def system_shortcut_reason(mods: frozenset[str], key: str) -> str | None:
    """Rule-based refusal of shortcuts that act on the system rather than the
    app (open launchers, switch apps/spaces, log out, take screenshots, move
    keyboard focus to the Dock/menu bar). Rules match on a subset of
    modifiers, so adding extra modifiers doesn't slip past them. The fn/globe
    modifier is not accepted at all (fn+a/fn+n/... focus the Dock and
    Notification Center)."""
    if key == "space" and mods & {"cmd", "alt", "ctrl"}:
        return "Spotlight / launchers / input-source switching"
    if key == "tab" and "cmd" in mods:
        return "application switcher"
    if key == "`" and "cmd" in mods and mods & {"alt", "ctrl"}:
        return "window cycling across apps"
    if key == "q" and "cmd" in mods and mods & {"shift", "ctrl", "alt"}:
        return "log out / lock screen"
    if key == "escape" and {"cmd", "alt"} <= mods:
        return "force-quit dialog"
    if key in {"3", "4", "5", "6"} and {"cmd", "shift"} <= mods:
        return "system screenshot (full screen)"
    if key in _FKEYS and mods & {"ctrl", "cmd", "alt"}:
        return "keyboard navigation / system function keys"
    if key in _ARROWS and "ctrl" in mods:
        return "Mission Control / switching Spaces"
    if key == "d" and {"cmd", "alt"} <= mods:
        return "Dock hiding"
    return None


def parse_key_combo(combo: str) -> tuple[frozenset[str], str]:
    """Parses "cmd+shift+t" into ({"cmd","shift"}, "t")."""
    if not isinstance(combo, str) or not combo or len(combo) > 64:
        raise ValidationError(f"invalid key combo: {combo!r}")
    parts = [p.strip().lower() for p in combo.split("+")]
    if combo.endswith("++"):  # "cmd++" -> plus key not supported; use "cmd+="
        raise ValidationError("use '=' for the plus key (e.g. 'cmd+=')")
    if any(p == "" for p in parts):
        raise ValidationError(f"invalid key combo: {combo!r}")
    *mods, key = parts
    mod_set = set()
    for m in mods:
        if m not in MODIFIERS:
            raise ValidationError(f"unknown modifier {m!r}")
        mod_set.add(MODIFIERS[m])
    if key not in KEY_CODES:
        raise ValidationError(f"unknown key {key!r}")
    canon = {"enter": "return", "esc": "escape", "backspace": "delete"}.get(key, key)
    reason = system_shortcut_reason(frozenset(mod_set), canon)
    if reason:
        raise ValidationError(f"key combo {combo!r} is not allowed (system shortcut: {reason})")
    return frozenset(mod_set), key


MOUSE_BUTTONS = ("left", "right", "middle")


def validate_click(button: str, count: int) -> None:
    if button not in MOUSE_BUTTONS:
        raise ValidationError(f"button must be one of {MOUSE_BUTTONS}")
    if not isinstance(count, int) or isinstance(count, bool) or not 1 <= count <= 3:
        raise ValidationError("count must be 1, 2 or 3")


def validate_text(text: str, maximum: int = 4000) -> str:
    if not isinstance(text, str) or not text:
        raise ValidationError("text must be a non-empty string")
    if len(text) > maximum:
        raise ValidationError(f"text longer than {maximum} chars")
    return text
