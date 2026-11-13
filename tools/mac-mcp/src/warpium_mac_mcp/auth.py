"""Bearer-token ASGI middleware.

Wraps the whole Starlette/MCP app, so every HTTP request (including the MCP
session handshake and any non-MCP path) must carry the token. Non-HTTP scopes
(the ASGI lifespan) pass through untouched.
"""

from __future__ import annotations

import hmac
import json
import logging
from collections.abc import Awaitable, Callable, MutableMapping
from typing import Any

Scope = MutableMapping[str, Any]
Message = MutableMapping[str, Any]
Receive = Callable[[], Awaitable[Message]]
Send = Callable[[Message], Awaitable[None]]
ASGIApp = Callable[[Scope, Receive, Send], Awaitable[None]]

log = logging.getLogger(__name__)


def _header(scope: Scope, name: bytes) -> list[bytes]:
    return [v for k, v in scope.get("headers", []) if k.lower() == name]


def token_matches(presented: str | None, expected: str) -> bool:
    """Constant-time comparison; also false for missing/empty tokens."""
    if not presented or not expected:
        return False
    return hmac.compare_digest(presented.encode(), expected.encode())


def extract_bearer(values: list[bytes]) -> str | None:
    # Exactly one Authorization header, exactly "Bearer <token>".
    if len(values) != 1:
        return None
    try:
        raw = values[0].decode("latin-1")
    except UnicodeDecodeError:  # pragma: no cover - latin-1 decodes anything
        return None
    scheme, _, token = raw.partition(" ")
    if scheme.lower() != "bearer" or not token or " " in token.strip():
        return None
    return token.strip()


class BearerAuthMiddleware:
    def __init__(
        self, app: ASGIApp, token: str, on_reject: Callable[[str, str], None] | None = None
    ):
        if not token:
            raise ValueError("token must be non-empty")
        self._app = app
        self._token = token
        self._on_reject = on_reject

    async def __call__(self, scope: Scope, receive: Receive, send: Send) -> None:
        if scope["type"] not in ("http", "websocket"):
            await self._app(scope, receive, send)
            return

        reason = None
        # Browsers always send Origin on cross-site requests; CLI MCP clients
        # never do. Rejecting it closes the browser/DNS-rebinding avenue even
        # before the token check.
        if _header(scope, b"origin"):
            reason = "origin header not allowed"
        else:
            token = extract_bearer(_header(scope, b"authorization"))
            if token is None:
                reason = "missing or malformed bearer token"
            elif not token_matches(token, self._token):
                reason = "invalid bearer token"

        if reason is None:
            await self._app(scope, receive, send)
            return

        client = scope.get("client") or ("?", 0)
        peer = f"{client[0]}:{client[1]}"
        log.warning("rejected request from %s: %s", peer, reason)
        if self._on_reject:
            self._on_reject(peer, reason)
        if scope["type"] == "websocket":
            await send({"type": "websocket.close", "code": 1008})
            return
        body = json.dumps({"error": "unauthorized", "reason": reason}).encode()
        await send(
            {
                "type": "http.response.start",
                "status": 401,
                "headers": [
                    (b"content-type", b"application/json"),
                    (b"content-length", str(len(body)).encode()),
                    (b"www-authenticate", b'Bearer realm="warpium-mac-mcp"'),
                ],
            }
        )
        await send({"type": "http.response.body", "body": body})
