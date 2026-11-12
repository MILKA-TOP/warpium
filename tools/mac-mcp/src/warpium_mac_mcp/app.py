"""Warpium process lifecycle and log access."""

from __future__ import annotations

import os
import re
import signal
import subprocess
import threading
import time
from collections import deque
from collections.abc import Callable, Mapping
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path

from .config import Config
from .pathjail import PathJail
from .validation import ValidationError, validate_app_args, validate_env
from .workspace import Workspace, WorkspaceError

SOURCES = ("build", "artifact")


@dataclass
class Launched:
    proc: subprocess.Popen
    source: str
    binary: Path
    started_at: float
    stdout_log: Path
    env_overrides: dict[str, str]
    args: list[str]


def list_processes() -> list[tuple[int, str]]:
    """(pid, executable path) for all processes of the current user."""
    r = subprocess.run(
        ["ps", "-x", "-ww", "-o", "pid=,comm="], capture_output=True, text=True, timeout=10
    )
    out = []
    for line in r.stdout.splitlines():
        m = re.match(r"\s*(\d+)\s+(.*)$", line)
        if m:
            out.append((int(m.group(1)), m.group(2).strip()))
    return out


class AppManager:
    def __init__(
        self,
        cfg: Config,
        workspace: Workspace,
        jail: PathJail,
        *,
        processes: Callable[[], list[tuple[int, str]]] = list_processes,
        base_env: Callable[[], dict[str, str]] | None = None,
    ):
        self.cfg = cfg
        self.ws = workspace
        self.jail = jail
        self._processes = processes
        self._base_env = base_env or (lambda: dict(os.environ))
        self._launched: Launched | None = None
        self._lock = threading.Lock()

    # --- target processes --------------------------------------------------
    def target_pids(self) -> set[int]:
        """PIDs we are allowed to observe and drive.

        The process we launched, plus any process whose executable lives inside
        the workspace (so a server restart can re-adopt a running test app).
        The owner's daily-use app in /Applications is never a target.
        """
        pids: set[int] = set()
        if self._launched and self._launched.proc.poll() is None:
            pids.add(self._launched.proc.pid)
        for pid, exe in self._processes():
            if exe.startswith("/") and self.jail.contains(exe) and self._is_app_binary(exe):
                pids.add(pid)
        return pids

    def _is_app_binary(self, exe: str) -> bool:
        p = Path(os.path.realpath(exe))
        target = Path(os.path.realpath(self.cfg.clone_dir / "target"))
        if p.parent in (target / "debug", target / "release"):
            # `integration` is the GUI integration-test runner (run_integration_test).
            return p.name in ("warp-oss", "integration")
        apps = Path(os.path.realpath(self.cfg.apps_dir))
        return p.parent.name == "MacOS" and p.parent.parent.name == "Contents" and apps in p.parents

    # --- launch ------------------------------------------------------------
    def resolve_binary(self, source: str, profile: str = "debug") -> Path:
        if source == "build":
            b = self.ws.binary_path(profile)
        elif source == "artifact":
            app = self.ws.installed_app()
            if app is None:
                raise WorkspaceError("no installed artifact; run install_artifact first")
            b = self.ws.app_executable(app)
        else:
            raise ValidationError(f"source must be one of {SOURCES}")
        b = self.jail.resolve(b)
        if not b.is_file() or not os.access(b, os.X_OK):
            raise WorkspaceError(f"{b} does not exist or is not executable (build/install first)")
        return b

    def launch(
        self,
        source: str = "build",
        env: Mapping[str, str] | None = None,
        args: list[str] | None = None,
        profile: str = "debug",
        isolate_data: bool = True,
    ) -> dict:
        if source == "build" and not self.cfg.allow_build:
            raise WorkspaceError("running locally built code is disabled (--no-build)")
        env_over = validate_env(env)
        argv_extra = validate_app_args(args)
        binary = self.resolve_binary(source, profile)
        with self._lock:
            if self._launched and self._launched.proc.poll() is None:
                raise WorkspaceError(
                    f"Warpium is already running (pid {self._launched.proc.pid}); app_quit first"
                )
            full_env = self._base_env()
            # Never inherit the server's own token-bearing or proxy-ish vars.
            for k in list(full_env):
                if k.startswith("WARPIUM_MAC_MCP"):
                    full_env.pop(k)
            if isolate_data and source == "build" and profile == "debug":
                # Honored only by debug builds; isolates UserDefaults, Application
                # Support, keychain items and ~/.warp-oss-<profile>.
                full_env.setdefault("WARP_DATA_PROFILE", self.cfg.data_profile)
            full_env.update(env_over)
            stamp = time.strftime("%Y%m%d-%H%M%S")
            out_log = self.cfg.artifacts_dir / f"app-{stamp}.log"
            logf = open(out_log, "ab")  # noqa: SIM115 - handed to the child
            try:
                proc = subprocess.Popen(
                    [str(binary), *argv_extra],
                    cwd=str(self.cfg.workspace_dir),
                    env=full_env,
                    stdin=subprocess.DEVNULL,
                    stdout=logf,
                    stderr=subprocess.STDOUT,
                    start_new_session=True,
                )
            finally:
                logf.close()
            self._launched = Launched(
                proc, source, binary, time.time(), out_log, env_over, argv_extra
            )
        time.sleep(0.5)
        if proc.poll() is not None:
            raise WorkspaceError(
                f"Warpium exited immediately with {proc.returncode}; see app_logs or {out_log}"
            )
        return {
            "pid": proc.pid,
            "binary": str(binary),
            "source": source,
            "stdout_log": str(out_log),
            "data_profile": full_env.get("WARP_DATA_PROFILE") if profile == "debug" else None,
        }

    def quit(self, timeout_s: float = 10.0) -> dict:
        pids = self.target_pids()
        if not pids:
            return {"quit": [], "note": "no Warpium process running"}
        for pid in pids:
            try:
                os.kill(pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
        deadline = time.time() + timeout_s
        remaining = set(pids)
        while remaining and time.time() < deadline:
            if self._launched:
                self._launched.proc.poll()  # reap our child so it doesn't linger as a zombie
            remaining = {p for p in remaining if _alive(p)}
            time.sleep(0.2)
        for pid in remaining:
            try:
                os.kill(pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
        if self._launched:
            try:
                self._launched.proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                pass
        return {"quit": sorted(pids), "force_killed": sorted(remaining)}

    def status(self) -> dict:
        launched = None
        if self._launched:
            rc = self._launched.proc.poll()
            launched = {
                "pid": self._launched.proc.pid,
                "running": rc is None,
                "exit_code": rc,
                "source": self._launched.source,
                "binary": str(self._launched.binary),
                "uptime_s": round(time.time() - self._launched.started_at, 1)
                if rc is None
                else None,
                "stdout_log": str(self._launched.stdout_log),
                "env_overrides": self._launched.env_overrides,
                "args": self._launched.args,
            }
        return {"launched": launched, "target_pids": sorted(self.target_pids())}

    # --- logs --------------------------------------------------------------
    def log_file(self) -> Path:
        files = [p for p in self.cfg.log_files if p.exists()]
        if not files:
            raise FileNotFoundError(
                f"no log file found (looked for {[str(p) for p in self.cfg.log_files]})"
            )
        newest = max(files, key=lambda p: p.stat().st_mtime)
        return self.jail.resolve(newest)

    def logs(
        self,
        tail: int = 200,
        grep: str | None = None,
        since: str | None = None,
        source: str = "app",
    ) -> dict:
        from .validation import parse_since, validate_grep, validate_tail

        tail = validate_tail(tail)
        grep = validate_grep(grep)
        since_ts = parse_since(since, time.time())
        if source == "app":
            path = self.log_file()
        elif source == "stdout":
            if not self._launched:
                raise FileNotFoundError("no app launched by this server yet")
            path = self.jail.resolve(self._launched.stdout_log)
        else:
            raise ValidationError("source must be 'app' (warp-oss.log) or 'stdout'")
        lines = filter_log_lines(path, tail, grep, since_ts)
        return {"path": str(path), "lines": lines}


_TS_RE = re.compile(r"^(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:?\d{2})?)")


def filter_log_lines(
    path: Path,
    tail: int,
    grep: str | None,
    since_ts: float | None,
    max_bytes: int = 32 * 1024 * 1024,
) -> str:
    """Tail/grep/since over a Warp log file. Lines start with an RFC 3339
    timestamp (env_logger default); continuation lines inherit the previous
    line's timestamp."""
    size = path.stat().st_size
    out: deque[str] = deque(maxlen=tail)
    needle = grep.lower() if grep else None
    keep_time = since_ts is None
    with open(path, "rb") as f:
        if size > max_bytes:
            f.seek(size - max_bytes)
            f.readline()
        for raw in f:
            line = raw.decode("utf-8", errors="replace").rstrip("\n")
            if since_ts is not None:
                m = _TS_RE.match(line)
                if m:
                    try:
                        dt = datetime.fromisoformat(m.group(1).replace("Z", "+00:00"))
                        if dt.tzinfo is None:
                            dt = dt.astimezone()
                        keep_time = dt.timestamp() >= since_ts
                    except ValueError:
                        pass
            if not keep_time:
                continue
            if needle is None or needle in line.lower():
                out.append(line)
    return "\n".join(out)


def _alive(pid: int) -> bool:
    try:
        os.kill(pid, 0)
        return True
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
