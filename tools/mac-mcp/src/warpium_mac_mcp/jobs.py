"""Background jobs (git sync, cargo build, artifact install, integration tests).

A job is a sequence of steps. A step is either an argv list (run as a
subprocess, never through a shell) or a Python callable that receives a
``log(str)`` function. Output goes to ``<jobs_dir>/<job_id>.log``.
"""

from __future__ import annotations

import os
import signal
import subprocess
import threading
import time
import uuid
from collections import deque
from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass, field
from pathlib import Path

# A Python step may return a dict, which is merged into the job's ``result``.
Step = Sequence[str] | Callable[[Callable[[str], None]], dict | None]


class JobError(RuntimeError):
    pass


@dataclass
class Job:
    id: str
    kind: str
    description: str
    log_path: Path
    exclusive: bool
    state: str = "queued"  # queued|running|succeeded|failed|cancelled
    exit_code: int | None = None
    error: str | None = None
    started_at: float = field(default_factory=time.time)
    finished_at: float | None = None
    current_step: str | None = None
    steps_total: int = 0
    steps_done: int = 0
    result: dict = field(default_factory=dict)
    _proc: subprocess.Popen | None = None
    _cancel: threading.Event = field(default_factory=threading.Event)

    def summary(self) -> dict:
        now = self.finished_at or time.time()
        return {
            "job_id": self.id,
            "kind": self.kind,
            "description": self.description,
            "state": self.state,
            "exit_code": self.exit_code,
            "error": self.error,
            "current_step": self.current_step,
            "steps_done": self.steps_done,
            "steps_total": self.steps_total,
            "elapsed_s": round(now - self.started_at, 1),
            "log_path": str(self.log_path),
            "result": self.result,
        }


class JobManager:
    def __init__(self, jobs_dir: Path, max_jobs: int = 200):
        self._dir = jobs_dir
        self._dir.mkdir(parents=True, exist_ok=True)
        self._jobs: dict[str, Job] = {}
        self._lock = threading.Lock()
        self._max_jobs = max_jobs

    def start(
        self,
        kind: str,
        description: str,
        steps: Sequence[Step],
        *,
        cwd: Path | None = None,
        env: Mapping[str, str] | None = None,
        exclusive: bool = True,
        timeout_s: float | None = None,
    ) -> Job:
        """``exclusive`` jobs mutate the workspace clone; only one may run at a time."""
        with self._lock:
            if exclusive:
                for j in self._jobs.values():
                    if j.exclusive and j.state in ("queued", "running"):
                        raise JobError(
                            f"job {j.id} ({j.kind}) is still running; wait or job_cancel it first"
                        )
            if len(self._jobs) >= self._max_jobs:
                self._evict_finished()
            job_id = f"{kind}-{time.strftime('%Y%m%d-%H%M%S')}-{uuid.uuid4().hex[:6]}"
            job = Job(
                id=job_id,
                kind=kind,
                description=description,
                log_path=self._dir / f"{job_id}.log",
                exclusive=exclusive,
                steps_total=len(steps),
            )
            self._jobs[job_id] = job
        t = threading.Thread(
            target=self._run,
            args=(job, list(steps), cwd, dict(env) if env else None, timeout_s),
            name=f"job-{job_id}",
            daemon=True,
        )
        t.start()
        return job

    def _evict_finished(self) -> None:
        finished = sorted(
            (j for j in self._jobs.values() if j.state not in ("queued", "running")),
            key=lambda j: j.started_at,
        )
        for j in finished[: max(1, len(finished) // 2)]:
            self._jobs.pop(j.id, None)

    def _run(
        self,
        job: Job,
        steps: list[Step],
        cwd: Path | None,
        env: dict | None,
        timeout_s: float | None,
    ) -> None:
        deadline = time.monotonic() + timeout_s if timeout_s else None
        job.state = "running"
        with open(job.log_path, "a", encoding="utf-8", errors="replace") as logf:

            def log(msg: str) -> None:
                logf.write(msg if msg.endswith("\n") else msg + "\n")
                logf.flush()

            try:
                for step in steps:
                    if job._cancel.is_set():
                        break
                    if callable(step):
                        job.current_step = getattr(step, "__name__", "python-step")
                        log(f"$ [{job.current_step}]")
                        out = step(log)
                        if isinstance(out, dict):
                            job.result.update(out)
                    else:
                        argv = list(step)
                        job.current_step = " ".join(argv)
                        log(f"$ {job.current_step}")
                        rc = self._run_proc(job, argv, cwd, env, logf, deadline)
                        if rc != 0:
                            job.exit_code = rc
                            if not job._cancel.is_set():
                                job.state = "failed"
                                job.error = f"step exited with {rc}: {job.current_step}"
                            break
                    job.steps_done += 1
                else:
                    job.exit_code = 0
                    job.state = "succeeded"
            except Exception as e:  # noqa: BLE001 - recorded on the job
                job.state = "failed"
                job.error = f"{type(e).__name__}: {e}"
                log(f"error: {job.error}")
            finally:
                if job._cancel.is_set():
                    job.state = "cancelled"
                job.finished_at = time.time()
                job._proc = None
                log(f"== job {job.state} (exit={job.exit_code}) ==")

    def _run_proc(
        self,
        job: Job,
        argv: list[str],
        cwd: Path | None,
        env: dict | None,
        logf,
        deadline: float | None,
    ) -> int:
        proc = subprocess.Popen(
            argv,
            cwd=cwd,
            env=env,
            stdin=subprocess.DEVNULL,
            stdout=logf,
            stderr=subprocess.STDOUT,
            start_new_session=True,  # own process group so cancel kills children too
        )
        job._proc = proc
        while True:
            try:
                return proc.wait(timeout=0.5)
            except subprocess.TimeoutExpired:
                pass
            if deadline is not None and time.monotonic() > deadline:
                logf.write("timeout: killing job\n")
                job.error = "timed out"
                _kill_group(proc)
                job._cancel.set()
            if job._cancel.is_set():
                _kill_group(proc)
                return proc.wait()

    def get(self, job_id: str) -> Job:
        job = self._jobs.get(job_id)
        if job is None:
            raise JobError(f"unknown job id: {job_id!r}")
        return job

    def list(self) -> list[dict]:
        return [j.summary() for j in sorted(self._jobs.values(), key=lambda j: j.started_at)]

    def cancel(self, job_id: str) -> Job:
        job = self.get(job_id)
        if job.state in ("queued", "running"):
            job._cancel.set()
            if job._proc is not None:
                _kill_group(job._proc)
        return job

    def logs(self, job_id: str, tail: int = 200, grep: str | None = None) -> str:
        job = self.get(job_id)
        return tail_file(job.log_path, tail, grep)

    def cancel_all(self) -> None:
        for job in list(self._jobs.values()):
            if job.state in ("queued", "running"):
                self.cancel(job.id)


def _kill_group(proc: subprocess.Popen) -> None:
    if proc.poll() is not None:
        return
    try:
        pgid = os.getpgid(proc.pid)
        os.killpg(pgid, signal.SIGTERM)
    except (ProcessLookupError, PermissionError):
        return
    try:
        proc.wait(timeout=5)
    except subprocess.TimeoutExpired:
        try:
            os.killpg(pgid, signal.SIGKILL)
        except ProcessLookupError:
            pass


def tail_file(
    path: Path, tail: int, grep: str | None = None, max_bytes: int = 32 * 1024 * 1024
) -> str:
    """Last ``tail`` lines of ``path`` (optionally only lines containing ``grep``,
    case-insensitive). Reads at most ``max_bytes`` from the end of the file."""
    if not path.exists():
        return ""
    size = path.stat().st_size
    with open(path, "rb") as f:
        if size > max_bytes:
            f.seek(size - max_bytes)
            f.readline()  # drop partial line
        lines: deque[str] = deque(maxlen=tail)
        needle = grep.lower() if grep else None
        for raw in f:
            line = raw.decode("utf-8", errors="replace").rstrip("\n")
            if needle is None or needle in line.lower():
                lines.append(line)
    return "\n".join(lines)
