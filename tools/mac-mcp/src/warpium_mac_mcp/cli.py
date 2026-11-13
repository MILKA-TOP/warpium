"""Command-line entry point: `warpium-mac-mcp [serve|doctor|client-config|rotate-token]`."""

from __future__ import annotations

import argparse
import ipaddress
import json
import logging
import sys
from pathlib import Path

from .config import DEFAULT_HOST, DEFAULT_PORT, Config, load_or_create_token


def _add_common(p: argparse.ArgumentParser) -> None:
    p.add_argument(
        "--host",
        default=DEFAULT_HOST,
        help="interface to bind (default 127.0.0.1; wildcard addresses are refused)",
    )
    p.add_argument("--port", type=int, default=DEFAULT_PORT)
    p.add_argument(
        "--workspace", type=Path, default=None, help="workspace dir (default ~/warpium-mac-test)"
    )
    p.add_argument(
        "--config-dir",
        type=Path,
        default=None,
        help="token/audit dir (default ~/.config/warpium-mac-mcp)",
    )


def _config(args: argparse.Namespace) -> Config:
    kw = {"host": args.host, "port": args.port}
    if args.workspace:
        kw["workspace_dir"] = args.workspace.expanduser().resolve()
    if args.config_dir:
        kw["config_dir"] = args.config_dir.expanduser().resolve()
    for name in ("allow_input", "allow_build", "vm_mode"):
        if hasattr(args, name):
            kw[name] = getattr(args, name)
    if getattr(args, "allow_host", None):
        kw["extra_allowed_hosts"] = tuple(args.allow_host)
    return Config(**kw)


def check_bind_host(host: str) -> None:
    """Refuses wildcard binds: the server would be reachable from the LAN."""
    if host in ("localhost",):
        return
    try:
        ip = ipaddress.ip_address(host)
    except ValueError as e:
        raise SystemExit(f"--host must be an IP address (got {host!r})") from e
    if ip.is_unspecified:
        raise SystemExit(
            "refusing to bind a wildcard address (0.0.0.0 / ::). Bind 127.0.0.1 (default) or one "
            "specific interface address; see README 'Networking'."
        )


def check_vm_mode(signals: dict | None = None) -> None:
    """--vm-mode relaxes the screen/input guards; refuse it outside a macOS VM
    guest so the relaxations can never apply to a real desktop."""
    from .doctor import is_vm, vm_signals

    signals = vm_signals() if signals is None else signals
    if is_vm(signals) is not True:
        raise SystemExit(
            "refusing --vm-mode: this machine is not a macOS VM guest "
            f"(kern.hv_vmm_present={signals.get('hv_vmm_present')!r}, "
            f"hw.model={signals.get('model')!r}; a VM reports 1 and VirtualMac*). "
            "VM mode lifts the protections for your real desktop; run without it on a host."
        )


def cmd_serve(args: argparse.Namespace) -> int:
    check_bind_host(args.host)
    if args.vm_mode:
        check_vm_mode()
    cfg = _config(args)
    logging.basicConfig(
        level=logging.INFO, format="%(asctime)s %(levelname)s %(name)s: %(message)s"
    )
    token = load_or_create_token(cfg.token_path)

    import socket

    import uvicorn

    from .server import build_asgi_app, build_services, make_platform

    # Fail before touching any state if another server already has the port
    # (it would otherwise overwrite that server's permission record and exit).
    with socket.socket(socket.AF_INET6 if ":" in cfg.host else socket.AF_INET) as s:
        try:
            s.bind((cfg.host, cfg.port))
        except OSError as e:
            raise SystemExit(
                f"cannot bind {cfg.host}:{cfg.port}: {e} (is a server already running?)"
            ) from None

    platform = make_platform(args.key_delivery)
    services = build_services(cfg, platform)
    from .doctor import clear_server_permissions, record_server_permissions

    perms = record_server_permissions(cfg, platform)
    services.audit.write(
        "server_start",
        host=cfg.host,
        port=cfg.port,
        workspace=str(cfg.workspace_dir),
        allow_input=cfg.allow_input,
        allow_build=cfg.allow_build,
        key_delivery=args.key_delivery,
        vm_mode=cfg.vm_mode,
    )
    app = build_asgi_app(services, token)
    print(f"warpium-mac-mcp listening on http://{cfg.host}:{cfg.port}/mcp", file=sys.stderr)
    print(
        f"  token file: {cfg.token_path}  (print client setup: warpium-mac-mcp client-config)",
        file=sys.stderr,
    )
    print(f"  workspace:  {cfg.workspace_dir}", file=sys.stderr)
    print(f"  audit log:  {cfg.audit_log_path}", file=sys.stderr)
    if cfg.vm_mode:
        print(
            "  VM mode:    ON (whole-screen capture and input to system dialogs allowed)",
            file=sys.stderr,
        )
    if platform is None:
        print(
            "  WARNING: macOS APIs unavailable; screenshot/input tools will fail", file=sys.stderr
        )
    elif perms is not None:
        for key, label in (
            ("screen_recording", "Screen Recording"),
            ("accessibility", "Accessibility"),
        ):
            if perms[key] is False:
                print(f"  WARNING: {label} permission not granted to this process", file=sys.stderr)
    print("  Ctrl+C to stop.", file=sys.stderr)
    try:
        uvicorn.run(
            app,
            host=cfg.host,
            port=cfg.port,
            log_level="info",
            proxy_headers=False,
            server_header=False,
            lifespan="on",
            ws="none",
        )
    finally:
        services.jobs.cancel_all()
        clear_server_permissions(cfg)
        services.audit.write("server_stop")
    return 0


def cmd_doctor(args: argparse.Namespace) -> int:
    from .doctor import format_checks, load_server_permissions, run_checks, summarize
    from .server import make_platform

    cfg = _config(args)
    checks = run_checks(
        cfg,
        make_platform(),
        check_port=not args.no_port_check,
        server_permissions=load_server_permissions(cfg),
    )
    print(format_checks(checks))
    return 0 if summarize(checks)["all_required_ok"] else 1


def format_server_status(perms: dict | None) -> tuple[str, int]:
    """Text and exit code for `server-status`: 0 both permissions granted,
    1 a permission is missing or unknown, 2 no running server found."""
    if perms is None:
        return (
            "no running server found (no live permission record; is the server running "
            "under this user and config dir?)",
            2,
        )

    def state(v: object) -> str:
        return "unknown" if v is None else ("granted" if v else "NOT granted")

    lines = [
        f"server pid {perms.get('pid')} ({perms.get('exe')}), checked {perms.get('checked_at')}",
        f"  Screen Recording: {state(perms.get('screen_recording'))}",
        f"  Accessibility:    {state(perms.get('accessibility'))}",
        f"  VM mode:          {'on' if perms.get('vm_mode') else 'off'}",
    ]
    good = perms.get("screen_recording") is True and perms.get("accessibility") is True
    return "\n".join(lines), 0 if good else 1


def cmd_server_status(args: argparse.Namespace) -> int:
    import time

    from .doctor import load_server_permissions

    cfg = _config(args)
    deadline = time.monotonic() + max(0.0, args.wait)
    perms = load_server_permissions(cfg)
    while perms is None and time.monotonic() < deadline:
        time.sleep(0.5)
        perms = load_server_permissions(cfg)
    if args.json:
        print(json.dumps(perms))
        _, rc = format_server_status(perms)
    else:
        text, rc = format_server_status(perms)
        print(text)
    return rc


def client_config_text(
    cfg: Config,
    token: str,
    name: str = "warpium-mac",
    host_from_container: str = "host.docker.internal",
) -> str:
    url = f"http://{host_from_container}:{cfg.port}/mcp"
    snippet = {
        "mcpServers": {
            name: {"type": "http", "url": url, "headers": {"Authorization": f"Bearer {token}"}}
        }
    }
    return (
        "# Run inside the container (adds the server to Claude Code, user scope):\n"
        f"claude mcp add --transport http --scope user {name} {url} "
        f'--header "Authorization: Bearer {token}"\n\n'
        "# Or put this in a git-ignored .mcp.json / ~/.claude.json (never commit it):\n"
        f"{json.dumps(snippet, indent=2)}\n\n"
        "# Connectivity check from the container (expects HTTP 401 without the token):\n"
        f"curl -s -o /dev/null -w '%{{http_code}}\\n' -X POST {url}\n"
    )


def cmd_client_config(args: argparse.Namespace) -> int:
    cfg = _config(args)
    token = load_or_create_token(cfg.token_path)
    print(client_config_text(cfg, token, host_from_container=args.container_host))
    return 0


def cmd_rotate_token(args: argparse.Namespace) -> int:
    cfg = _config(args)
    if cfg.token_path.exists():
        cfg.token_path.unlink()
    load_or_create_token(cfg.token_path)
    print(f"new token written to {cfg.token_path}; restart the server and re-run client-config")
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(prog="warpium-mac-mcp", description=__doc__)
    sub = parser.add_subparsers(dest="cmd")

    p_serve = sub.add_parser("serve", help="run the MCP server in the foreground (default)")
    _add_common(p_serve)
    p_serve.add_argument(
        "--no-input",
        dest="allow_input",
        action="store_false",
        help="disable click/type/keys/scroll and warpctrl input.* actions",
    )
    p_serve.add_argument(
        "--no-build",
        dest="allow_build",
        action="store_false",
        help="disable build, run_integration_test and launching local builds",
    )
    p_serve.add_argument(
        "--key-delivery",
        choices=("pid", "hid"),
        default="pid",
        help="keyboard events to the Warpium pid only (default) or global HID tap",
    )
    p_serve.add_argument(
        "--allow-host",
        action="append",
        default=[],
        help="extra Host header name to accept (e.g. a VM gateway IP)",
    )
    p_serve.add_argument(
        "--vm-mode",
        action="store_true",
        help="inside a macOS VM guest only (refused elsewhere): allow whole-screen "
        "screenshots and input to system dialogs (vm/provision.sh sets it)",
    )
    p_serve.set_defaults(func=cmd_serve)

    p_st = sub.add_parser(
        "server-status",
        help="permissions the running server process itself has (from its own record)",
    )
    _add_common(p_st)
    p_st.add_argument(
        "--wait", type=float, default=0.0, help="seconds to wait for a running server's record"
    )
    p_st.add_argument("--json", action="store_true", help="print the raw record as JSON")
    p_st.set_defaults(func=cmd_server_status)

    p_doc = sub.add_parser("doctor", help="check prerequisites and permissions")
    _add_common(p_doc)
    p_doc.add_argument(
        "--no-port-check",
        action="store_true",
        help="skip the 'port free' check (use while the server is already running)",
    )
    p_doc.set_defaults(func=cmd_doctor)

    p_cc = sub.add_parser("client-config", help="print the container-side `claude mcp add` command")
    _add_common(p_cc)
    p_cc.add_argument("--container-host", default="host.docker.internal")
    p_cc.set_defaults(func=cmd_client_config)

    p_rot = sub.add_parser("rotate-token", help="generate a new bearer token")
    _add_common(p_rot)
    p_rot.set_defaults(func=cmd_rotate_token)

    argv = sys.argv[1:] if argv is None else argv
    if not argv or argv[0].startswith("-"):
        argv = ["serve", *argv]
    args = parser.parse_args(argv)
    return args.func(args)


if __name__ == "__main__":
    raise SystemExit(main())
