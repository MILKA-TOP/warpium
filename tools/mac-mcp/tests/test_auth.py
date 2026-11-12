import asyncio
import os
import stat

import pytest

from warpium_mac_mcp.auth import BearerAuthMiddleware, extract_bearer, token_matches
from warpium_mac_mcp.config import Config, load_or_create_token

TOKEN = "t" * 43


async def _ok_app(scope, receive, send):
    await send({"type": "http.response.start", "status": 200, "headers": []})
    await send({"type": "http.response.body", "body": b"ok"})


def _call(app, headers, scope_type="http"):
    sent = []

    async def receive():
        return {"type": "http.request", "body": b"", "more_body": False}

    async def send(msg):
        sent.append(msg)

    scope = {"type": scope_type, "headers": headers, "client": ("10.0.0.1", 1234)}
    asyncio.run(app(scope, receive, send))
    return sent


def status_of(sent):
    return sent[0]["status"]


def test_missing_token_rejected():
    rejects = []
    app = BearerAuthMiddleware(_ok_app, TOKEN, on_reject=lambda p, r: rejects.append(r))
    assert status_of(_call(app, [])) == 401
    assert rejects == ["missing or malformed bearer token"]


def test_wrong_token_rejected():
    app = BearerAuthMiddleware(_ok_app, TOKEN)
    assert status_of(_call(app, [(b"authorization", b"Bearer " + b"x" * 43)])) == 401
    assert status_of(_call(app, [(b"authorization", b"Bearer " + TOKEN[:-1].encode())])) == 401
    assert status_of(_call(app, [(b"authorization", b"Bearer " + TOKEN.encode() + b"x")])) == 401


def test_wrong_scheme_rejected():
    app = BearerAuthMiddleware(_ok_app, TOKEN)
    assert status_of(_call(app, [(b"authorization", b"Basic " + TOKEN.encode())])) == 401
    assert status_of(_call(app, [(b"authorization", TOKEN.encode())])) == 401
    assert status_of(_call(app, [(b"authorization", b"Bearer ")])) == 401


def test_duplicate_authorization_rejected():
    app = BearerAuthMiddleware(_ok_app, TOKEN)
    h = (b"authorization", b"Bearer " + TOKEN.encode())
    assert status_of(_call(app, [h, h])) == 401


def test_correct_token_passes():
    app = BearerAuthMiddleware(_ok_app, TOKEN)
    sent = _call(app, [(b"authorization", b"Bearer " + TOKEN.encode())])
    assert status_of(sent) == 200
    sent = _call(app, [(b"Authorization", b"bearer " + TOKEN.encode())])
    assert status_of(sent) == 200


def test_origin_header_rejected_even_with_token():
    app = BearerAuthMiddleware(_ok_app, TOKEN)
    sent = _call(
        app,
        [(b"authorization", b"Bearer " + TOKEN.encode()), (b"origin", b"http://localhost:3000")],
    )
    assert status_of(sent) == 401


def test_lifespan_passes_through():
    seen = []

    async def app(scope, receive, send):
        seen.append(scope["type"])

    mw = BearerAuthMiddleware(app, TOKEN)
    _call(mw, [], scope_type="lifespan")
    assert seen == ["lifespan"]


def test_websocket_rejected():
    app = BearerAuthMiddleware(_ok_app, TOKEN)
    sent = _call(app, [], scope_type="websocket")
    assert sent == [{"type": "websocket.close", "code": 1008}]


def test_empty_token_refused():
    with pytest.raises(ValueError):
        BearerAuthMiddleware(_ok_app, "")


def test_token_matches_helpers():
    assert token_matches("abc", "abc")
    assert not token_matches("abd", "abc")
    assert not token_matches(None, "abc")
    assert not token_matches("", "")
    assert extract_bearer([b"Bearer a b"]) is None
    assert extract_bearer([b"Bearer abc"]) == "abc"


def test_token_created_private(tmp_path):
    p = tmp_path / "cfg" / "token"
    t = load_or_create_token(p)
    assert len(t) >= 40
    assert stat.S_IMODE(os.stat(p).st_mode) == 0o600
    assert stat.S_IMODE(os.stat(p.parent).st_mode) == 0o700
    assert load_or_create_token(p) == t


def test_token_world_readable_refused(tmp_path):
    p = tmp_path / "token"
    load_or_create_token(p)
    os.chmod(p, 0o644)
    with pytest.raises(PermissionError):
        load_or_create_token(p)


def test_token_symlink_refused(tmp_path):
    real = tmp_path / "real"
    real.write_text("x" * 50)
    os.chmod(real, 0o600)
    (tmp_path / "d").mkdir()
    os.symlink(real, tmp_path / "d" / "token")
    with pytest.raises(PermissionError):
        load_or_create_token(tmp_path / "d" / "token")


def test_allowed_hosts_include_docker_host():
    cfg = Config(port=9999)
    hosts = cfg.allowed_hosts()
    assert "host.docker.internal:9999" in hosts
    assert "127.0.0.1:9999" in hosts
    assert all(h.endswith(":9999") for h in hosts)
