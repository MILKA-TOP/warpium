"""Workspace clone, cargo builds, CI artifact install and integration tests.

All of these run as background jobs in the dedicated workspace directory.
"""

from __future__ import annotations

import os
import plistlib
import re
import shutil
import subprocess
from collections.abc import Callable, Mapping
from pathlib import Path

from .config import REPO_SLUG, REPO_URL, Config
from .jobs import Job, JobManager
from .pathjail import PathJail
from .validation import (
    DEFAULT_BUILD_FEATURES,
    ValidationError,
    validate_bin,
    validate_branch,
    validate_features,
    validate_profile,
    validate_run_id,
    validate_test_name,
)


class WorkspaceError(RuntimeError):
    pass


def _git(clone: Path, *args: str, timeout: float = 60) -> subprocess.CompletedProcess:
    return subprocess.run(
        ["git", "-C", str(clone), *args], capture_output=True, text=True, timeout=timeout
    )


def normalize_remote(url: str) -> str:
    u = url.strip().removesuffix("/").removesuffix(".git").lower()
    u = re.sub(r"^git@github\.com:", "https://github.com/", u)
    return u


def check_origin(clone: Path) -> None:
    """Refuse to operate on a clone whose origin is not the fork."""
    r = _git(clone, "remote", "get-url", "origin")
    if r.returncode != 0:
        raise WorkspaceError(f"{clone} has no 'origin' remote")
    if normalize_remote(r.stdout) != normalize_remote(REPO_URL):
        raise WorkspaceError(
            f"{clone} origin is {r.stdout.strip()!r}, expected {REPO_URL}; refusing to touch it"
        )


def dirty_paths(clone: Path) -> list[str]:
    r = _git(clone, "status", "--porcelain", "--untracked-files=normal")
    if r.returncode != 0:
        raise WorkspaceError(f"git status failed: {r.stderr.strip()}")
    return [line for line in r.stdout.splitlines() if line.strip()]


def known_app_features(clone: Path) -> set[str] | None:
    """Feature names from the app crate's [features] table, or None if unreadable."""
    import tomllib

    try:
        data = tomllib.loads((clone / "app" / "Cargo.toml").read_text())
    except (OSError, ValueError):
        return None
    return set((data.get("features") or {}).keys())


def registered_integration_tests(clone: Path) -> set[str] | None:
    try:
        src = (clone / "crates" / "integration" / "src" / "bin" / "integration.rs").read_text()
    except OSError:
        return None
    return set(re.findall(r"register_test!\(\s*([a-z0-9_]+)\s*\)", src))


class Workspace:
    def __init__(
        self,
        cfg: Config,
        jobs: JobManager,
        jail: PathJail,
        base_env: Callable[[], dict[str, str]] | None = None,
    ):
        self.cfg = cfg
        self.jobs = jobs
        self.jail = jail
        self._base_env = base_env or (lambda: dict(os.environ))

    @property
    def clone(self) -> Path:
        return self.cfg.clone_dir

    def _require_build_allowed(self) -> None:
        if not self.cfg.allow_build:
            raise WorkspaceError(
                "building/running code from the repo is disabled (server started with --no-build)"
            )

    # --- repo_sync ---------------------------------------------------------
    def repo_sync(self, branch: str, discard_local_changes: bool = False) -> Job:
        branch = validate_branch(branch)
        clone = self.clone
        steps: list = []
        if not (clone / ".git").exists():
            if clone.exists() and any(clone.iterdir()):
                raise WorkspaceError(f"{clone} exists, is not empty and is not a git clone")
            steps.append(["git", "clone", "--no-checkout", REPO_URL, str(clone)])
        else:
            check_origin(clone)
            dirty = dirty_paths(clone)
            if dirty and not discard_local_changes:
                raise WorkspaceError(
                    "workspace clone has local changes; pass discard_local_changes=true to "
                    f"reset them. Changes: {dirty[:20]}"
                )
            if dirty:
                # Tracked changes and untracked (non-ignored) files only; the
                # ignored target/ dir is kept so incremental builds stay fast.
                steps.append(["git", "-C", str(clone), "reset", "--hard", "--quiet"])
                steps.append(["git", "-C", str(clone), "clean", "-fd", "--quiet"])
        refspec = f"+refs/heads/{branch}:refs/remotes/origin/{branch}"
        steps.append(["git", "-C", str(clone), "fetch", "--prune", "origin", refspec])
        steps.append(
            [
                "git",
                "-C",
                str(clone),
                "checkout",
                "--force",
                "-B",
                branch,
                f"refs/remotes/origin/{branch}",
            ]
        )
        steps.append(["git", "-C", str(clone), "log", "-1", "--format=%H %s"])
        env = self._base_env()
        env["GIT_TERMINAL_PROMPT"] = "0"
        return self.jobs.start(
            "repo_sync",
            f"sync {REPO_SLUG}@{branch}",
            steps,
            cwd=self.cfg.workspace_dir,
            env=env,
            timeout_s=3600,
        )

    # --- build -------------------------------------------------------------
    def build_command(self, profile: str, bin_name: str, features: list[str]) -> list[str]:
        cmd = ["cargo", "build", "-p", "warp", "--bin", bin_name]
        if profile == "release":
            cmd.append("--release")
        if features:
            cmd += ["--features", ",".join(features)]
        return cmd

    def build(
        self, profile: str = "debug", bin_name: str = "warp-oss", features: list[str] | None = None
    ) -> Job:
        self._require_build_allowed()
        profile = validate_profile(profile)
        bin_name = validate_bin(bin_name)
        if not (self.clone / "Cargo.toml").exists():
            raise WorkspaceError("no workspace clone yet; run repo_sync first")
        feats = validate_features(
            list(DEFAULT_BUILD_FEATURES) if features is None else features,
            known_app_features(self.clone),
        )
        cmd = self.build_command(profile, bin_name, feats)
        env = self._base_env()
        # Keep build products inside the workspace even if the owner set a
        # shared CARGO_TARGET_DIR for their own checkouts.
        env["CARGO_TARGET_DIR"] = str(self.clone / "target")
        job = self.jobs.start(
            "build", " ".join(cmd), [cmd], cwd=self.clone, env=env, timeout_s=4 * 3600
        )
        job.result["binary"] = str(self.binary_path(profile, bin_name))
        return job

    def binary_path(self, profile: str, bin_name: str = "warp-oss") -> Path:
        return self.clone / "target" / profile / bin_name

    # --- install_artifact --------------------------------------------------
    def install_artifact(self, run_id: str | int, artifact_name: str | None = None) -> Job:
        run_id = validate_run_id(run_id)
        if artifact_name is not None and not re.fullmatch(r"[A-Za-z0-9._-]{1,128}", artifact_name):
            raise ValidationError("artifact_name may only contain letters, digits, . _ -")
        dl_dir = self.jail.resolve(self.cfg.downloads_dir / run_id)
        mnt = self.jail.resolve(self.cfg.downloads_dir / f"{run_id}-mnt")
        apps_dir = self.jail.resolve(self.cfg.apps_dir)
        if dl_dir.exists():
            shutil.rmtree(dl_dir)
        dl_dir.mkdir(parents=True)
        cmd = ["gh", "run", "download", run_id, "--repo", REPO_SLUG, "--dir", str(dl_dir)]
        cmd += ["--name", artifact_name] if artifact_name else ["--pattern", "Warpium-*"]
        jail = self.jail

        def mount_and_copy(log: Callable[[str], None]) -> None:
            dmgs = sorted(p for p in dl_dir.rglob("*.dmg") if not p.is_symlink())
            if len(dmgs) != 1:
                raise WorkspaceError(f"expected exactly one .dmg in the artifact, found {dmgs}")
            dmg = jail.resolve(dmgs[0])
            mnt.mkdir(parents=True, exist_ok=True)
            r = subprocess.run(
                [
                    "hdiutil",
                    "attach",
                    "-nobrowse",
                    "-readonly",
                    "-noautoopen",
                    "-mountpoint",
                    str(mnt),
                    str(dmg),
                ],
                capture_output=True,
                text=True,
                timeout=300,
            )
            log(r.stdout + r.stderr)
            if r.returncode != 0:
                raise WorkspaceError("hdiutil attach failed")
            try:
                apps = [p for p in mnt.iterdir() if p.suffix == ".app" and not p.is_symlink()]
                if len(apps) != 1:
                    raise WorkspaceError(f"expected one .app in the dmg, found {apps}")
                dest = jail.resolve(apps_dir / apps[0].name)
                if dest.exists():
                    shutil.rmtree(dest)
                r = subprocess.run(
                    ["ditto", str(apps[0]), str(dest)], capture_output=True, text=True, timeout=600
                )
                log(r.stdout + r.stderr)
                if r.returncode != 0:
                    raise WorkspaceError("ditto failed")
            finally:
                r = subprocess.run(
                    ["hdiutil", "detach", str(mnt)], capture_output=True, text=True, timeout=120
                )
                log(r.stdout + r.stderr)
            # The fork's build is ad-hoc signed and not notarized; clear the
            # quarantine bit on our private copy only (never /Applications).
            subprocess.run(
                ["xattr", "-dr", "com.apple.quarantine", str(dest)],
                capture_output=True,
                timeout=120,
            )
            log(f"installed {dest}")
            job_ref["job"].result["app"] = str(dest)

        mount_and_copy.__name__ = "mount_dmg_and_copy_app"
        job_ref: dict = {}
        job = self.jobs.start(
            "install_artifact",
            f"install artifact from run {run_id}",
            [cmd, mount_and_copy],
            cwd=self.cfg.workspace_dir,
            env=self._base_env(),
            timeout_s=3600,
        )
        job_ref["job"] = job
        return job

    def installed_app(self) -> Path | None:
        apps = sorted(p for p in self.cfg.apps_dir.glob("*.app") if not p.is_symlink())
        return apps[0] if apps else None

    @staticmethod
    def app_executable(app: Path) -> Path:
        with open(app / "Contents" / "Info.plist", "rb") as f:
            info = plistlib.load(f)
        exe = info.get("CFBundleExecutable")
        if not isinstance(exe, str) or "/" in exe or exe.startswith("."):
            raise WorkspaceError("invalid CFBundleExecutable in Info.plist")
        return app / "Contents" / "MacOS" / exe

    # --- integration tests -------------------------------------------------
    def run_integration_test(
        self, name: str, real_display: bool = False, env_extra: Mapping[str, str] | None = None
    ) -> Job:
        self._require_build_allowed()
        name = validate_test_name(name)
        if not (self.clone / "Cargo.toml").exists():
            raise WorkspaceError("no workspace clone yet; run repo_sync first")
        known = registered_integration_tests(self.clone)
        if known is not None and name not in known:
            close = sorted(t for t in known if name in t)[:20]
            raise ValidationError(
                f"{name!r} is not registered in the integration runner; similar: {close}"
            )
        env = self._base_env()
        env["CARGO_TARGET_DIR"] = str(self.clone / "target")
        env.setdefault("RUST_BACKTRACE", "1")
        if real_display:
            env["WARPUI_USE_REAL_DISPLAY_IN_INTEGRATION_TESTS"] = "1"
        env.update(env_extra or {})
        # Documented manual runner (gui-integration-test skill): runs exactly
        # one registered test in an isolated temp HOME.
        cmd = ["cargo", "run", "-p", "integration", "--bin", "integration", "--", name]
        return self.jobs.start(
            "integration_test",
            f"integration test {name}",
            [cmd],
            cwd=self.clone,
            env=env,
            timeout_s=2 * 3600,
        )
