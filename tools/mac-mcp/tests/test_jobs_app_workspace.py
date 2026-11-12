import os
import subprocess
import sys
import time
from pathlib import Path

import pytest

from warpium_mac_mcp.app import AppManager, filter_log_lines
from warpium_mac_mcp.audit import AuditLog, audited, redact_args
from warpium_mac_mcp.config import REPO_URL, Config
from warpium_mac_mcp.jobs import JobError, JobManager, tail_file
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
    with pytest.raises(WorkspaceError, match="local changes"):
        w.repo_sync("master")


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
    for step in captured["steps"]:
        assert list(step[1:5]) == list(GIT_SAFE_OPTS)


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


def test_app_logs_refused_while_other_warpium_runs(ws):
    w, cfg = ws
    cfg.logs_dir.mkdir(parents=True, exist_ok=True)
    (cfg.logs_dir / "warp-oss.log").write_text("2026-01-01T00:00:00Z [INFO] daily app line\n")
    built = cfg.clone_dir / "target" / "debug" / "warp-oss"
    built.parent.mkdir(parents=True)
    built.write_text("")
    procs = [(10, str(built)), (12, "/Applications/Warpium.app/Contents/MacOS/warp-oss")]
    jail = PathJail([cfg.workspace_dir], files=cfg.log_files)
    am = AppManager(cfg, w, jail, processes=lambda: procs)
    with pytest.raises(PermissionError, match="another Warpium"):
        am.logs(source="app")
    procs.pop()
    assert "daily app line" in am.logs(source="app")["lines"]
    procs.clear()  # nothing under test running: the log may be someone else's
    with pytest.raises(PermissionError):
        am.logs(source="app")


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
