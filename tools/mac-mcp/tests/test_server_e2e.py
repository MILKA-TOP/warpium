"""Runs the real ASGI app (auth middleware + FastMCP streamable HTTP) under
uvicorn and talks to it with the official MCP client."""

import asyncio
import json
import socket
import threading
import time

import httpx
import pytest
import uvicorn
from mcp import ClientSession
from mcp.client.streamable_http import streamable_http_client

from warpium_mac_mcp.config import Config
from warpium_mac_mcp.server import build_asgi_app, build_services

TOKEN = "e2e-token-" + "x" * 40


def _free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


@pytest.fixture(scope="module")
def server(tmp_path_factory):
    # The container may route HTTP through a proxy; loopback must bypass it.
    mp = pytest.MonkeyPatch()
    mp.setenv("NO_PROXY", "127.0.0.1,localhost")
    mp.setenv("no_proxy", "127.0.0.1,localhost")
    tmp = tmp_path_factory.mktemp("e2e")
    port = _free_port()
    cfg = Config(port=port, workspace_dir=tmp / "ws", config_dir=tmp / "cfg", logs_dir=tmp / "logs")
    services = build_services(cfg, platform=None)
    app = build_asgi_app(services, TOKEN)
    srv = uvicorn.Server(
        uvicorn.Config(
            app, host="127.0.0.1", port=port, log_level="warning", lifespan="on", ws="none"
        )
    )
    t = threading.Thread(target=srv.run, daemon=True)
    t.start()
    for _ in range(100):
        if srv.started:
            break
        time.sleep(0.05)
    yield f"http://127.0.0.1:{port}/mcp", cfg
    srv.should_exit = True
    t.join(5)
    mp.undo()


def test_unauthenticated_rejected(server):
    url, _ = server
    r = httpx.post(url, json={})
    assert r.status_code == 401
    r = httpx.post(url, json={}, headers={"Authorization": "Bearer nope"})
    assert r.status_code == 401
    r = httpx.get(url.replace("/mcp", "/anything"))
    assert r.status_code == 401


def test_bad_host_header_rejected(server):
    url, _ = server
    r = httpx.post(
        url,
        json={},
        headers={
            "Authorization": f"Bearer {TOKEN}",
            "Host": "attacker.example:80",
            "Accept": "application/json, text/event-stream",
        },
    )
    assert r.status_code in (400, 421)


async def _session_calls(url):
    client = httpx.AsyncClient(headers={"Authorization": f"Bearer {TOKEN}"}, timeout=30)
    async with client, streamable_http_client(url, http_client=client) as (r, w, _):
        async with ClientSession(r, w) as session:
            await session.initialize()
            tools = {t.name for t in (await session.list_tools()).tools}
            bad = await session.call_tool("warpctrl", {"action": "shell.exec"})
            cat = await session.call_tool("warpctrl_catalog", {})
            sync = await session.call_tool("repo_sync", {"branch": "--upload-pack=x"})
            wins = await session.call_tool("list_windows", {})
            return tools, bad, cat, sync, wins


def test_file_open_cannot_reach_log(server):
    from warpium_mac_mcp.pathjail import PathEscapeError
    from warpium_mac_mcp.server import run_warpctrl

    _, cfg = server
    services = build_services(cfg, platform=None)
    log = cfg.log_files[0]
    log.parent.mkdir(parents=True, exist_ok=True)
    log.write_text("x")
    assert services.jail.contains(log)  # app_logs may read it (with ownership checks)
    with pytest.raises(PathEscapeError):
        run_warpctrl(services, "file.open", {"path": str(log)}, None)


def test_tools_over_mcp(server):
    url, cfg = server
    tools, bad, cat, sync, wins = asyncio.run(_session_calls(url))
    expected = {
        "doctor",
        "repo_sync",
        "build",
        "job_status",
        "job_logs",
        "job_cancel",
        "install_artifact",
        "run_integration_test",
        "app_launch",
        "app_quit",
        "app_status",
        "app_logs",
        "list_windows",
        "screenshot",
        "focus_app",
        "click",
        "scroll",
        "type_text",
        "press_keys",
        "warpctrl",
        "warpctrl_catalog",
        "list_targets",
        "get_metadata",
    }
    assert expected <= tools
    assert not any(n in tools for n in ("shell", "exec", "run_command", "bash"))
    assert bad.isError and "not in the local-control catalog" in bad.content[0].text
    assert "tab.create" in json.loads(cat.content[0].text)["actions"]
    assert sync.isError
    assert json.loads(wins.content[0].text) == {"windows": []}
    audit = cfg.audit_log_path.read_text()
    assert '"tool": "warpctrl"' in audit and '"tool": "repo_sync"' in audit
