"""Server configuration, on-disk locations and bearer-token management."""

from __future__ import annotations

import os
import secrets
import stat
from collections.abc import Mapping
from dataclasses import dataclass, field
from pathlib import Path

# Arbitrary high port, away from common dev-server ports (8000, 8080, 8765...).
DEFAULT_PORT = 17654
DEFAULT_HOST = "127.0.0.1"
REPO_SLUG = "MILKA-TOP/warpium"
REPO_URL = f"https://github.com/{REPO_SLUG}.git"

# Channel string published by the `warp-oss` binary in local-control discovery
# records (`Channel::Oss` Display impl in crates/warp_core/src/channel/mod.rs).
OSS_CHANNEL = "warp-oss"

# `logfile_name` from app/src/bin/oss.rs. The GUI writes it to ~/Library/Logs/
# on macOS (crates/warp_logging/src/native.rs `init_log_directory`).
DEFAULT_LOGFILE_NAMES = ("warp-oss.log",)

# Debug builds honor WARP_DATA_PROFILE (crates/warp_core/src/channel/state.rs),
# which isolates UserDefaults, Application Support, keychain entries and the
# ~/.warp-oss-<profile> config dir from the owner's normal Warpium data.
DEFAULT_DATA_PROFILE = "mcp-test"


# Environment passed to builds, tests and the launched app. Everything else in
# the server's environment (API keys, cloud credentials, tokens) is dropped, so
# code compiled from the fork (build.rs, the app itself) doesn't inherit it.
_ENV_PASSTHROUGH = frozenset(
    {
        "PATH",
        "HOME",
        "USER",
        "LOGNAME",
        "SHELL",
        "TMPDIR",
        "TERM",
        "LANG",
        "CARGO_HOME",
        "RUSTUP_HOME",
        "DEVELOPER_DIR",
        "SDKROOT",
        "MACOSX_DEPLOYMENT_TARGET",
        "PROTOC",
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "NO_PROXY",
        "http_proxy",
        "https_proxy",
        "no_proxy",
        "__CF_USER_TEXT_ENCODING",
    }
)


def scrubbed_env(environ: Mapping[str, str] | None = None) -> dict[str, str]:
    env = os.environ if environ is None else environ
    return {k: v for k, v in env.items() if k in _ENV_PASSTHROUGH or k.startswith("LC_")}


def default_config_dir() -> Path:
    return Path.home() / ".config" / "warpium-mac-mcp"


def default_workspace_dir() -> Path:
    return Path.home() / "warpium-mac-test"


@dataclass(frozen=True)
class Config:
    host: str = DEFAULT_HOST
    port: int = DEFAULT_PORT
    config_dir: Path = field(default_factory=default_config_dir)
    workspace_dir: Path = field(default_factory=default_workspace_dir)
    logs_dir: Path = field(default_factory=lambda: Path.home() / "Library" / "Logs")
    logfile_names: tuple[str, ...] = DEFAULT_LOGFILE_NAMES
    data_profile: str = DEFAULT_DATA_PROFILE
    # Restricted modes. Both default to enabled because driving and rebuilding
    # the app is the point of the server; SECURITY.md explains what they mean.
    allow_input: bool = True
    allow_build: bool = True
    extra_allowed_hosts: tuple[str, ...] = ()

    @property
    def token_path(self) -> Path:
        return self.config_dir / "token"

    @property
    def audit_log_path(self) -> Path:
        return self.config_dir / "audit.log"

    @property
    def clone_dir(self) -> Path:
        return self.workspace_dir / "warpium"

    @property
    def artifacts_dir(self) -> Path:
        return self.workspace_dir / "artifacts"

    @property
    def apps_dir(self) -> Path:
        return self.workspace_dir / "apps"

    @property
    def screenshots_dir(self) -> Path:
        return self.artifacts_dir / "screenshots"

    @property
    def jobs_dir(self) -> Path:
        return self.artifacts_dir / "jobs"

    @property
    def downloads_dir(self) -> Path:
        return self.workspace_dir / "downloads"

    @property
    def log_files(self) -> list[Path]:
        return [self.logs_dir / name for name in self.logfile_names]

    def allowed_hosts(self) -> list[str]:
        """Host header values accepted by the HTTP transport (DNS-rebinding guard)."""
        hosts = [
            "127.0.0.1",
            "localhost",
            "[::1]",
            "host.docker.internal",
            *self.extra_allowed_hosts,
        ]
        if self.host not in ("127.0.0.1", "localhost", "::1"):
            hosts.append(self.host)
        return [f"{h}:{self.port}" for h in hosts]

    def ensure_dirs(self) -> None:
        _mkdir_private(self.config_dir)
        for d in (
            self.workspace_dir,
            self.artifacts_dir,
            self.apps_dir,
            self.screenshots_dir,
            self.jobs_dir,
            self.downloads_dir,
        ):
            d.mkdir(parents=True, exist_ok=True)


def _mkdir_private(path: Path) -> None:
    path.mkdir(parents=True, exist_ok=True)
    os.chmod(path, 0o700)


def load_or_create_token(path: Path) -> str:
    """Returns the bearer token, creating it (mode 0600) on first run.

    Refuses to use a token file that is a symlink or readable by other users,
    since anyone who can read it can drive the owner's machine.
    """
    _mkdir_private(path.parent)
    if not path.exists() and not path.is_symlink():
        token = secrets.token_urlsafe(32)
        fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
        with os.fdopen(fd, "w") as f:
            f.write(token + "\n")
        return token

    st = os.lstat(path)
    if stat.S_ISLNK(st.st_mode) or not stat.S_ISREG(st.st_mode):
        raise PermissionError(f"{path} must be a regular file, not a symlink")
    if st.st_uid != os.getuid():
        raise PermissionError(f"{path} is not owned by the current user")
    if st.st_mode & 0o077:
        raise PermissionError(
            f"{path} is accessible by group/others (mode {oct(st.st_mode & 0o777)}); "
            f"run: chmod 600 {path}"
        )
    token = path.read_text().strip()
    if len(token) < 32:
        raise ValueError(f"{path} does not contain a valid token; delete it to regenerate")
    return token
