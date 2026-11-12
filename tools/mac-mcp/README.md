# warpium-mac-mcp

An MCP server you run **on your Mac**. Claude Code inside a Docker container connects to it
over HTTP (`host.docker.internal`) and uses it to build, launch, observe, drive and test
Warpium on the real machine. That way the agent can reproduce UI bugs and verify fixes it
can't see from Linux.

This is a fork-only tool. It doesn't touch the Rust code. **Read [SECURITY.md](SECURITY.md)
first**: this server can build and run code from the fork, and it can type into a terminal.

## Quickstart (Mac host)

### 1. Install uv

```sh
brew install uv            # or: curl -LsSf https://astral.sh/uv/install.sh | sh
```

### 2. Get the server code (a reviewed checkout, separate from the workspace)

The server manages its own clone in `~/warpium-mac-test/warpium` and resets it to whatever
branch the agent syncs. Don't run the server from that clone. Use a separate checkout that
you update yourself:

```sh
git clone --filter=blob:none --sparse https://github.com/MILKA-TOP/warpium ~/src/warpium-mcp
cd ~/src/warpium-mcp && git sparse-checkout set tools/mac-mcp
cd tools/mac-mcp
```

### 3. Run it (foreground; Ctrl+C stops it)

```sh
uv run warpium-mac-mcp            # = serve; binds 127.0.0.1:17654
```

On first run this creates `~/.config/warpium-mac-mcp/token` (mode 0600) and the workspace
`~/warpium-mac-test/`. Every tool call is logged to `~/.config/warpium-mac-mcp/audit.log`.

Useful flags: `--port N`, `--workspace DIR`, `--no-input` (observe only), `--no-build`
(only CI artifacts can be run), `--key-delivery hid` (see SECURITY.md).

### 4. Grant macOS permissions to the terminal app that runs the server

- System Settings > Privacy & Security > **Screen Recording**: enable your terminal app.
- System Settings > Privacy & Security > **Accessibility**: enable your terminal app.
- Quit and reopen the terminal, then start the server again.

### 5. Check prerequisites

```sh
uv run warpium-mac-mcp doctor
```

To build Warpium locally, the workspace needs:

- **Rust 1.92** through rustup (`curl https://sh.rustup.rs -sSf | sh`). The repo's
  `rust-toolchain.toml` pins `1.92.0`, and rustup installs it on first build.
- **Xcode 26** with the **Metal toolchain**: install Xcode from the App Store, run
  `sudo xcode-select -s /Applications/Xcode.app`, then
  `xcodebuild -downloadComponent MetalToolchain` (this is what `script/macos/install_build_deps`
  does).
- **protoc**: `brew install protobuf`.
- **gh** (only for `install_artifact`): `brew install gh && gh auth login`.

The workspace clone is created by the `repo_sync` tool on first use. Or create it yourself:
`git clone https://github.com/MILKA-TOP/warpium ~/warpium-mac-test/warpium`.

### 6. Connect from the container

On the Mac, print the exact command (it contains the token):

```sh
./scripts/print-client-config.sh      # = uv run warpium-mac-mcp client-config
```

It prints something like the following. Run it **inside the container**:

```sh
claude mcp add --transport http --scope user warpium-mac http://host.docker.internal:17654/mcp \
  --header "Authorization: Bearer <token>"
```

It also prints an equivalent `.mcp.json` snippet. If you use that snippet, keep the file out
of git (`tools/mac-mcp/.gitignore` ignores `.mcp.json` there). Never commit the token.

If the container routes HTTP through a proxy (`HTTP_PROXY` is set), add the host to
`NO_PROXY`, otherwise requests to `host.docker.internal` go to the proxy:

```sh
export NO_PROXY="host.docker.internal,${NO_PROXY:-}"
```

Connectivity check from the container. `401` means you reached the server and auth works:

```sh
curl -s --noproxy host.docker.internal -o /dev/null -w '%{http_code}\n' -X POST http://host.docker.internal:17654/mcp
```

### 7. Smoke test (ask the agent, or call the tools yourself)

1. `doctor`: required checks are ok and both permissions are granted.
2. `repo_sync(branch="master")`, then poll `job_status` until `succeeded`.
3. `build()` (debug, features `gui,warp_control_cli`), then poll `job_status`/`job_logs`. The
   first build takes a long time.
4. `app_launch()`: a Warpium window appears. `app_status` shows its pid and window.
5. `screenshot()` returns an image of **only** the Warpium window.
6. Click another app so it's frontmost, then `press_keys("cmd+t")`. It must be **refused**.
   Then `focus_app()` and `press_keys("cmd+t")`: a new tab opens.
7. `type_text("echo hello\n")`: the command runs in Warpium (verify with `screenshot`).
8. In the test app, open Settings > Scripting and set **warpctrl CLI** to **Enabled** (once per
   data profile). Then `list_targets()` returns windows/tabs/panes/sessions, and
   `warpctrl("tab.create", {"tab_type": "terminal"})` opens a tab.
9. `app_logs(tail=50)` shows `~/Library/Logs/warp-oss.log`.
10. `app_quit()`.
11. Check `~/.config/warpium-mac-mcp/audit.log` for those calls.

## Tools

| Tool | What it does |
|---|---|
| `doctor` | Checks toolchain, gh, Xcode/Metal, protoc, permissions, workspace, log and discovery paths. |
| `repo_sync(branch, discard_local_changes=false)` | Fetches the fork and checks out `branch` at `origin/branch` in the workspace clone (job). Refuses a dirty clone unless told to discard local changes. |
| `build(profile, bin, features)` | `cargo build -p warp --bin warp-oss` (job). Default features are `gui,warp_control_cli`. |
| `job_status(job_id?)`, `job_logs(job_id, tail, grep)`, `job_cancel(job_id)` | Inspect and cancel background jobs. |
| `install_artifact(run_id, artifact_name?)` | `gh run download` of the Warpium .dmg from a fork Actions run. Mounts it read-only and copies the .app into `~/warpium-mac-test/apps/` (job). |
| `run_integration_test(name, real_display)` | `cargo run -p integration --bin integration -- <name>` (job). |
| `app_launch(source, env, args, profile, isolate_data)` | Runs the dev binary (`build`) or the installed artifact (`artifact`). |
| `app_quit`, `app_status` | Stops the app under test, or reports its pid, windows, frontmost state and warpctrl instance. |
| `app_logs(tail, grep, since, source)` | `~/Library/Logs/warp-oss.log`, or the launched app's stdout/stderr. |
| `list_windows` | Warpium windows with ids and bounds. |
| `screenshot(window, max_width)` | PNG of one Warpium window (max 1600 px wide by default) plus bounds and scale. |
| `focus_app` | Brings Warpium to the front. |
| `click(x, y, button, count, window, from_screenshot)` | Window-relative click. |
| `scroll(x, y, dy, dx, ...)` | Scrolls by lines at a window point. |
| `type_text(text, redact)` | Types text. `\n` is sent as Return. |
| `press_keys(combo)` | Key combos such as `cmd+t`, `cmd+shift+]`, `ctrl+c`, `enter`. |
| `warpctrl(action, params, target)` | One local-control catalog action. |
| `warpctrl_catalog` | Allowed actions, parameter shapes and enums. |
| `list_targets` | `app.active` plus window, tab, pane and session lists. |
| `get_metadata(scope, target)` | `<scope>.inspect` for window, tab, pane, session or instance. |

Coordinates: `click` and `scroll` take **window-relative points**. `screenshot` returns
`image_px_per_window_point`; divide pixel coordinates by it, or pass `from_screenshot=true`
to use pixels from the last screenshot of that window.

## warpctrl (local control): what works

The server speaks Warp's local-control protocol directly, with no CLI wrapper needed. The steps
are discovery records in `~/.warp/local-control/`, then a credential from the per-instance
Unix broker socket, then a POST to `127.0.0.1:<port>/v1/control`. It talks only to the
instance whose pid is the Warpium under test.

Requirements:

- The binary must be built with the `warp_control_cli` cargo feature. `build()` includes it by
  default. The oss channel does **not** enable it otherwise, and the fork's CI .dmg is bundled
  without it, so warpctrl is unavailable for `source="artifact"` unless that workflow adds the
  feature.
- In the running test app: Settings > Scripting > **warpctrl CLI** set to **Enabled**. It is
  stored in the keychain per data profile, so you only do this once for `mcp-test`.

All 84 catalog actions in `crates/local_control/src/catalog.rs` are marked `Implemented` and
allowlisted (a unit test fails if the Python allowlist drifts from `catalog.rs`):

- instance/app: `instance.list`, `instance.inspect`, `app.ping`, `app.version`, `app.active`, `app.focus`
- capability/action: `capability.list|inspect`, `action.list|inspect`
- window: `window.list|inspect|create|focus|close`
- tab: `tab.list|inspect|create|activate|move|close|rename|reset_name`, `tab.color.set|clear`
- pane: `pane.list|inspect|split|focus|navigate|resize|maximize|unmaximize|close|rename|reset_name`
- session: `session.list|inspect|activate|previous|next|reopen_closed`
- input: `input.insert|replace` (disabled with `--no-input`)
- theme/appearance: `theme.list|get|set`, `theme.system.set`, `theme.light.set`, `theme.dark.set`,
  `appearance.get`, `appearance.font_size.increase|decrease|reset`, `appearance.zoom.increase|decrease|reset`
- settings/keybindings: `setting.list|get|set|toggle`, `keybinding.list|get`
- surfaces: `surface.list` and `surface.*.open|toggle` (settings, command palette, command search,
  theme picker, keybindings, Warp Drive, resource center, AI assistant, code review, project
  explorer, global search, conversation list, left/right panel, vertical tabs, agent management)
- `file.open`, restricted to absolute paths inside `~/warpium-mac-test/`

Known gap: session, pane and tab metadata contain ids, indices and active flags, but **no
working directory**. To check "Open in new tab opens `~`", open the tab (`tab.create` or the UI
path), then read the cwd from the UI: run `pwd` with `type_text("pwd\n")` and `screenshot()`,
or use `app_logs`.

## Networking

The default bind is `127.0.0.1`. On Docker Desktop for Mac, `host.docker.internal` resolves
inside containers to a Docker Desktop address. Docker Desktop's host-side process forwards
those connections to the Mac's loopback, so a server listening only on `127.0.0.1` is reachable
from containers and not from the LAN. Verify with the `curl` check above.

If that doesn't work on your setup (e.g. a different VM backend such as Colima/OrbStack, or
"host networking" settings), use the smallest alternative that works:

1. **Bind to the VM bridge interface only.** Find the host-side address the VM talks to (e.g.
   `ifconfig bridge100`, often `192.168.64.1`), then run
   `uv run warpium-mac-mcp --host 192.168.64.1 --allow-host 192.168.64.1` and connect to
   `http://192.168.64.1:17654/mcp` from the container. That interface is local to the
   VM network.
2. Keep `127.0.0.1` and forward a port into the container through an SSH tunnel you start.

Never bind `0.0.0.0`; the server refuses to.

## Development

```sh
uv sync
uv run pytest            # unit tests run on Linux too (macOS APIs are faked)
uv run ruff check . && uv run ruff format --check .
```

Layout: `config.py` (paths, token), `auth.py` (bearer middleware), `pathjail.py`,
`validation.py`, `jobs.py`, `workspace.py` (repo/build/artifact/tests), `app.py` (process and
logs), `platform.py` (macOS APIs), `ui.py` (window and input guard), `warpctrl.py`
(local-control client), `server.py` (MCP tools), `doctor.py`, `cli.py`.
