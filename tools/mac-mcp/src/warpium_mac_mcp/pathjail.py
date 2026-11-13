"""Filesystem jail: every path the server reads or writes on behalf of a client
must resolve (after following symlinks) inside one of a fixed set of roots."""

from __future__ import annotations

import os
from pathlib import Path


class PathEscapeError(PermissionError):
    pass


class PathJail:
    def __init__(self, roots: list[Path], files: list[Path] | None = None):
        """``roots`` are directories whose whole subtree is allowed; ``files``
        are individual files allowed outside those roots (e.g. log files)."""
        self._roots = [Path(os.path.realpath(r)) for r in roots]
        self._files = {Path(os.path.realpath(f)) for f in (files or [])}

    @property
    def roots(self) -> list[Path]:
        return list(self._roots)

    def resolve(self, path: str | os.PathLike[str], *, base: Path | None = None) -> Path:
        """Resolves ``path`` (relative paths are taken against ``base`` or the
        first root) and returns the real path if it is inside the jail.

        Symlinks are resolved with ``realpath`` before the containment check,
        so a link that points outside a root is rejected, as is any ``..``.
        Non-existent leaves are fine (realpath resolves what exists).
        """
        raw = os.fspath(path)
        if "\x00" in raw:
            raise PathEscapeError("path contains NUL byte")
        p = Path(raw).expanduser()
        if not p.is_absolute():
            p = (base or self._roots[0]) / p
        real = Path(os.path.realpath(p))
        if real in self._files:
            return real
        for root in self._roots:
            if real == root or root in real.parents:
                return real
        raise PathEscapeError(f"path {raw!r} resolves outside the allowed directories")

    def contains(self, path: str | os.PathLike[str]) -> bool:
        try:
            self.resolve(path)
            return True
        except PathEscapeError:
            return False
