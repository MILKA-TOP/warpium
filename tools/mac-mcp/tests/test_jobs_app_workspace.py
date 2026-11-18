import json
import os
import shutil
import subprocess
import sys
import time
from pathlib import Path

import pytest

from warpium_mac_mcp.app import AppManager, filter_log_lines
from warpium_mac_mcp.audit import AuditLog, audited, redact_args
from warpium_mac_mcp.config import REPO_URL, Config
from warpium_mac_mcp.jobs import JobCancelled, JobError, JobManager, run_tracked, tail_file
from warpium_mac_mcp.pathjail import PathJail
from warpium_mac_mcp.validation import ValidationError
from warpium_mac_mcp.workspace import (
    CANONICAL_GIT_CONFIG,
    GIT_SAFE_OPTS,
    Workspace,
    WorkspaceError,
    check_run_info,
    normalize_remote,
    pick_artifact,
    remove_old_downloads,
    remove_self_update_backups,
    reset_git_config,
)


def wait(job, timeout=10):
    end = time.time() + timeout
    while job.state in ("queued", "running") and time.time() < end:
        time.sleep(0.05)
    return job


def test_job_success_and_logs(tmp_path):
    jm = JobManager(tmp_path)
    job = jm.start("t", "echo", [[sys.executable, "-c", "print('hello'); print('world')"]])
    wait(job)
    assert job.state == "succeeded" and job.exit_code == 0
    assert "hello" in jm.logs(job.id, 50)
    assert jm.logs(job.id, 50, grep="WORLD").splitlines()[-1] == "world"


def test_job_failure_stops_steps(tmp_path):
    jm = JobManager(tmp_path)
    job = jm.start(
        "t",
        "x",
        [[sys.executable, "-c", "raise SystemExit(3)"], [sys.executable, "-c", "print('never')"]],
    )
    wait(job)
    assert job.state == "failed" and job.exit_code == 3
    assert "never" not in jm.logs(job.id, 50)


def test_job_cancel_kills_group(tmp_path):
    jm = JobManager(tmp_path)
    job = jm.start("t", "sleep", [[sys.executable, "-c", "import time; time.sleep(60)"]])
    time.sleep(0.3)
    jm.cancel(job.id)
    wait(job)
    assert job.state == "cancelled"


def test_cancel_kills_python_step_subprocess(tmp_path):
    jm = JobManager(tmp_path)
    marker = tmp_path / "later-step-ran"

    def long_step(log):
        run_tracked(["sleep", "600"])
        marker.write_text("x")  # must not run after a cancel

    def later_step(log):
        marker.write_text("x")

    job = jm.start("t", "py", [long_step, later_step])
    end = time.time() + 5
    while job._proc is None and time.time() < end:
        time.sleep(0.05)
    proc = job._proc
    assert proc is not None, "run_tracked did not register its child with the job"
    t0 = time.monotonic()
    jm.cancel(job.id)
    assert job.summary()["cancel_requested"]
    wait(job, 5)
    assert job.state == "cancelled", job.summary()
    assert time.monotonic() - t0 < 3
    assert proc.poll() is not None
    assert not marker.exists()
    assert job.steps_done == 0


def test_run_tracked_outside_job_and_timeout(tmp_path):
    r = run_tracked(
        [sys.executable, "-c", "import sys; print('out'); print('err', file=sys.stderr)"]
    )
    assert (r.returncode, r.stdout.strip(), r.stderr.strip()) == (0, "out", "err")
    with pytest.raises(subprocess.TimeoutExpired):
        run_tracked(["sleep", "600"], timeout=0.3)


def test_run_tracked_not_cancellable_runs_after_cancel(tmp_path):
    jm = JobManager(tmp_path)
    seen = {}

    def step(log):
        jm.cancel(job_holder[0].id)
        try:
            run_tracked(["true"])
        except JobCancelled:
            seen["cancelled"] = True
        seen["cleanup_rc"] = run_tracked(["true"], cancellable=False).returncode

    job_holder = []
    job = jm.start("t", "py", [lambda log: time.sleep(0.2), step])
    job_holder.append(job)
    wait(job)
    assert job.state == "cancelled"
    assert seen == {"cancelled": True, "cleanup_rc": 0}


def test_exclusive_jobs(tmp_path):
    jm = JobManager(tmp_path)
    job = jm.start("t", "sleep", [[sys.executable, "-c", "import time; time.sleep(60)"]])
    with pytest.raises(JobError):
        jm.start("t2", "x", [[sys.executable, "-c", "pass"]])
    jm.cancel(job.id)
    wait(job)
    jm.start("t3", "x", [[sys.executable, "-c", "pass"]])


def test_python_step_and_unknown_job(tmp_path):
    jm = JobManager(tmp_path)

    def step(log):
        log("from python")

    job = wait(jm.start("t", "py", [step]))
    assert job.state == "succeeded"
    assert "from python" in jm.logs(job.id, 10)
    with pytest.raises(JobError):
        jm.get("nope")


def test_tail_file(tmp_path):
    p = tmp_path / "f"
    p.write_text("\n".join(f"line {i}" for i in range(100)) + "\n")
    assert tail_file(p, 3) == "line 97\nline 98\nline 99"
    assert tail_file(tmp_path / "missing", 3) == ""


def test_filter_log_lines_since(tmp_path):
    p = tmp_path / "warp-oss.log"
    p.write_text(
        "2026-01-01T00:00:00Z [INFO] old\n"
        "  continuation of old\n"
        "2026-01-01T01:00:00Z [ERROR] new failure\n"
        "  continuation of new\n"
        "2026-01-01T01:00:05Z [INFO] new ok\n"
    )
    from datetime import datetime

    since = datetime.fromisoformat("2026-01-01T00:30:00+00:00").timestamp()
    out = filter_log_lines(p, 100, None, since)
    assert "old" not in out and "continuation of new" in out
    assert filter_log_lines(p, 100, "error", None) == "2026-01-01T01:00:00Z [ERROR] new failure"


def test_audit_redaction(tmp_path):
    log = AuditLog(tmp_path / "audit.log")

    @audited(log, redact_when=("redact", ["text"]))
    def type_text(text, redact=False):
        return "ok"

    type_text("secret-pw", redact=True)
    type_text("visible")
    content = (tmp_path / "audit.log").read_text()
    assert "secret-pw" not in content and "<redacted>" in content and "visible" in content
    assert oct(os.stat(tmp_path / "audit.log").st_mode & 0o777) == "0o600"
    assert redact_args({"a": 1, "b": 2}, always=["b"]) == {"a": 1, "b": "<redacted>"}


def test_audit_records_errors(tmp_path):
    log = AuditLog(tmp_path / "audit.log")

    @audited(log)
    def boom(x):
        raise ValueError("bad")

    with pytest.raises(ValueError):
        boom(1)
    assert '"ok": false' in (tmp_path / "audit.log").read_text()


# --- workspace ----------------------------------------------------------------


def _git(*args, cwd):
    subprocess.run(
        ["git", *args],
        cwd=cwd,
        check=True,
        capture_output=True,
        env={
            **os.environ,
            "GIT_AUTHOR_NAME": "t",
            "GIT_AUTHOR_EMAIL": "t@t",
            "GIT_COMMITTER_NAME": "t",
            "GIT_COMMITTER_EMAIL": "t@t",
        },
    )


@pytest.fixture
def ws(tmp_path):
    cfg = Config(
        workspace_dir=tmp_path / "ws", config_dir=tmp_path / "cfg", logs_dir=tmp_path / "logs"
    )
    cfg.ensure_dirs()
    jail = PathJail([cfg.workspace_dir])
    return Workspace(cfg, JobManager(cfg.jobs_dir), jail), cfg


def test_repo_sync_refuses_dirty_clone(ws):
    w, cfg = ws
    clone = cfg.clone_dir
    clone.mkdir()
    _git("init", "-q", cwd=clone)
    _git("remote", "add", "origin", REPO_URL, cwd=clone)
    (clone / "f").write_text("x")
    job = wait(w.repo_sync("master"))
    assert job.state == "failed" and "local changes" in job.error
    assert (clone / "f").exists()  # nothing discarded


def test_repo_sync_refuses_foreign_origin(ws):
    w, cfg = ws
    clone = cfg.clone_dir
    clone.mkdir()
    _git("init", "-q", cwd=clone)
    _git("remote", "add", "origin", "https://github.com/someone/else.git", cwd=clone)
    with pytest.raises(WorkspaceError, match="refusing"):
        w.repo_sync("master")


def test_repo_sync_validates_branch(ws):
    w, _ = ws
    with pytest.raises(ValidationError):
        w.repo_sync("--upload-pack=touch /tmp/x")


def test_normalize_remote():
    assert normalize_remote("git@github.com:MILKA-TOP/warpium.git") == normalize_remote(REPO_URL)
    assert normalize_remote("https://github.com/milka-top/warpium/") == normalize_remote(REPO_URL)


def test_build_command_and_feature_check(ws):
    w, cfg = ws
    assert w.build_command("release", "warp-oss", ["gui"]) == [
        "cargo",
        "build",
        "-p",
        "warp",
        "--bin",
        "warp-oss",
        "--release",
        "--features",
        "gui",
    ]
    with pytest.raises(WorkspaceError):
        w.build()  # no clone yet
    clone = cfg.clone_dir
    (clone / "app").mkdir(parents=True)
    (clone / "Cargo.toml").write_text("[workspace]\n")
    (clone / "app" / "Cargo.toml").write_text("[features]\ngui = []\nwarp_control_cli = []\n")
    with pytest.raises(ValidationError):
        w.build(features=["not_a_feature"])


def test_build_disabled(tmp_path):
    cfg = Config(workspace_dir=tmp_path / "ws", config_dir=tmp_path / "c", allow_build=False)
    cfg.ensure_dirs()
    w = Workspace(cfg, JobManager(cfg.jobs_dir), PathJail([cfg.workspace_dir]))
    with pytest.raises(WorkspaceError, match="disabled"):
        w.build()
    with pytest.raises(WorkspaceError, match="disabled"):
        w.run_integration_test("test_x")


def test_integration_test_must_be_registered(ws):
    w, cfg = ws
    clone = cfg.clone_dir
    (clone / "crates" / "integration" / "src" / "bin").mkdir(parents=True)
    (clone / "Cargo.toml").write_text("")
    (clone / "crates" / "integration" / "src" / "bin" / "integration.rs").write_text(
        "register_test!(test_open_tab);\nregister_test!( test_other );\n"
    )
    with pytest.raises(ValidationError):
        w.run_integration_test("test_missing")


def test_install_artifact_validates(ws):
    w, _ = ws
    with pytest.raises(ValidationError):
        w.install_artifact("12; rm -rf /")
    with pytest.raises(ValidationError):
        w.install_artifact("12", artifact_name="../x")


def test_target_pids_only_workspace_binaries(ws, tmp_path):
    w, cfg = ws
    built = cfg.clone_dir / "target" / "debug" / "warp-oss"
    app_exe = cfg.apps_dir / "Warpium.app" / "Contents" / "MacOS" / "warp-oss"
    for p in (built, app_exe):
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text("")
    procs = [
        (10, str(built)),
        (11, str(app_exe)),
        (12, "/Applications/Warpium.app/Contents/MacOS/warp-oss"),  # owner's daily app
        (13, str(cfg.workspace_dir / "evil")),  # something else in the workspace
        (14, "warp-oss"),
    ]
    am = AppManager(cfg, w, PathJail([cfg.workspace_dir]), processes=lambda: procs)
    assert am.target_pids() == {10, 11}


def test_launch_validates_before_spawning(ws):
    w, cfg = ws
    am = AppManager(cfg, w, PathJail([cfg.workspace_dir]), processes=lambda: [])
    with pytest.raises(ValidationError):
        am.launch(env={"DYLD_INSERT_LIBRARIES": "/x.dylib"})
    with pytest.raises(ValidationError):
        am.launch(args=["agent", "run"])
    with pytest.raises(WorkspaceError):
        am.launch()  # binary not built
    with pytest.raises(ValidationError):
        am.launch(source="anything")


def test_launch_and_quit_fake_binary(ws):
    w, cfg = ws
    built = cfg.clone_dir / "target" / "debug" / "warp-oss"
    built.parent.mkdir(parents=True)
    built.write_text(
        "#!/bin/sh\necho started WARP_DATA_PROFILE=$WARP_DATA_PROFILE\nexec sleep 60\n"
    )
    built.chmod(0o755)
    am = AppManager(cfg, w, PathJail([cfg.workspace_dir]), processes=lambda: [])
    info = am.launch(env={"RUST_LOG": "debug"})
    assert info["data_profile"] == "mcp-test"
    assert am.target_pids() == {info["pid"]}
    with pytest.raises(WorkspaceError):
        am.launch()  # already running
    out = am.quit()
    assert info["pid"] in out["quit"]
    assert am.target_pids() == set()
    assert "WARP_DATA_PROFILE=mcp-test" in Path(info["stdout_log"]).read_text()


def test_reset_git_config_drops_planted_settings(ws):
    w, cfg = ws
    clone = cfg.clone_dir
    clone.mkdir()
    _git("init", "-q", cwd=clone)
    _git("remote", "add", "origin", REPO_URL, cwd=clone)
    _git("config", "core.fsmonitor", "/tmp/payload", cwd=clone)
    _git("config", "filter.x.smudge", "/tmp/payload", cwd=clone)
    reset_git_config(clone)
    text = (clone / ".git" / "config").read_text()
    assert text == CANONICAL_GIT_CONFIG
    assert "payload" not in text


def test_repo_sync_steps_use_safe_git_options(ws):
    w, cfg = ws
    clone = cfg.clone_dir
    clone.mkdir()
    _git("init", "-q", cwd=clone)
    _git("remote", "add", "origin", REPO_URL, cwd=clone)
    captured = {}

    def fake_start(kind, desc, steps, **kw):
        captured["steps"] = steps
        return None

    w.jobs.start = fake_start
    w.repo_sync("master")
    argv_steps = [st for st in captured["steps"] if not callable(st)]
    assert argv_steps
    for step in argv_steps:
        assert list(step[1:5]) == list(GIT_SAFE_OPTS)


def test_git_env_ignores_global_config():
    from warpium_mac_mcp.workspace import git_env

    env = git_env({"PATH": "/bin"})
    assert env["GIT_CONFIG_GLOBAL"] == "/dev/null"
    assert env["GIT_CONFIG_NOSYSTEM"] == "1"


def test_reset_git_config_refuses_commondir_and_tmp_symlink(ws, tmp_path):
    w, cfg = ws
    clone = cfg.clone_dir
    clone.mkdir()
    _git("init", "-q", cwd=clone)
    victim = tmp_path / "victim"
    victim.write_text("owner data")
    os.symlink(victim, clone / ".git" / "config.mcp-tmp")
    reset_git_config(clone)
    assert victim.read_text() == "owner data"  # symlink not followed
    assert not (clone / ".git" / "config").is_symlink()
    (clone / ".git" / "commondir").write_text("/tmp/evil")
    with pytest.raises(WorkspaceError, match="commondir"):
        reset_git_config(clone)


def _run(**over):
    run = {
        "repository": {"full_name": "MILKA-TOP/warpium"},
        "head_repository": {"full_name": "MILKA-TOP/warpium"},
        "path": ".github/workflows/warpium-build.yml",
        "conclusion": "success",
        "head_branch": "master",
        "head_sha": "abc",
        "event": "push",
    }
    run.update(over)
    return run


def test_check_run_info():
    assert check_run_info(_run())["head_branch"] == "master"
    for bad in [
        _run(head_repository={"full_name": "attacker/warpium"}),
        _run(path=".github/workflows/ci.yml"),
        _run(conclusion="failure"),
        _run(repository={"full_name": "warpdotdev/warp"}),
    ]:
        with pytest.raises(WorkspaceError):
            check_run_info(bad)


def test_install_artifact_disabled_with_no_build(tmp_path):
    cfg = Config(workspace_dir=tmp_path / "ws", config_dir=tmp_path / "c", allow_build=False)
    cfg.ensure_dirs()
    w = Workspace(cfg, JobManager(cfg.jobs_dir), PathJail([cfg.workspace_dir]))
    with pytest.raises(WorkspaceError, match="disabled"):
        w.install_artifact("123")


def test_app_logs_ownership(ws):
    w, cfg = ws
    cfg.logs_dir.mkdir(parents=True, exist_ok=True)
    log = cfg.logs_dir / "warp-oss.log"
    built = cfg.clone_dir / "target" / "debug" / "warp-oss"
    built.parent.mkdir(parents=True)
    built.write_text("#!/bin/sh\nexec sleep 60\n")
    built.chmod(0o755)
    procs = [(12, "/Applications/Warpium.app/Contents/MacOS/warp-oss")]
    jail = PathJail([cfg.workspace_dir], files=cfg.log_files)
    am = AppManager(cfg, w, jail, processes=lambda: procs)
    log.write_text("2026-01-01T00:00:00Z [INFO] some line\n")
    with pytest.raises(PermissionError):  # nothing launched by us
        am.logs(source="app")
    am.launch()
    try:
        with pytest.raises(PermissionError, match="another Warpium"):
            am.logs(source="app")
        procs.clear()
        log.unlink()
        log.write_text("2026-01-01T00:00:00Z [INFO] test app line\n")  # created after launch
        assert "test app line" in am.logs(source="app")["lines"]
        # Log (re)created long after our launch: another app started and quit since.
        am._launched.started_at -= 1000
        with pytest.raises(PermissionError, match="not created by"):
            am.logs(source="app")
    finally:
        am.quit()
    with pytest.raises(PermissionError):  # our app is gone
        am.logs(source="app")


def test_real_executable_path_not_argv0():
    from warpium_mac_mcp.app import executable_path, list_processes

    me = os.getpid()
    assert executable_path(me) == os.path.realpath(sys.executable)
    assert (me, os.path.realpath(sys.executable)) in list_processes()


def test_pick_artifact():
    arts = [
        {"name": "Warpium-a70e8571-macos-arm64", "expired": False},
        {"name": "Warpium-old-macos-arm64", "expired": True},
        {"name": "logs", "expired": False},
    ]
    assert pick_artifact(arts, None) == "Warpium-a70e8571-macos-arm64"
    assert pick_artifact(arts, "logs") == "logs"
    with pytest.raises(WorkspaceError):
        pick_artifact(arts, "Warpium-old-macos-arm64")  # expired
    with pytest.raises(WorkspaceError):
        pick_artifact(arts + [{"name": "Warpium-x", "expired": False}], None)
    with pytest.raises(WorkspaceError):
        pick_artifact([], None)


def _fake_app(apps_dir: Path, name: str, exe: str = "warp-oss") -> Path:
    import plistlib

    app = apps_dir / name
    (app / "Contents" / "MacOS").mkdir(parents=True)
    with open(app / "Contents" / "Info.plist", "wb") as f:
        plistlib.dump({"CFBundleExecutable": exe}, f)
    binary = app / "Contents" / "MacOS" / exe
    binary.write_text("#!/bin/sh\n")
    binary.chmod(0o755)
    return app


def test_installed_app_ignores_self_update_backup(ws):
    # The self-updater leaves `.Warpium-previous.app` next to Warpium.app; "."
    # sorts first, so app_launch(source="artifact") used to run the stale backup.
    w, cfg = ws
    assert w.installed_app() is None
    _fake_app(cfg.apps_dir, ".Warpium-previous.app")
    _fake_app(cfg.apps_dir, ".Warpium-update-123.app")
    assert w.installed_app() is None
    app = _fake_app(cfg.apps_dir, "Warpium.app")
    assert w.installed_app() == app
    am = AppManager(cfg, w, PathJail([cfg.workspace_dir]), processes=lambda: [])
    exe = am.resolve_binary("artifact")
    assert exe == Path(os.path.realpath(app / "Contents" / "MacOS" / "warp-oss"))


def test_installed_app_prefers_warpium_app(ws):
    w, cfg = ws
    other = _fake_app(cfg.apps_dir, "Another.app")
    assert w.installed_app() == other
    app = _fake_app(cfg.apps_dir, "Warpium.app")
    assert w.installed_app() == app


def test_installed_app_skips_broken_bundles(ws):
    w, cfg = ws
    (cfg.apps_dir / "Warpium.app").mkdir()  # partial copy, no Info.plist
    (cfg.apps_dir / "Stray.app").write_text("")  # a file, not a bundle
    assert w.installed_app() is None
    other = _fake_app(cfg.apps_dir, "Other.app")
    assert w.installed_app() == other


def test_remove_self_update_backups(ws, tmp_path):
    _, cfg = ws
    _fake_app(cfg.apps_dir, "Warpium.app")
    _fake_app(cfg.apps_dir, ".Warpium-previous.app")
    _fake_app(cfg.apps_dir, ".Warpium-update-123.app")  # maybe a live self-update
    _fake_app(cfg.apps_dir, ".Warpium-known-good.app")  # not the updater's
    outside = tmp_path / "outside.app"
    outside.mkdir()
    (outside / "x").write_text("keep")
    (cfg.apps_dir / ".Other-previous.app").symlink_to(outside)
    lines = []
    remove_self_update_backups(cfg.apps_dir, lines.append)
    assert sorted(p.name for p in cfg.apps_dir.iterdir()) == [
        ".Warpium-known-good.app",
        ".Warpium-update-123.app",
        "Warpium.app",
    ]
    assert (outside / "x").read_text() == "keep"  # symlink target not followed
    assert len(lines) == 2


def test_remove_old_downloads_keeps_only_current_run(ws, tmp_path):
    _, cfg = ws
    d = cfg.downloads_dir
    for name in ("111", "111-tmp", "222", "222-tmp", "111-mnt", "notes"):
        (d / name).mkdir(parents=True)
        (d / name / "f").write_text("x")
    (d / "333").write_text("stray file")
    outside = tmp_path / "outside"
    outside.mkdir()
    (outside / "x").write_text("keep")
    (d / "444").symlink_to(outside)
    lines = []
    remove_old_downloads(d, "222", lines.append)
    assert sorted(p.name for p in d.iterdir()) == ["111-mnt", "222", "222-tmp", "notes"]
    assert (outside / "x").read_text() == "keep"  # symlink target not followed
    assert len(lines) == 4


def test_install_artifact_removes_old_downloads(ws, tmp_path, monkeypatch):
    w, cfg = ws
    (cfg.downloads_dir / "99").mkdir(parents=True)
    (cfg.downloads_dir / "99" / "Old.dmg").write_text("old")
    _fake_tools(tmp_path, monkeypatch, 'echo dmg > "$DIR/Warpium.dmg"')
    job = w.install_artifact("123")
    wait(job, 10)
    assert job.state == "succeeded", job.summary()
    assert not (cfg.downloads_dir / "99").exists()
    assert (cfg.downloads_dir / "123" / "Warpium.dmg").exists()


def test_remove_self_update_backups_is_best_effort(ws, monkeypatch):
    _, cfg = ws
    _fake_app(cfg.apps_dir, ".Warpium-previous.app")

    def boom(p):
        raise PermissionError("nope")

    monkeypatch.setattr("warpium_mac_mcp.workspace.shutil.rmtree", boom)
    lines = []
    remove_self_update_backups(cfg.apps_dir, lines.append)
    assert "could not remove" in lines[0]


def test_install_artifact_removes_self_update_backup(ws, monkeypatch):
    w, cfg = ws
    _fake_app(cfg.apps_dir, "Warpium.app", exe="old")
    _fake_app(cfg.apps_dir, ".Warpium-previous.app")
    calls = []

    def fake_run(argv, **kw):
        calls.append(argv[0:2])
        out = ""
        if argv[:2] == ["gh", "api"]:
            if argv[2].endswith("/artifacts?per_page=100"):
                out = json.dumps({"artifacts": [{"name": "Warpium-abc", "expired": False}]})
            else:
                out = json.dumps(_run())
        elif argv[:3] == ["gh", "run", "download"]:
            d = Path(argv[argv.index("--dir") + 1])
            (d / "Warpium.dmg").write_text("")
        elif argv[:2] == ["hdiutil", "attach"]:
            mnt = Path(argv[argv.index("-mountpoint") + 1])
            _fake_app(mnt, "Warpium.app", exe="new")
        elif argv[0] == "ditto":
            shutil.copytree(argv[1], argv[2], symlinks=True)
        return subprocess.CompletedProcess(argv, 0, out, "")

    monkeypatch.setattr("warpium_mac_mcp.workspace.run_tracked", fake_run)
    job = wait(w.install_artifact("123"))
    assert job.state == "succeeded", job.error
    assert sorted(p.name for p in cfg.apps_dir.iterdir()) == ["Warpium.app"]
    assert w.app_executable(w.installed_app()).name == "new"
    assert ["hdiutil", "detach"] in calls


# --- install_artifact with fake gh/hdiutil/ditto on PATH ------------------------


def _fake_tools(tmp_path: Path, monkeypatch, download_body: str) -> Path:
    """Puts fake gh, hdiutil, ditto and xattr on PATH. Each tool appends its
    name to ``calls.log``; ``download_body`` is the shell code for
    ``gh run download`` ($DIR is the --dir argument)."""
    bindir = tmp_path / "fakebin"
    bindir.mkdir()
    (bindir / "run.json").write_text(json.dumps(_run()))
    (bindir / "artifacts.json").write_text(
        json.dumps({"artifacts": [{"name": "Warpium-abc", "expired": False}]})
    )
    calls = tmp_path / "calls.log"
    scripts = {
        "gh": f"""
echo "gh $1 $2" >> {calls}
if [ "$1" = api ]; then
  case "$2" in *artifacts*) cat {bindir}/artifacts.json ;; *) cat {bindir}/run.json ;; esac
  exit 0
fi
DIR=""
while [ $# -gt 0 ]; do [ "$1" = --dir ] && DIR="$2"; shift; done
echo $$ > {tmp_path}/gh.pid
{download_body}
""",
        "hdiutil": f"""
echo "hdiutil $1" >> {calls}
if [ "$1" = attach ]; then
  while [ $# -gt 0 ]; do [ "$1" = -mountpoint ] && MNT="$2"; shift; done
  mkdir -p "$MNT/Warpium.app/Contents"
fi
""",
        "ditto": f'echo ditto >> {calls}\ncp -R "$1" "$2"\n',
        "xattr": f"echo xattr >> {calls}\n",
    }
    for name, body in scripts.items():
        f = bindir / name
        f.write_text("#!/bin/sh\n" + body)
        f.chmod(0o755)
    monkeypatch.setenv("PATH", f"{bindir}:{os.environ['PATH']}")
    return calls


def test_install_artifact_cancel_kills_download(ws, tmp_path, monkeypatch):
    w, cfg = ws
    calls = _fake_tools(tmp_path, monkeypatch, "exec sleep 600")
    job = w.install_artifact("123")
    pidfile = tmp_path / "gh.pid"
    end = time.time() + 10
    while not (pidfile.exists() and pidfile.read_text().strip()) and time.time() < end:
        time.sleep(0.05)
    assert job.current_step == "download", job.summary()
    pid = int(pidfile.read_text())
    t0 = time.monotonic()
    w.jobs.cancel(job.id)
    wait(job, 5)
    assert job.state == "cancelled", job.summary()
    assert time.monotonic() - t0 < 4
    with pytest.raises(ProcessLookupError):
        os.kill(pid, 0)
    log = calls.read_text()
    assert "hdiutil" not in log and "ditto" not in log, log
    assert not (cfg.downloads_dir / "123-tmp").exists()


def test_install_artifact_logs_download_progress(ws, tmp_path, monkeypatch):
    w, cfg = ws
    w.download_tick_s = 0.2
    body = """
i=0
while [ $i -lt 8 ]; do
  head -c 40000 /dev/zero >> "$TMPDIR/gh-artifact.1.zip"
  sleep 0.15
  i=$((i+1))
done
echo dmg > "$DIR/Warpium.dmg"
"""
    calls = _fake_tools(tmp_path, monkeypatch, body)
    job = wait(w.install_artifact("123"), 20)
    assert job.state == "succeeded", (job.summary(), w.jobs.logs(job.id, 100))
    progress = w.jobs.logs(job.id, 200, grep="download: zip")
    assert progress, w.jobs.logs(job.id, 100)
    assert "KB/s" in progress
    sizes = [float(line.split("zip ")[1].split(" MB")[0]) for line in progress.splitlines()]
    assert max(sizes) > 0
    assert "download: done" in w.jobs.logs(job.id, 200)
    assert "hdiutil detach" in calls.read_text()


def test_install_artifact_download_stall_fails(ws, tmp_path, monkeypatch):
    w, cfg = ws
    w.download_tick_s, w.download_stall_s = 0.2, 0.7
    calls = _fake_tools(tmp_path, monkeypatch, "exec sleep 600")
    job = wait(w.install_artifact("123"), 10)
    assert job.state == "failed", job.summary()
    assert "stalled" in job.error
    with pytest.raises(ProcessLookupError):
        os.kill(int((tmp_path / "gh.pid").read_text()), 0)
    assert "hdiutil" not in calls.read_text()
