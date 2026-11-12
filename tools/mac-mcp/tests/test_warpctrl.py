import json
import re
import socket
import threading
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path

import pytest

from warpium_mac_mcp.pathjail import PathEscapeError, PathJail
from warpium_mac_mcp.validation import ValidationError
from warpium_mac_mcp.warpctrl import (
    CATALOG,
    LocalControlClient,
    WarpctrlError,
    discovery_dir,
    list_instances,
    parse_record,
    validate_action,
    validate_target,
)

CATALOG_RS = Path(__file__).resolve().parents[3] / "crates" / "local_control" / "src" / "catalog.rs"
IID = "inst_" + "a" * 32


def _snake(name: str) -> str:
    return re.sub(r"(?<!^)(?=[A-Z])", "_", name).lower()


@pytest.mark.skipif(not CATALOG_RS.exists(), reason="repo catalog.rs not available")
def test_catalog_matches_rust_source():
    src = CATALOG_RS.read_text()
    entries = re.findall(
        r'name:\s*"([a-z_.]+)",\s*status:\s*(\w+),\s*target:\s*\w+,\s*params:\s*(\w+)', src
    )
    assert entries, "failed to parse catalog.rs"
    rust = {name: (status, _snake(params)) for name, status, params in entries}
    assert set(rust) == set(CATALOG), "CATALOG drifted from catalog.rs"
    for name, (status, params) in rust.items():
        assert CATALOG[name] == params, name
        assert status == "Implemented", f"{name} is {status}; drop it from the allowlist"


def test_unknown_action_rejected():
    for bad in ["shell.exec", "tab.create;rm", "", "TAB.CREATE", "input.insert "]:
        with pytest.raises(ValidationError):
            validate_action(bad, None)


def test_params_validated():
    assert validate_action("tab.create", None) == {}
    assert validate_action("tab.create", {"tab_type": "terminal"}) == {"tab_type": "terminal"}
    with pytest.raises(ValidationError):
        validate_action("tab.create", {"tab_type": "evil"})
    with pytest.raises(ValidationError):
        validate_action("tab.create", {"cwd": "/"})
    with pytest.raises(ValidationError):
        validate_action("pane.split", {})
    assert validate_action("pane.split", {"direction": "right"})
    with pytest.raises(ValidationError):
        validate_action("pane.split", {"direction": "diagonal"})
    with pytest.raises(ValidationError):
        validate_action("tab.close", {"mode": "all"})
    assert validate_action("tab.close", {"mode": "right_of"})
    with pytest.raises(ValidationError):
        validate_action("pane.resize", {"direction": "left", "amount": -1})
    with pytest.raises(ValidationError):
        validate_action("pane.resize", {"direction": "left", "amount": True})
    with pytest.raises(ValidationError):
        validate_action("theme.system.set", {"value": "yes"})
    assert validate_action("setting.set", {"key": "a.b", "value": {"x": [1]}})


def test_input_actions_disabled_with_no_input():
    assert validate_action("input.insert", {"text": "ls"})
    with pytest.raises(ValidationError):
        validate_action("input.insert", {"text": "ls"}, allow_input=False)
    with pytest.raises(ValidationError):
        validate_action("input.replace", {"text": "ls"}, allow_input=False)


def test_file_open_jailed(tmp_path):
    ws = tmp_path / "ws"
    ws.mkdir()
    (ws / "a.md").write_text("hi")
    jail = PathJail([ws])
    p = validate_action("file.open", {"path": str(ws / "a.md")}, file_jail=jail)
    assert p["path"] == str((ws / "a.md").resolve())
    with pytest.raises(PathEscapeError):
        validate_action("file.open", {"path": "/etc/passwd"}, file_jail=jail)
    with pytest.raises(ValidationError):
        validate_action("file.open", {"path": "a.md"}, file_jail=jail)
    with pytest.raises(ValidationError):
        validate_action("file.open", {"path": str(ws / "a.md")}, file_jail=None)


def test_target_validation():
    assert validate_target(None) == {}
    assert validate_target({"tab": {"type": "index", "index": 2}}) == {
        "tab": {"type": "index", "index": 2}
    }
    assert validate_target({"pane": {"type": "id", "id": "p1"}, "window": {"type": "active"}})
    for bad in [
        {"tab": {"type": "title"}},
        {"session": {"type": "index", "index": 1}},
        {"pane": {"type": "title", "title": "x"}},
        {"tab": {"type": "index", "index": -1}},
        {"tab": {"type": "id", "id": "x", "extra": 1}},
        {"galaxy": {"type": "active"}},
        {"tab": "active"},
    ]:
        with pytest.raises(ValidationError):
            validate_target(bad)


def _record(**over):
    r = {
        "protocol_version": 1,
        "instance_id": IID,
        "pid": 4242,
        "channel": "warp-oss",
        "app_id": "dev.warp.WarpOss",
        "app_version": None,
        "started_at": "2026-01-01T00:00:00Z",
        "executable_path": "/x/warp-oss",
        "endpoint": {"host": "127.0.0.1", "port": 5555},
        "credential_broker": {"socket_path": f"{IID}.broker.sock"},
        "actions": [],
    }
    r.update(over)
    return r


def test_parse_record_authority_checks(tmp_path):
    assert parse_record(_record(), tmp_path).port == 5555
    assert parse_record(_record(endpoint={"host": "10.0.0.1", "port": 5555}), tmp_path) is None
    assert (
        parse_record(_record(credential_broker={"socket_path": "/tmp/evil.sock"}), tmp_path) is None
    )
    assert (
        parse_record(_record(credential_broker={"socket_path": "../x.broker.sock"}), tmp_path)
        is None
    )
    assert parse_record(_record(endpoint=None), tmp_path) is None  # inconsistent
    disabled = parse_record(_record(endpoint=None, credential_broker=None), tmp_path)
    assert disabled is not None and disabled.port is None
    assert parse_record(_record(instance_id="../../etc"), tmp_path) is None
    assert parse_record(_record(protocol_version=2), tmp_path) is None


def test_list_instances_filters(tmp_path):
    (tmp_path / f"{IID}.json").write_text(json.dumps(_record()))
    other = "inst_" + "b" * 32
    (tmp_path / f"{other}.json").write_text(
        json.dumps(
            _record(
                instance_id=other,
                channel="stable",
                credential_broker={"socket_path": f"{other}.broker.sock"},
            )
        )
    )
    (tmp_path / "inst_bad.json").write_text("{not json")
    found = list_instances(tmp_path, "warp-oss", pid_alive=lambda p: True)
    assert [i.instance_id for i in found] == [IID]
    assert list_instances(tmp_path, "warp-oss", pid_alive=lambda p: False) == []


def test_discovery_dir_resolution():
    assert discovery_dir({"WARP_LOCAL_CONTROL_DISCOVERY_DIR": "/d"}) == Path("/d")
    assert discovery_dir({"XDG_RUNTIME_DIR": "/run/u"}) == Path("/run/u/warp/local-control")
    assert discovery_dir({"HOME": "/Users/me"}) == Path("/Users/me/.warp/local-control")


class _FakeWarp:
    """Fake app side: credential broker on a Unix socket + /v1/control on loopback."""

    def __init__(self, directory: Path):
        self.dir = directory
        self.issued = {}
        self.requests = []
        self.sock_path = directory / f"{IID}.broker.sock"
        self.broker = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.broker.bind(str(self.sock_path))
        self.broker.listen(4)
        threading.Thread(target=self._serve_broker, daemon=True).start()
        fake = self

        class H(BaseHTTPRequestHandler):
            def log_message(self, *a):
                pass

            def do_POST(self):
                body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
                fake.requests.append((dict(self.headers), body))
                auth = self.headers.get("Authorization", "")
                ok_host = self.headers.get("Host") == f"127.0.0.1:{self.server.server_port}"
                token = auth.removeprefix("Bearer ")
                if (
                    self.headers.get("Origin")
                    or not ok_host
                    or fake.issued.get(token) != body["action"]["kind"]
                ):
                    resp = {
                        "protocol_version": 1,
                        "error": {"code": "unauthorized_local_client", "message": "denied"},
                    }
                else:
                    resp = {
                        "protocol_version": 1,
                        "request_id": body["request_id"],
                        "response": {"status": "ok", "data": {"echo": body["action"]}},
                    }
                data = json.dumps(resp).encode()
                self.send_response(200)
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)

        self.http = HTTPServer(("127.0.0.1", 0), H)
        threading.Thread(target=self.http.serve_forever, daemon=True).start()
        self.port = self.http.server_port

    def _serve_broker(self):
        while True:
            try:
                conn, _ = self.broker.accept()
            except OSError:
                return
            with conn:
                buf = b""
                while True:
                    b = conn.recv(4096)
                    if not b:
                        break
                    buf += b
                req = json.loads(buf)
                if req["action"] == "window.close":
                    resp = {
                        "protocol_version": 1,
                        "error": {"code": "local_control_disabled", "message": "disabled"},
                    }
                else:
                    tok = f"tok-{len(self.issued)}"
                    self.issued[tok] = req["action"]
                    resp = {"bearer_token": tok, "grant": {"action": req["action"]}}
                conn.sendall(json.dumps(resp).encode())

    def close(self):
        self.broker.close()
        self.http.shutdown()


def test_client_roundtrip(tmp_path):
    fake = _FakeWarp(tmp_path)
    try:
        inst = parse_record(_record(endpoint={"host": "127.0.0.1", "port": fake.port}), tmp_path)
        client = LocalControlClient(tmp_path)
        data = client.send(
            inst, "tab.create", {"tab_type": "terminal"}, {"window": {"type": "active"}}
        )
        assert data == {"echo": {"kind": "tab.create", "params": {"tab_type": "terminal"}}}
        headers, body = fake.requests[0]
        assert "Origin" not in headers
        assert body["target"] == {"window": {"type": "active"}}
        assert body["protocol_version"] == 1
        with pytest.raises(WarpctrlError) as e:
            client.send(inst, "window.close", {}, {})
        assert e.value.code == "local_control_disabled"
    finally:
        fake.close()


def test_client_disabled_instance(tmp_path):
    inst = parse_record(_record(endpoint=None, credential_broker=None), tmp_path)
    with pytest.raises(WarpctrlError):
        LocalControlClient(tmp_path).send(inst, "app.ping", {}, {})
