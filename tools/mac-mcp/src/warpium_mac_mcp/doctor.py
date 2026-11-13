"""Prerequisite and permission checks (`warpium-mac-mcp doctor` / `doctor` tool)."""

from __future__ import annotations

import json
import os
import platform as py_platform
import shutil
import socket
import stat
import subprocess
import sys
import time
from collections.abc import Callable
from contextvars import ContextVar
from dataclasses import asdict, dataclass
from pathlib import Path

from .config import Config
from .warpctrl import discovery_dir


@dataclass
class Check:
    name: str
    ok: bool
    detail: str = ""
    fix: str = ""
    required: bool = True
    # "core": needed to drive an installed artifact (screenshots, input, logs).
    # "build": only needed for local builds (build, run_integration_test);
    #          never affects all_required_ok, summarized as build_ready.
    # "info": informational.
    category: str = "core"


# Cold xcrun/protoc/rustup invocations can take well over 20 s on the first
# call after boot (xcrun populates its cache, Gatekeeper scans the binary).
TOOL_TIMEOUT_S = 60
TIMEOUT_RC = -999
RETRY_FIX = "slow first run: run doctor again (it usually succeeds once warm)"


# run_checks(tool_timeout_s=...) overrides the default for one run (the MCP
# doctor tool uses a shorter one so a call stays well inside client timeouts).
_timeout: ContextVar[float] = ContextVar("doctor_tool_timeout", default=TOOL_TIMEOUT_S)


def _run(argv: list[str], timeout: float | None = None) -> tuple[int, str]:
    if timeout is None:
        timeout = _timeout.get()
    try:
        r = subprocess.run(argv, capture_output=True, text=True, timeout=timeout)
        return r.returncode, (r.stdout + r.stderr).strip()
    except subprocess.TimeoutExpired:
        return TIMEOUT_RC, f"timed out after {timeout:.0f} s"
    except OSError as e:
        return 127, str(e)


def _tool(
    name: str,
    version_args: list[str],
    fix: str,
    required: bool = True,
    category: str = "core",
) -> Check:
    path = shutil.which(name)
    if not path:
        return Check(name, False, "not found on PATH", fix, required, category)
    rc, out = _run([path, *version_args])
    if rc == TIMEOUT_RC:
        return Check(name, False, f"{path}: {out}", RETRY_FIX, required, category)
    detail = out.splitlines()[0] if out else path
    return Check(name, rc == 0, detail, fix if rc else "", required, category)


def _build_tool(name: str, version_args: list[str], fix: str) -> Check:
    return _tool(name, version_args, fix, required=False, category="build")


def _build_command(name: str, argv: list[str], ok: Callable[[str], bool], fix: str) -> Check:
    rc, out = _run(argv)
    if rc == TIMEOUT_RC:
        return Check(name, False, out, RETRY_FIX, False, "build")
    return Check(name, rc == 0 and ok(out), out, fix, False, "build")


def in_virtual_machine() -> bool | None:
    """macOS sets kern.hv_vmm_present=1 inside a Virtualization.framework guest."""
    if sys.platform != "darwin":
        return None
    rc, out = _run(["/usr/sbin/sysctl", "-n", "kern.hv_vmm_present"], timeout=10)
    return out.strip() == "1" if rc == 0 else None


def summarize(checks: list[Check]) -> dict:
    return {
        "all_required_ok": all(c.ok for c in checks if c.required),
        "build_ready": all(c.ok for c in checks if c.category == "build"),
    }


def server_permissions_path(cfg: Config) -> Path:
    return cfg.config_dir / "server-permissions.json"


def record_server_permissions(cfg: Config, platform_impl) -> dict | None:
    """Called by the server process (at start and on each doctor call): saves
    its own TCC state, so `warpium-mac-mcp doctor` run from another context
    (an SSH session into the VM) can report what the server actually has."""
    if platform_impl is None:
        return None
    from .app import executable_path

    def tri(v: bool | None) -> bool | None:
        return None if v is None else bool(v)

    data = {
        "pid": os.getpid(),
        # With the pid, identifies this process even after pid reuse.
        "exe": executable_path(os.getpid()),
        "checked_at": time.strftime("%Y-%m-%dT%H:%M:%S%z"),
        # None = macOS didn't say (API unavailable), not "denied".
        "screen_recording": tri(platform_impl.screen_recording_allowed()),
        "accessibility": tri(platform_impl.accessibility_allowed()),
    }
    path = server_permissions_path(cfg)
    try:
        path.parent.mkdir(parents=True, exist_ok=True)
        tmp = path.with_suffix(".tmp")
        fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC | os.O_NOFOLLOW, 0o600)
        with os.fdopen(fd, "w") as f:
            json.dump(data, f)
        os.replace(tmp, path)
    except OSError:
        pass  # diagnostics only
    return data


def clear_server_permissions(cfg: Config) -> None:
    """Called by the server on shutdown: removes its own record."""
    path = server_permissions_path(cfg)
    try:
        if json.loads(path.read_text()).get("pid") == os.getpid():
            path.unlink()
    except (OSError, ValueError, AttributeError):
        pass


def _same_process(pid: int, exe: str | None) -> bool:
    """True if ``pid`` is alive and still runs ``exe`` (guards against pid reuse)."""
    from .app import executable_path

    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        pass
    return exe is not None and executable_path(pid) == exe


def load_server_permissions(
    cfg: Config, alive: Callable[[int, str | None], bool] = _same_process
) -> dict | None:
    """The running server's last permission report, or None if the server that
    wrote it is gone."""
    try:
        data = json.loads(server_permissions_path(cfg).read_text())
    except (OSError, ValueError):
        return None
    pid = data.get("pid") if isinstance(data, dict) else None
    if not isinstance(pid, int) or pid == os.getpid() or not alive(pid, data.get("exe")):
        return None
    return data


def run_checks(
    cfg: Config,
    platform_impl=None,
    check_port: bool = False,
    server_permissions: dict | None = None,
    tool_timeout_s: float = TOOL_TIMEOUT_S,
) -> list[Check]:
    token = _timeout.set(tool_timeout_s)
    try:
        return _run_checks(cfg, platform_impl, check_port, server_permissions)
    finally:
        _timeout.reset(token)


def _run_checks(
    cfg: Config,
    platform_impl,
    check_port: bool,
    server_permissions: dict | None,
) -> list[Check]:
    checks: list[Check] = []
    is_mac = sys.platform == "darwin"
    checks.append(
        Check(
            "macOS",
            is_mac,
            f"{py_platform.system()} {py_platform.mac_ver()[0]}",
            "the server must run on the Mac host, not in the container",
        )
    )
    checks.append(Check("python>=3.11", sys.version_info >= (3, 11), sys.version.split()[0]))
    vm = in_virtual_machine()
    if vm is not None:
        checks.append(
            Check(
                "environment",
                True,
                "macOS VM guest (isolated from your desktop)"
                if vm
                else "macOS host (input acts on your real desktop; the Tart VM setup is safer)",
                required=False,
                category="info",
            )
        )

    # Permissions belong to the app that runs the server (its terminal app).
    # A doctor run from another context (e.g. over SSH into the VM) has other
    # grants than the server, so a running server's own report wins.
    if is_mac and (server_permissions is not None or platform_impl is not None):
        if server_permissions is not None:
            sr = server_permissions.get("screen_recording")
            ax = server_permissions.get("accessibility")
            source = (
                f" (reported by the running server, pid {server_permissions.get('pid')}, "
                f"at {server_permissions.get('checked_at')})"
            )
        else:
            sr = platform_impl.screen_recording_allowed()
            ax = platform_impl.accessibility_allowed()
            source = ""

        def state(v: bool | None) -> str:
            return (
                "unknown (macOS did not report it)"
                if v is None
                else ("granted" if v else "not granted")
            )

        checks.append(
            Check(
                "Screen Recording permission",
                bool(sr),
                state(sr) + source,
                "System Settings > Privacy & Security > Screen & System Audio Recording: "
                "enable the app that runs the server (your terminal app; in the VM run "
                "vm/grant-permissions.sh on the host), then restart the server",
            )
        )
        checks.append(
            Check(
                "Accessibility permission (synthetic input)",
                bool(ax),
                state(ax) + source,
                "System Settings > Privacy & Security > Accessibility: enable the app that "
                "runs the server, then restart it. Still 'not granted' after enabling: run "
                "`tccutil reset Accessibility <bundle id>` (e.g. com.apple.Terminal) and add "
                "the app again",
            )
        )
    elif is_mac:
        checks.append(
            Check(
                "pyobjc (Quartz)",
                False,
                "import failed",
                "run via `uv run warpium-mac-mcp` so dependencies are installed",
            )
        )

    # git/gh: repo_sync and install_artifact. The Rust/Xcode toolchain is only
    # needed for local builds (category "build", reported as build_ready).
    checks.append(_tool("git", ["--version"], "xcode-select --install"))
    checks.append(_tool("gh", ["--version"], "brew install gh && gh auth login", required=False))
    if shutil.which("gh"):
        rc, out = _run(["gh", "auth", "status"])
        checks.append(
            Check(
                "gh auth",
                rc == 0,
                out.splitlines()[0] if out else "",
                "gh auth login (needed for install_artifact)",
                required=False,
            )
        )
    checks.append(
        _build_tool(
            "rustup",
            ["--version"],
            "curl https://sh.rustup.rs -sSf | sh (then reopen the terminal)",
        )
    )
    checks.append(_build_tool("cargo", ["--version"], "install Rust via rustup"))
    if (cfg.clone_dir / "rust-toolchain.toml").exists() and shutil.which("rustup"):
        checks.append(
            _build_command(
                "rust toolchain for the clone",
                ["rustup", "show", "active-toolchain"],
                lambda out: "1.92" in out,
                f"cd {cfg.clone_dir} && rustup toolchain install 1.92.0",
            )
        )
    checks.append(_build_tool("protoc", ["--version"], "brew install protobuf"))
    if is_mac:
        checks.append(
            _build_command(
                "Xcode (xcode-select -p)",
                ["xcode-select", "-p"],
                lambda out: "Xcode" in out,
                "install Xcode 26 from the App Store, then "
                "sudo xcode-select -s /Applications/Xcode.app",
            )
        )
        checks.append(
            _build_command(
                "Metal toolchain (xcrun -f metal)",
                ["xcrun", "-f", "metal"],
                lambda out: True,
                "xcodebuild -downloadComponent MetalToolchain",
            )
        )
        for name, path in (
            ("screencapture", "/usr/sbin/screencapture"),
            ("sips", "/usr/bin/sips"),
            ("lsappinfo", "/usr/bin/lsappinfo"),
            ("hdiutil", "/usr/bin/hdiutil"),
        ):
            checks.append(Check(name, os.path.exists(path), path))

    # Workspace and files.
    clone = cfg.clone_dir
    checks.append(
        Check(
            "workspace dir",
            cfg.workspace_dir.is_dir(),
            str(cfg.workspace_dir),
            "created on first server start",
            required=False,
        )
    )
    checks.append(
        Check(
            "workspace clone",
            (clone / ".git").exists(),
            str(clone),
            "call the repo_sync tool (or git clone the fork there)",
            required=False,
        )
    )
    tp = cfg.token_path
    if tp.exists():
        mode = stat.S_IMODE(os.lstat(tp).st_mode)
        checks.append(
            Check(
                "token file permissions",
                mode & 0o077 == 0 and not tp.is_symlink(),
                f"{tp} mode {oct(mode)}",
                f"chmod 600 {tp}",
            )
        )
    else:
        checks.append(
            Check(
                "token file",
                False,
                f"{tp} missing",
                "created on first server start",
                required=False,
            )
        )
    logs = [str(p) for p in cfg.log_files if p.exists()]
    checks.append(
        Check(
            "Warpium log file",
            bool(logs),
            ", ".join(logs) or ", ".join(str(p) for p in cfg.log_files),
            "appears after Warpium has run once",
            required=False,
        )
    )
    dd = discovery_dir()
    checks.append(
        Check(
            "local-control discovery dir",
            dd.is_dir(),
            str(dd),
            "appears once a Warpium built with warp_control_cli runs with "
            "Settings > Scripting enabled",
            required=False,
        )
    )

    if check_port:
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
            try:
                s.bind((cfg.host, cfg.port))
                checks.append(Check(f"port {cfg.host}:{cfg.port} free", True))
            except OSError as e:
                checks.append(
                    Check(
                        f"port {cfg.host}:{cfg.port} free",
                        False,
                        str(e),
                        "pick another port with --port",
                    )
                )
    return checks


def format_checks(checks: list[Check]) -> str:
    lines = []
    for c in checks:
        if c.ok:
            mark = "ok  "
        elif c.required:
            mark = "FAIL"
        elif c.category == "build":
            mark = "bld "  # missing, but only needed for local builds
        else:
            mark = "warn"
        line = f"[{mark}] {c.name}: {c.detail}"
        if not c.ok and c.fix:
            line += f"\n        fix: {c.fix}"
        lines.append(line)
    s = summarize(checks)
    lines.append("")
    lines.append(
        "required (artifacts, screenshots, input): "
        + ("all ok" if s["all_required_ok"] else "MISSING, see FAIL lines")
    )
    lines.append(
        "local builds (build, run_integration_test): "
        + ("ready" if s["build_ready"] else "not ready, see 'bld' lines (fine with artifacts)")
    )
    return "\n".join(lines)


def checks_as_dicts(checks: list[Check]) -> list[dict]:
    return [asdict(c) for c in checks]
