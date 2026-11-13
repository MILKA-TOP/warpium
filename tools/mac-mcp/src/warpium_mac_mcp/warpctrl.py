"""Native Python client for Warp's local-control protocol (the protocol behind
``warpctrl``), restricted to the action catalog.

Protocol (crates/local_control/src/{discovery,client,auth,protocol}.rs):

1. Discovery: each running app with Settings > Scripting enabled writes an
   owner-only JSON record ``<instance_id>.json`` to the discovery dir
   (``$WARP_LOCAL_CONTROL_DISCOVERY_DIR`` or ``$XDG_RUNTIME_DIR/warp/local-control``
   or ``~/.warp/local-control``). It holds pid, channel, a ``127.0.0.1``
   endpoint and the filename of a Unix credential-broker socket.
2. Credential: connect to ``<instance_id>.broker.sock`` (the app checks the
   peer UID), send ``{"protocol_version":1,"request_id":<uuid>,"action":<name>}``,
   half-close, read a ``ScopedCredential`` (bearer token valid 5 min for that
   one action).
3. Request: POST ``http://127.0.0.1:<port>/v1/control`` with that bearer and
   ``{"protocol_version":1,"request_id":..,"target":{..},"action":{"kind":..,"params":{..}}}``.
   The app rejects requests with an ``Origin`` header or a Host other than
   ``127.0.0.1:<port>``.

The credential is held in memory for one request and never logged.
"""

from __future__ import annotations

import http.client
import json
import os
import re
import socket
import uuid
from collections.abc import Callable, Mapping
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from .pathjail import PathJail
from .validation import ValidationError

PROTOCOL_VERSION = 1

# Parameter contracts (ActionParameterSpec in crates/local_control/src/catalog.rs
# and the *Params structs in protocol.rs). Value: (required keys, optional keys).
_PARAM_SPECS: dict[str, tuple[dict[str, type | tuple], dict[str, type | tuple]]] = {
    "none": ({}, {}),
    "action_name": ({"action": str}, {}),
    "binding_name": ({"binding_name": str}, {}),
    "boolean_value": ({"value": bool}, {}),
    "color_value": ({"color": str}, {}),
    "direction": ({"direction": str}, {}),
    "file_open": ({"path": str}, {"line": int, "column": int, "new_tab": bool}),
    "key": ({"key": str}, {}),
    "key_value": ({"key": str, "value": (str, int, float, bool, list, dict, type(None))}, {}),
    "namespace": ({}, {"namespace": str}),
    "page_query": ({}, {"page": str, "query": str}),
    "query": ({}, {"query": str}),
    "rename": ({"title": str}, {}),
    "resize": ({"direction": str}, {"amount": int}),
    "tab_activate": ({"mode": str}, {}),
    "tab_close": ({"mode": str}, {}),
    "tab_create": ({}, {"tab_type": str}),
    "text": ({"text": str}, {}),
    "theme_name": ({"theme_name": str}, {}),
}

_ENUMS = {
    "direction": {"left", "right", "up", "down", "previous", "next"},
    "mode@tab_activate": {"target", "previous", "next", "last"},
    "mode@tab_close": {"target", "active", "others", "right_of"},
    "tab_type": {"terminal", "agent", "cloud_agent", "default"},
}

# Every action in the catalog; all are `status: Implemented` at the time of
# writing. name -> parameter spec. Keep in sync with catalog.rs (a unit test
# parses catalog.rs and compares when the repo is available).
CATALOG: dict[str, str] = {
    "instance.list": "none",
    "instance.inspect": "none",
    "app.ping": "none",
    "app.version": "none",
    "app.active": "none",
    "app.focus": "none",
    "capability.list": "none",
    "capability.inspect": "action_name",
    "window.list": "none",
    "window.inspect": "none",
    "window.create": "tab_create",
    "window.focus": "none",
    "window.close": "none",
    "tab.list": "none",
    "tab.inspect": "none",
    "tab.create": "tab_create",
    "tab.activate": "tab_activate",
    "tab.move": "direction",
    "tab.close": "tab_close",
    "tab.rename": "rename",
    "tab.reset_name": "none",
    "tab.color.set": "color_value",
    "tab.color.clear": "none",
    "pane.list": "none",
    "pane.inspect": "none",
    "pane.split": "direction",
    "pane.focus": "none",
    "pane.navigate": "direction",
    "pane.resize": "resize",
    "pane.maximize": "none",
    "pane.unmaximize": "none",
    "pane.close": "none",
    "pane.rename": "rename",
    "pane.reset_name": "none",
    "session.list": "none",
    "session.inspect": "none",
    "session.activate": "none",
    "session.previous": "none",
    "session.next": "none",
    "session.reopen_closed": "none",
    "input.insert": "text",
    "input.replace": "text",
    "theme.list": "none",
    "theme.get": "none",
    "theme.set": "theme_name",
    "theme.system.set": "boolean_value",
    "theme.light.set": "theme_name",
    "theme.dark.set": "theme_name",
    "appearance.get": "none",
    "appearance.font_size.increase": "none",
    "appearance.font_size.decrease": "none",
    "appearance.font_size.reset": "none",
    "appearance.zoom.increase": "none",
    "appearance.zoom.decrease": "none",
    "appearance.zoom.reset": "none",
    "setting.list": "namespace",
    "setting.get": "key",
    "setting.set": "key_value",
    "setting.toggle": "key",
    "keybinding.list": "none",
    "keybinding.get": "binding_name",
    "action.list": "none",
    "action.inspect": "action_name",
    "surface.list": "none",
    "surface.settings.open": "page_query",
    "surface.command_palette.open": "query",
    "surface.command_search.open": "query",
    "surface.theme_picker.open": "none",
    "surface.keybindings.open": "none",
    "surface.warp_drive.open": "none",
    "surface.warp_drive.toggle": "none",
    "surface.resource_center.toggle": "none",
    "surface.ai_assistant.toggle": "none",
    "surface.code_review.open": "none",
    "surface.code_review.toggle": "none",
    "surface.project_explorer.open": "none",
    "surface.global_search.open": "none",
    "surface.conversation_list.open": "none",
    "surface.left_panel.toggle": "none",
    "surface.right_panel.toggle": "none",
    "surface.vertical_tabs.open": "none",
    "surface.vertical_tabs.toggle": "none",
    "surface.agent_management.open": "none",
    "file.open": "file_open",
}

# Actions that inject text into the terminal input; disabled with --no-input.
INPUT_ACTIONS = frozenset({"input.insert", "input.replace"})

_INSTANCE_ID_RE = re.compile(r"^inst_[0-9a-f]{32}$")


class WarpctrlError(RuntimeError):
    def __init__(self, message: str, code: str | None = None, details: Any = None):
        super().__init__(message)
        self.code = code
        self.details = details

    def to_dict(self) -> dict:
        return {"error": str(self), "code": self.code, "details": self.details}


def validate_target(target: Mapping[str, Any] | None) -> dict:
    """Validates a TargetSelector: {window|tab|pane|session: {type, id|index|title}}."""
    if not target:
        return {}
    if not isinstance(target, Mapping):
        raise ValidationError("target must be an object")
    allowed_types = {
        "window": {"active", "id", "index", "title"},
        "tab": {"active", "id", "index", "title"},
        "pane": {"active", "id", "index"},
        "session": {"active", "id"},
    }
    out: dict[str, dict] = {}
    for level, sel in target.items():
        if level not in allowed_types:
            raise ValidationError(f"unknown target level {level!r}")
        if not isinstance(sel, Mapping) or sel.get("type") not in allowed_types[level]:
            raise ValidationError(
                f"target.{level}.type must be one of {sorted(allowed_types[level])}"
            )
        t = sel["type"]
        extra = set(sel) - {"type", t} if t != "active" else set(sel) - {"type"}
        if extra:
            raise ValidationError(f"unexpected keys in target.{level}: {sorted(extra)}")
        if t == "active":
            out[level] = {"type": "active"}
        elif t == "index":
            v = sel.get("index")
            if not isinstance(v, int) or isinstance(v, bool) or not 0 <= v < 10000:
                raise ValidationError(f"target.{level}.index must be a non-negative integer")
            out[level] = {"type": "index", "index": v}
        else:
            v = sel.get(t)
            if not isinstance(v, str) or not v or len(v) > 256:
                raise ValidationError(f"target.{level}.{t} must be a non-empty string")
            out[level] = {"type": t, t: v}
    return out


def validate_action(
    action: str,
    params: Mapping[str, Any] | None,
    *,
    allow_input: bool = True,
    file_jail: PathJail | None = None,
) -> dict:
    """Allowlist + parameter validation. Returns the params to send."""
    if action not in CATALOG:
        raise ValidationError(f"action {action!r} is not in the local-control catalog")
    if action in INPUT_ACTIONS and not allow_input:
        raise ValidationError(f"{action} is disabled (server started with --no-input)")
    spec_name = CATALOG[action]
    required, optional = _PARAM_SPECS[spec_name]
    params = dict(params or {})
    unknown = set(params) - set(required) - set(optional)
    if unknown:
        raise ValidationError(f"{action}: unexpected params {sorted(unknown)}")
    for k in required:
        if k not in params:
            raise ValidationError(f"{action}: missing required param {k!r}")
    for k, v in params.items():
        typ = required.get(k) or optional.get(k)
        if typ is int and (isinstance(v, bool) or not isinstance(v, int) or v < 0):
            raise ValidationError(f"{action}: {k} must be a non-negative integer")
        if not isinstance(v, typ):  # type: ignore[arg-type]
            raise ValidationError(f"{action}: {k} has the wrong type")
        if isinstance(v, str) and len(v) > 4000:
            raise ValidationError(f"{action}: {k} is too long")
    enum_key = {"tab_activate": "mode@tab_activate", "tab_close": "mode@tab_close"}.get(spec_name)
    if enum_key and params["mode"] not in _ENUMS[enum_key]:
        raise ValidationError(f"{action}: mode must be one of {sorted(_ENUMS[enum_key])}")
    if "direction" in params and params["direction"] not in _ENUMS["direction"]:
        raise ValidationError(f"{action}: direction must be one of {sorted(_ENUMS['direction'])}")
    if "tab_type" in params and params["tab_type"] not in _ENUMS["tab_type"]:
        raise ValidationError(f"{action}: tab_type must be one of {sorted(_ENUMS['tab_type'])}")
    if action == "file.open":
        # The app would open (and display, i.e. let us screenshot) any file the
        # owner can read. Keep it inside the workspace jail, absolute only
        # (relative paths are resolved by the app against the session cwd).
        if file_jail is None:
            raise ValidationError("file.open is unavailable")
        if not os.path.isabs(params["path"]):
            raise ValidationError("file.open: path must be absolute")
        params["path"] = str(file_jail.resolve(params["path"]))
    return params


@dataclass
class Instance:
    instance_id: str
    pid: int
    channel: str
    app_id: str
    app_version: str | None
    executable_path: str | None
    port: int | None
    broker_socket: Path | None
    started_at: str | None

    def public(self) -> dict:
        return {
            "instance_id": self.instance_id,
            "pid": self.pid,
            "channel": self.channel,
            "app_id": self.app_id,
            "app_version": self.app_version,
            "executable_path": self.executable_path,
            "local_control_enabled": self.port is not None,
            "started_at": self.started_at,
        }


def discovery_dir(environ: Mapping[str, str] | None = None) -> Path:
    env = os.environ if environ is None else environ
    if env.get("WARP_LOCAL_CONTROL_DISCOVERY_DIR"):
        return Path(env["WARP_LOCAL_CONTROL_DISCOVERY_DIR"])
    if env.get("XDG_RUNTIME_DIR"):
        return Path(env["XDG_RUNTIME_DIR"]) / "warp" / "local-control"
    return Path(env.get("HOME", str(Path.home()))) / ".warp" / "local-control"


def parse_record(data: Mapping[str, Any], directory: Path) -> Instance | None:
    """Parses a discovery record, applying the same authority checks as the
    Rust client (`validate_local_control_authority`). Returns None if unsafe."""
    if data.get("protocol_version") != PROTOCOL_VERSION:
        return None
    iid = data.get("instance_id")
    if not isinstance(iid, str) or not _INSTANCE_ID_RE.match(iid):
        return None
    endpoint = data.get("endpoint")
    broker = data.get("credential_broker")
    port = None
    sock = None
    if endpoint is None and broker is None:
        pass
    elif endpoint is not None and broker is not None:
        if endpoint.get("host") != "127.0.0.1":
            return None
        p = endpoint.get("port")
        if not isinstance(p, int) or not 0 < p < 65536:
            return None
        if broker.get("socket_path") != f"{iid}.broker.sock":
            return None
        port = p
        sock = directory / f"{iid}.broker.sock"
    else:
        return None
    pid = data.get("pid")
    if not isinstance(pid, int) or pid <= 0:
        return None
    return Instance(
        instance_id=iid,
        pid=pid,
        channel=str(data.get("channel", "")),
        app_id=str(data.get("app_id", "")),
        app_version=data.get("app_version"),
        executable_path=data.get("executable_path"),
        port=port,
        broker_socket=sock,
        started_at=data.get("started_at"),
    )


def _pid_alive(pid: int) -> bool:
    try:
        os.kill(pid, 0)
        return True
    except ProcessLookupError:
        return False
    except PermissionError:
        return True


def list_instances(
    directory: Path, channel: str | None, pid_alive: Callable[[int], bool] = _pid_alive
) -> list[Instance]:
    out = []
    if not directory.is_dir():
        return out
    for entry in sorted(directory.glob("inst_*.json")):
        if entry.is_symlink():
            continue
        try:
            data = json.loads(entry.read_text())
        except (OSError, ValueError):
            continue
        inst = parse_record(data, directory)
        if inst is None or (channel and inst.channel != channel) or not pid_alive(inst.pid):
            continue
        out.append(inst)
    return out


class LocalControlClient:
    def __init__(self, directory: Path, timeout_s: float = 10.0):
        self._dir = directory
        self._timeout = timeout_s
        self._verified: set[tuple[str, int, int | None]] = set()

    def request_credential(self, inst: Instance, action: str) -> str:
        if inst.broker_socket is None:
            raise WarpctrlError(
                "local control is disabled in this Warpium instance; enable Settings > Scripting",
                "local_control_disabled",
            )
        req = json.dumps(
            {
                "protocol_version": PROTOCOL_VERSION,
                "request_id": str(uuid.uuid4()),
                "action": action,
            }
        ).encode()
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as s:
            s.settimeout(self._timeout)
            try:
                s.connect(str(inst.broker_socket))
            except OSError as e:
                raise WarpctrlError(
                    f"cannot reach credential broker: {e}", "transport_unavailable"
                ) from e
            s.sendall(req)
            s.shutdown(socket.SHUT_WR)
            chunks = []
            while True:
                b = s.recv(65536)
                if not b:
                    break
                chunks.append(b)
        text = b"".join(chunks).decode("utf-8", errors="replace")
        try:
            data = json.loads(text)
        except ValueError as e:
            raise WarpctrlError(
                "credential broker returned invalid JSON", "transport_unavailable"
            ) from e
        if isinstance(data, dict) and isinstance(data.get("bearer_token"), str):
            return data["bearer_token"]
        err = (data or {}).get("error") if isinstance(data, dict) else None
        if err:
            raise WarpctrlError(
                err.get("message", "credential denied"), err.get("code"), err.get("details")
            )
        raise WarpctrlError(
            "credential broker returned an unexpected response", "transport_unavailable"
        )

    def verify_identity(self, inst: Instance) -> None:
        key = (inst.instance_id, inst.pid, inst.port)
        if key in self._verified:
            return
        data = self.send(inst, "instance.inspect", {}, {})
        if (
            not isinstance(data, dict)
            or data.get("instance_id") != inst.instance_id
            or data.get("pid") != inst.pid
        ):
            raise WarpctrlError(
                "local-control endpoint answered with a different instance identity",
                "transport_unavailable",
            )
        self._verified.add(key)

    def send(self, inst: Instance, action: str, params: dict, target: dict) -> Any:
        token = self.request_credential(inst, action)
        body = json.dumps(
            {
                "protocol_version": PROTOCOL_VERSION,
                "request_id": str(uuid.uuid4()),
                "target": target,
                "action": {"kind": action, "params": params},
            }
        ).encode()
        conn = http.client.HTTPConnection("127.0.0.1", inst.port, timeout=self._timeout)
        try:
            # http.client sends Host: 127.0.0.1:<port> and no Origin, which is
            # what validate_loopback_headers() in the app requires.
            conn.request(
                "POST",
                "/v1/control",
                body=body,
                headers={
                    "Content-Type": "application/json",
                    "Authorization": f"Bearer {token}",
                },
            )
            resp = conn.getresponse()
            text = resp.read().decode("utf-8", errors="replace")
        except OSError as e:
            raise WarpctrlError(
                f"local-control request failed: {e}", "transport_unavailable"
            ) from e
        finally:
            conn.close()
            del token
        try:
            data = json.loads(text)
        except ValueError as e:
            raise WarpctrlError(
                f"HTTP {resp.status}: non-JSON response", "transport_unavailable", text[:500]
            ) from e
        response = data.get("response") if isinstance(data, dict) else None
        if isinstance(response, dict):
            if response.get("status") == "ok":
                return response.get("data")
            err = response.get("error") or {}
            raise WarpctrlError(err.get("message", "error"), err.get("code"), err.get("details"))
        if isinstance(data, dict) and isinstance(data.get("error"), dict):
            err = data["error"]
            raise WarpctrlError(err.get("message", "error"), err.get("code"), err.get("details"))
        raise WarpctrlError(f"HTTP {resp.status}: unexpected response", "transport_unavailable")
