"""Append-only JSON-lines audit log of every tool call."""

from __future__ import annotations

import functools
import inspect
import json
import os
import threading
import time
from collections.abc import Callable, Iterable
from pathlib import Path
from typing import Any

REDACTED = "<redacted>"
_MAX_VALUE_CHARS = 2000


def _clip(value: Any) -> Any:
    if isinstance(value, str) and len(value) > _MAX_VALUE_CHARS:
        return value[:_MAX_VALUE_CHARS] + f"...<{len(value) - _MAX_VALUE_CHARS} more chars>"
    if isinstance(value, dict):
        return {k: _clip(v) for k, v in value.items()}
    if isinstance(value, list | tuple):
        return [_clip(v) for v in value]
    return value


class AuditLog:
    def __init__(self, path: Path):
        self._path = path
        self._lock = threading.Lock()
        path.parent.mkdir(parents=True, exist_ok=True)
        fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_APPEND | os.O_NOFOLLOW, 0o600)
        os.close(fd)
        os.chmod(path, 0o600)

    def write(self, event: str, **fields: Any) -> None:
        record = {"ts": time.strftime("%Y-%m-%dT%H:%M:%S%z"), "event": event, **_clip(fields)}
        line = json.dumps(record, default=str, ensure_ascii=False)
        with self._lock:
            fd = os.open(self._path, os.O_WRONLY | os.O_APPEND | os.O_NOFOLLOW)
            with os.fdopen(fd, "a", encoding="utf-8") as f:
                f.write(line + "\n")


def redact_args(
    args: dict[str, Any],
    always: Iterable[str] = (),
    when_flag: tuple[str, Iterable[str]] | None = None,
) -> dict[str, Any]:
    """Returns a copy of ``args`` with sensitive values replaced.

    ``always``: argument names whose values are never logged.
    ``when_flag``: ``(flag_name, names)``; the named values are redacted when
    the boolean argument ``flag_name`` is true (e.g. ``type_text(redact=True)``).
    """
    out = dict(args)
    for name in always:
        if name in out:
            out[name] = REDACTED
    if when_flag is not None:
        flag, names = when_flag
        if out.get(flag):
            for name in names:
                if name in out:
                    out[name] = REDACTED
    return out


def audited(
    audit: AuditLog,
    *,
    redact: Iterable[str] = (),
    redact_when: tuple[str, Iterable[str]] | None = None,
) -> Callable[[Callable[..., Any]], Callable[..., Any]]:
    """Decorator that logs a tool call, its (redacted) arguments and outcome."""
    redact = tuple(redact)

    def decorator(fn: Callable[..., Any]) -> Callable[..., Any]:
        sig = inspect.signature(fn)

        def _bound(args: tuple, kwargs: dict) -> dict[str, Any]:
            try:
                b = sig.bind(*args, **kwargs)
                b.apply_defaults()
                return redact_args(dict(b.arguments), redact, redact_when)
            except TypeError:
                return {"_unbindable": True}

        if inspect.iscoroutinefunction(fn):

            @functools.wraps(fn)
            async def awrapper(*args: Any, **kwargs: Any) -> Any:
                call_args = _bound(args, kwargs)
                start = time.monotonic()
                try:
                    result = await fn(*args, **kwargs)
                except Exception as e:
                    audit.write(
                        "tool",
                        tool=fn.__name__,
                        args=call_args,
                        ok=False,
                        error=f"{type(e).__name__}: {e}",
                        ms=int((time.monotonic() - start) * 1000),
                    )
                    raise
                audit.write(
                    "tool",
                    tool=fn.__name__,
                    args=call_args,
                    ok=True,
                    ms=int((time.monotonic() - start) * 1000),
                )
                return result

            return awrapper

        @functools.wraps(fn)
        def wrapper(*args: Any, **kwargs: Any) -> Any:
            call_args = _bound(args, kwargs)
            start = time.monotonic()
            try:
                result = fn(*args, **kwargs)
            except Exception as e:
                audit.write(
                    "tool",
                    tool=fn.__name__,
                    args=call_args,
                    ok=False,
                    error=f"{type(e).__name__}: {e}",
                    ms=int((time.monotonic() - start) * 1000),
                )
                raise
            audit.write(
                "tool",
                tool=fn.__name__,
                args=call_args,
                ok=True,
                ms=int((time.monotonic() - start) * 1000),
            )
            return result

        return wrapper

    return decorator
