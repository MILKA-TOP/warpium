"""MCP tool definitions and HTTP app assembly."""

from __future__ import annotations

import functools
import json
import logging
import sys
from collections.abc import Callable
from dataclasses import dataclass
from typing import Any, Literal

import anyio
from mcp.server.fastmcp import FastMCP, Image
from mcp.server.transport_security import TransportSecuritySettings

from .app import AppManager
from .audit import AuditLog, audited
from .auth import BearerAuthMiddleware
from .config import OSS_CHANNEL, Config
from .doctor import checks_as_dicts, record_server_permissions, run_checks, summarize
from .jobs import JobManager
from .pathjail import PathJail
from .platform import Platform
from .ui import UiController
from .validation import ValidationError, validate_grep, validate_tail
from .warpctrl import (
    CATALOG,
    LocalControlClient,
    WarpctrlError,
    discovery_dir,
    list_instances,
    validate_action,
    validate_target,
)
from .workspace import Workspace

log = logging.getLogger(__name__)

INSTRUCTIONS = """\
Drives a Warpium (Warp OSS fork) build on the owner's Mac.
Typical loop: repo_sync(branch) -> build() -> job_status/job_logs until done ->
app_launch() -> warpctrl/list_targets (preferred) or screenshot + click/type_text/press_keys
-> app_logs -> app_quit. Input tools only act while a Warpium window is frontmost; call
focus_app first. Coordinates for click/scroll are window-relative points (see screenshot
metadata). warpctrl needs a build with the warp_control_cli feature (the build() default) and
Settings > Scripting enabled once in the test app.
"""


@dataclass
class Services:
    cfg: Config
    audit: AuditLog
    jobs: JobManager
    jail: PathJail
    workspace: Workspace
    app: AppManager
    ui: UiController
    lc: LocalControlClient
    platform: Platform | None
    # Workspace only, without the log-file exceptions: used for paths handed
    # to the app (file.open), which must not open the shared warp-oss.log.
    workspace_jail: PathJail | None = None


def build_services(cfg: Config, platform: Platform | None) -> Services:
    cfg.ensure_dirs()
    audit = AuditLog(cfg.audit_log_path)
    jail = PathJail([cfg.workspace_dir], files=cfg.log_files)
    jobs = JobManager(cfg.jobs_dir)
    ws = Workspace(cfg, jobs, jail)
    app = AppManager(cfg, ws, jail)
    if platform is None:
        from .platform import WindowInfo

        class _Unavailable:
            def __getattr__(self, name: str) -> Callable[..., Any]:
                def _fail(*_a: Any, **_k: Any) -> Any:
                    raise RuntimeError("macOS window/input APIs unavailable (not on macOS?)")

                return _fail

            def windows(self) -> list[WindowInfo]:
                return []

        ui_platform: Any = _Unavailable()
    else:
        ui_platform = platform
    ui = UiController(
        ui_platform,
        app.target_pids,
        allow_input=cfg.allow_input,
        screenshots_dir=cfg.screenshots_dir,
    )
    lc = LocalControlClient(discovery_dir())
    return Services(
        cfg,
        audit,
        jobs,
        jail,
        ws,
        app,
        ui,
        lc,
        platform,
        workspace_jail=PathJail([cfg.workspace_dir]),
    )


async def _thread(fn: Callable[..., Any], *args: Any, **kwargs: Any) -> Any:
    return await anyio.to_thread.run_sync(functools.partial(fn, *args, **kwargs))


def _select_instance(s: Services) -> Any:
    """The local-control instance belonging to our target process."""
    pids = s.app.target_pids()
    insts = [i for i in list_instances(discovery_dir(), OSS_CHANNEL) if i.pid in pids]
    if not insts:
        all_oss = list_instances(discovery_dir(), OSS_CHANNEL)
        raise WarpctrlError(
            "no local-control instance for the Warpium process under test "
            f"(target pids {sorted(pids)}; warp-oss instances seen: "
            f"{[i.pid for i in all_oss]}). Is the app running, built with warp_control_cli, "
            "and is Settings > Scripting enabled?",
            "no_instance",
        )
    if len(insts) > 1:
        raise WarpctrlError(
            f"multiple instances: {[i.public() for i in insts]}", "ambiguous_instance"
        )
    return insts[0]


def run_warpctrl(s: Services, action: str, params: dict | None, target: dict | None) -> Any:
    clean_params = validate_action(
        action, params, allow_input=s.cfg.allow_input, file_jail=s.workspace_jail
    )
    clean_target = validate_target(target)
    if action == "instance.list":
        pids = s.app.target_pids()
        return {
            "instances": [
                i.public() for i in list_instances(discovery_dir(), OSS_CHANNEL) if i.pid in pids
            ]
        }
    inst = _select_instance(s)
    # Like the Rust client's probe_instance: the app that answers must report
    # the instance id and pid we selected (a stale/forged discovery record
    # can't redirect actions to another Warp instance).
    s.lc.verify_identity(inst)
    return s.lc.send(inst, action, clean_params, clean_target)


def create_server(s: Services) -> FastMCP:
    cfg = s.cfg
    mcp = FastMCP(
        "warpium-mac",
        instructions=INSTRUCTIONS,
        host=cfg.host,
        port=cfg.port,
        stateless_http=False,
        transport_security=TransportSecuritySettings(
            enable_dns_rebinding_protection=True,
            allowed_hosts=cfg.allowed_hosts(),
            allowed_origins=[],  # no browser origins, ever
        ),
    )
    A = functools.partial(audited, s.audit)

    # ---------------------------------------------------------------- doctor
    @mcp.tool()
    @A()
    async def doctor() -> dict:
        """Check prerequisites, macOS permissions (Screen Recording, Accessibility), workspace
        and log locations. all_required_ok covers what artifacts/screenshots/input need;
        build_ready covers the local-build toolchain (rustup, cargo, protoc, Xcode/Metal),
        which is only needed for build and run_integration_test."""
        checks = await _thread(run_checks, cfg, s.platform)
        await _thread(record_server_permissions, cfg, s.platform)
        return {
            "checks": checks_as_dicts(checks),
            **summarize(checks),
            "input_enabled": cfg.allow_input,
            "build_enabled": cfg.allow_build,
        }

    # ------------------------------------------------------ workspace & build
    @mcp.tool()
    @A()
    async def repo_sync(branch: str, discard_local_changes: bool = False) -> dict:
        """Fetch MILKA-TOP/warpium into the workspace clone (~/warpium-mac-test/warpium) and
        check out `branch` exactly at origin. Refuses if the clone has local changes unless
        discard_local_changes=true (then resets tracked changes and removes untracked,
        non-ignored files; target/ is kept). Runs as a background job; returns a job id."""
        job = await _thread(s.workspace.repo_sync, branch, discard_local_changes)
        return job.summary()

    @mcp.tool()
    @A()
    async def build(
        profile: Literal["debug", "release"] = "debug",
        bin: Literal["warp-oss"] = "warp-oss",
        features: list[str] | None = None,
    ) -> dict:
        """`cargo build -p warp --bin warp-oss` in the workspace clone as a background job.
        features defaults to ["gui", "warp_control_cli"] (warp_control_cli enables warpctrl);
        names are checked against app/Cargo.toml. Poll with job_status/job_logs."""
        job = await _thread(s.workspace.build, profile, bin, features)
        return job.summary()

    @mcp.tool()
    @A()
    async def job_status(job_id: str | None = None) -> dict:
        """Status of one job, or of all jobs when job_id is omitted."""
        if job_id is None:
            return {"jobs": s.jobs.list()}
        return s.jobs.get(job_id).summary()

    @mcp.tool()
    @A()
    async def job_logs(job_id: str, tail: int = 200, grep: str | None = None) -> dict:
        """Last `tail` lines (max 5000) of a job's output; `grep` keeps only lines containing
        that substring (case-insensitive)."""
        tail = validate_tail(tail)
        grep = validate_grep(grep)
        text = await _thread(s.jobs.logs, job_id, tail, grep)
        return {"job": s.jobs.get(job_id).summary(), "lines": text}

    @mcp.tool()
    @A()
    async def job_cancel(job_id: str) -> dict:
        """Cancel a running job (kills its whole process group)."""
        return (await _thread(s.jobs.cancel, job_id)).summary()

    @mcp.tool()
    @A()
    async def install_artifact(run_id: str, artifact_name: str | None = None) -> dict:
        """Download the Warpium .dmg from a successful run of the fork's warpium-build.yml
        (the run must be built from MILKA-TOP/warpium itself), mount it read-only and copy the
        .app into ~/warpium-mac-test/apps/. Never touches /Applications. Background job.
        Without artifact_name, the run's artifacts are listed and the single non-expired
        'Warpium-*' one is used (e.g. run 43262238821 -> Warpium-a70e8571-macos-arm64).
        Disabled with --no-build."""
        job = await _thread(s.workspace.install_artifact, run_id, artifact_name)
        return job.summary()

    @mcp.tool()
    @A()
    async def run_integration_test(name: str, real_display: bool = False) -> dict:
        """Run one registered GUI integration test from crates/integration via its manual runner
        (`cargo run -p integration --bin integration -- <name>`) as a background job."""
        job = await _thread(s.workspace.run_integration_test, name, real_display)
        return job.summary()

    # --------------------------------------------------------- app lifecycle
    @mcp.tool()
    @A()
    async def app_launch(
        source: Literal["build", "artifact"] = "build",
        env: dict[str, str] | None = None,
        args: list[str] | None = None,
        profile: Literal["debug", "release"] = "debug",
    ) -> dict:
        """Launch Warpium: source="build" runs target/<profile>/warp-oss from the workspace clone,
        "artifact" runs the app installed by install_artifact. env accepts only RUST_LOG,
        RUST_BACKTRACE, RUST_LIB_BACKTRACE. args accepts only --flag[=value]. The server always
        sets WARP_DATA_PROFILE=mcp-test; debug builds honor it (own settings, keychain items and
        data). Release builds and artifacts ignore it and share oss-channel data."""
        return await _thread(s.app.launch, source, env, args, profile)

    @mcp.tool()
    @A()
    async def app_quit() -> dict:
        """Quit the Warpium process(es) under test (SIGTERM, then SIGKILL after 10 s)."""
        return await _thread(s.app.quit)

    @mcp.tool()
    @A()
    async def app_status() -> dict:
        """Launched process info, target PIDs, visible Warpium windows, whether Warpium is
        frontmost, and the local-control (warpctrl) instance if any."""
        st = await _thread(s.app.status)
        try:
            wins = await _thread(s.ui.list_windows)
            st["windows"] = [w.public() for w in wins]
        except Exception as e:  # noqa: BLE001
            st["windows_error"] = str(e)
        if s.platform is not None:
            try:
                st["frontmost_pid"] = await _thread(s.platform.frontmost_pid)
                st["warpium_frontmost"] = st["frontmost_pid"] in st["target_pids"]
            except Exception as e:  # noqa: BLE001
                st["frontmost_error"] = str(e)
        pids = set(st["target_pids"])
        st["local_control_instances"] = [
            i.public() for i in list_instances(discovery_dir(), OSS_CHANNEL) if i.pid in pids
        ]
        return st

    @mcp.tool()
    @A()
    async def app_logs(
        tail: int = 200,
        grep: str | None = None,
        since: str | None = None,
        source: Literal["app", "stdout"] = "app",
    ) -> dict:
        """Read the Warpium log (~/Library/Logs/warp-oss.log) or the launched process's
        stdout/stderr capture. since: '15m', '2h', or an ISO-8601 timestamp."""
        return await _thread(s.app.logs, tail, grep, since, source)

    # ------------------------------------------------------------ observation
    @mcp.tool()
    @A()
    async def list_windows() -> dict:
        """On-screen Warpium windows (front-to-back) with window ids and bounds in screen points."""
        wins = await _thread(s.ui.list_windows)
        return {"windows": [dict(index=i, **w.public()) for i, w in enumerate(wins)]}

    @mcp.tool(structured_output=False)
    @A()
    async def screenshot(window: str = "frontmost-warpium", max_width: int = 1600) -> list:
        """PNG of one Warpium window only (never the full screen), downscaled to max_width.
        window: 'frontmost-warpium', an index from list_windows, or 'id:<window_id>'.
        Returns the image plus bounds and the pixel/point scale for click coordinates."""
        win: str | int = int(window) if window.isdigit() else window
        shot = await _thread(s.ui.screenshot, win, max_width)
        return [Image(path=shot.path), json.dumps(shot.metadata(), indent=1)]

    # ------------------------------------------------------------------ input
    @mcp.tool()
    @A()
    async def focus_app() -> dict:
        """Bring the Warpium window under test to the front (required before input tools)."""
        return await _thread(s.ui.focus)

    @mcp.tool()
    @A()
    async def click(
        x: float,
        y: float,
        button: Literal["left", "right", "middle"] = "left",
        count: int = 1,
        window: str = "frontmost-warpium",
        from_screenshot: bool = False,
    ) -> dict:
        """Click at window-relative point (x, y). With from_screenshot=true, x/y are pixel
        coordinates in the last screenshot of that window. Refused unless Warpium is frontmost
        and the point is not covered by another app's window."""
        win: str | int = int(window) if window.isdigit() else window
        return await _thread(s.ui.click, x, y, button, count, win, from_screenshot)

    @mcp.tool()
    @A()
    async def scroll(
        x: float,
        y: float,
        dy: int = -3,
        dx: int = 0,
        window: str = "frontmost-warpium",
        from_screenshot: bool = False,
    ) -> dict:
        """Scroll by dy/dx lines (negative dy scrolls down) with the pointer at window
        point (x, y)."""
        win: str | int = int(window) if window.isdigit() else window
        return await _thread(s.ui.scroll, x, y, dx, dy, win, from_screenshot)

    @mcp.tool()
    @A(redact_when=("redact", ["text"]))
    async def type_text(text: str, redact: bool = False) -> dict:
        """Type text into the frontmost Warpium window. '\\n' is sent as Return (which runs the
        command in a terminal), '\\t' as Tab. redact=true keeps the text out of the audit log."""
        return await _thread(s.ui.type_text, text)

    @mcp.tool()
    @A()
    async def press_keys(combo: str) -> dict:
        """Press a key combination in Warpium, e.g. 'cmd+t', 'cmd+shift+]', 'ctrl+c', 'enter',
        'escape', 'up'. System shortcuts (cmd+space, cmd+tab, ...) are refused."""
        return await _thread(s.ui.press_keys, combo)

    # -------------------------------------------------------------- warpctrl
    @mcp.tool()
    @A(redact_when=("redact", ["params"]))
    async def warpctrl(
        action: str,
        params: dict[str, Any] | None = None,
        target: dict[str, Any] | None = None,
        redact: bool = False,
    ) -> dict:
        """Invoke one Warp local-control catalog action on the Warpium under test, e.g.
        action='tab.create' params={'tab_type':'terminal'}; action='pane.split'
        params={'direction':'right'}; action='setting.get' params={'key':'...'}.
        target selects a window/tab/pane/session: {'tab': {'type':'index','index':1}} or
        {'pane': {'type':'id','id':'...'}} or {'window': {'type':'active'}}.
        Call warpctrl_catalog for the action list and parameter shapes."""
        try:
            data = await _thread(run_warpctrl, s, action, params, target)
        except WarpctrlError as e:
            return {"ok": False, **e.to_dict()}
        return {"ok": True, "data": data}

    @mcp.tool()
    @A()
    async def warpctrl_catalog() -> dict:
        """The allowlisted local-control actions and their parameter spec names."""
        from .warpctrl import _ENUMS, _PARAM_SPECS, INPUT_ACTIONS

        specs = {
            k: {
                "required": {n: _tname(t) for n, t in r.items()},
                "optional": {n: _tname(t) for n, t in o.items()},
            }
            for k, (r, o) in _PARAM_SPECS.items()
        }
        return {
            "actions": CATALOG,
            "param_specs": specs,
            "enums": {k: sorted(v) for k, v in _ENUMS.items()},
            "disabled": sorted(INPUT_ACTIONS) if not cfg.allow_input else [],
            "notes": [
                "file.open: path must be absolute and inside the workspace",
                "session metadata has no cwd field in this protocol version",
            ],
        }

    @mcp.tool()
    @A()
    async def list_targets() -> dict:
        """Windows, tabs, panes and sessions of the Warpium under test (via warpctrl), plus the
        active chain. Sessions carry ids/indices only; the protocol exposes no cwd."""
        out: dict[str, Any] = {}
        for key, action in (
            ("active", "app.active"),
            ("windows", "window.list"),
            ("tabs", "tab.list"),
            ("panes", "pane.list"),
            ("sessions", "session.list"),
        ):
            try:
                data = await _thread(run_warpctrl, s, action, None, None)
                out[key] = data.get(key, data) if isinstance(data, dict) else data
            except WarpctrlError as e:
                out[key] = e.to_dict()
        return out

    @mcp.tool()
    @A()
    async def get_metadata(
        scope: Literal["instance", "window", "tab", "pane", "session"] = "tab",
        target: dict[str, Any] | None = None,
    ) -> dict:
        """`<scope>.inspect` for the active (or targeted) window/tab/pane/session/instance."""
        try:
            data = await _thread(run_warpctrl, s, f"{scope}.inspect", None, target)
        except WarpctrlError as e:
            return {"ok": False, **e.to_dict()}
        return {"ok": True, "data": data}

    return mcp


def _tname(t: Any) -> str:
    if isinstance(t, tuple):
        return "any"
    return getattr(t, "__name__", str(t))


def build_asgi_app(s: Services, token: str):
    mcp = create_server(s)
    inner = mcp.streamable_http_app()

    def on_reject(peer: str, reason: str) -> None:
        s.audit.write("auth_reject", peer=peer, reason=reason)

    return BearerAuthMiddleware(inner, token, on_reject=on_reject)


def make_platform(key_delivery: str = "pid") -> Platform | None:
    if sys.platform != "darwin":
        return None
    try:
        from .platform import MacPlatform

        return MacPlatform(key_delivery=key_delivery)
    except ImportError as e:
        log.error("pyobjc unavailable: %s", e)
        return None


__all__ = [
    "Services",
    "build_services",
    "create_server",
    "build_asgi_app",
    "make_platform",
    "run_warpctrl",
    "ValidationError",
]
