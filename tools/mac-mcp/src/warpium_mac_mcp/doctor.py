"""Prerequisite and permission checks (`warpium-mac-mcp doctor` / `doctor` tool)."""

from __future__ import annotations

import os
import platform as py_platform
import shutil
import socket
import stat
import subprocess
import sys
from dataclasses import asdict, dataclass

from .config import Config
from .warpctrl import discovery_dir


@dataclass
class Check:
    name: str
    ok: bool
    detail: str = ""
    fix: str = ""
    required: bool = True


def _run(argv: list[str], timeout: float = 20) -> tuple[int, str]:
    try:
        r = subprocess.run(argv, capture_output=True, text=True, timeout=timeout)
        return r.returncode, (r.stdout + r.stderr).strip()
    except (OSError, subprocess.TimeoutExpired) as e:
        return 127, str(e)


def _tool(name: str, version_args: list[str], fix: str, required: bool = True) -> Check:
    path = shutil.which(name)
    if not path:
        return Check(name, False, "not found on PATH", fix, required)
    rc, out = _run([path, *version_args])
    return Check(name, rc == 0, out.splitlines()[0] if out else path, fix if rc else "", required)


def run_checks(cfg: Config, platform_impl=None, check_port: bool = False) -> list[Check]:
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

    # Permissions (granted to the terminal app that runs the server).
    if is_mac and platform_impl is not None:
        sr = platform_impl.screen_recording_allowed()
        checks.append(
            Check(
                "Screen Recording permission",
                bool(sr),
                "granted" if sr else "not granted",
                "System Settings > Privacy & Security > Screen Recording: "
                "enable your terminal app, then restart it",
            )
        )
        ax = platform_impl.accessibility_allowed()
        checks.append(
            Check(
                "Accessibility permission (synthetic input)",
                bool(ax),
                "granted" if ax else "not granted",
                "System Settings > Privacy & Security > Accessibility: enable your terminal app, "
                "then restart it",
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

    # Build toolchain.
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
        _tool(
            "rustup",
            ["--version"],
            "curl https://sh.rustup.rs -sSf | sh (then reopen the terminal)",
        )
    )
    checks.append(_tool("cargo", ["--version"], "install Rust via rustup"))
    if (cfg.clone_dir / "rust-toolchain.toml").exists() and shutil.which("rustup"):
        rc, out = _run(["rustup", "show", "active-toolchain"], timeout=60)
        checks.append(
            Check(
                "rust toolchain for the clone",
                rc == 0 and "1.92" in out,
                out,
                f"cd {cfg.clone_dir} && rustup toolchain install 1.92.0",
            )
        )
    checks.append(_tool("protoc", ["--version"], "brew install protobuf"))
    if is_mac:
        rc, out = _run(["xcode-select", "-p"])
        checks.append(
            Check(
                "Xcode (xcode-select -p)",
                rc == 0 and "Xcode" in out,
                out,
                "install Xcode 26 from the App Store, then "
                "sudo xcode-select -s /Applications/Xcode.app",
            )
        )
        rc, out = _run(["xcrun", "-f", "metal"])
        checks.append(
            Check(
                "Metal toolchain (xcrun -f metal)",
                rc == 0,
                out,
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
        mark = "ok  " if c.ok else ("FAIL" if c.required else "warn")
        line = f"[{mark}] {c.name}: {c.detail}"
        if not c.ok and c.fix:
            line += f"\n        fix: {c.fix}"
        lines.append(line)
    return "\n".join(lines)


def checks_as_dicts(checks: list[Check]) -> list[dict]:
    return [asdict(c) for c in checks]
