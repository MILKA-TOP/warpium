# warpium-mac-mcp: security model

This server runs on the owner's real Mac and lets a remote agent (Claude Code in a
Docker container) build, launch, observe and drive Warpium. Read this before you run it.

## The short version

- Anyone who has the bearer token can do everything listed under "What it can do".
- The tools have narrow, validated arguments, and there is no shell tool. But two
  capabilities amount to running code as you, by design:
  1. **Building and running code from the fork.** `build`, `run_integration_test` and
     `app_launch(source="build")` compile and run whatever is on the synced branch of
     `MILKA-TOP/warpium`, including `build.rs` scripts. Anyone who can push a branch to the
     fork (including an agent with your `gh` credentials) can run code on your Mac this way.
  2. **Typing into a terminal.** Warpium is a terminal. `type_text("...\n")`, `press_keys("enter")`
     and the warpctrl `input.*` actions can type and run shell commands inside the Warpium window
     under test, as you.
- Start with `--no-build` and/or `--no-input` to switch these off when you don't need them.
- Treat the token like an SSH key. Stop the server (Ctrl+C) when you're not using it.

## What it can do

| Area | Capability | Limits |
|---|---|---|
| Repo | Clone/fetch `https://github.com/MILKA-TOP/warpium` into `~/warpium-mac-test/warpium` and check out a branch | Validated branch name. Refuses a clone whose `origin` is anything else. Refuses a dirty clone unless `discard_local_changes=true`, which resets tracked files and removes untracked, non-ignored files inside the clone only. |
| Build | `cargo build -p warp --bin warp-oss [--release] [--features ...]` | Fixed command. Profile, bin and feature names are allowlisted or validated against `app/Cargo.toml`. `CARGO_TARGET_DIR` is forced into the clone. |
| Tests | `cargo run -p integration --bin integration -- <name>` | Test name must be registered in `crates/integration/src/bin/integration.rs`. |
| Artifacts | `gh run download <run_id> --repo MILKA-TOP/warpium`, mount the .dmg read-only, copy the .app to `~/warpium-mac-test/apps/` | Numeric run id. Never writes to `/Applications`. Clears quarantine only on that private copy. |
| App | Launch `target/<profile>/warp-oss` from the clone, or the installed test app; quit (SIGTERM/SIGKILL) | Env limited to `RUST_LOG`, `RUST_BACKTRACE`, `RUST_LIB_BACKTRACE`, `WARP_*`, `WARPUI_*`. Args limited to `--flag[=value]`, with no positionals and no `--warpctrl`. Only processes whose executable is inside the workspace are ever targeted or killed. Your daily Warpium in `/Applications` is never touched. |
| Logs | Read `~/Library/Logs/warp-oss.log`, the launched app's stdout capture, job logs | Path jail (see below). |
| Screen | Capture **one Warpium window** at a time (`screencapture -l <window id>`) | Window must belong to a target PID. There is no full-screen capture. |
| Input | Mouse click/scroll, key combos and text, only into Warpium | See "Input guard". Can be disabled with `--no-input`. |
| warpctrl | Warp's local-control actions (tabs, panes, sessions, settings, themes, surfaces, input insert, file open) | Only names from the action catalog. Parameters are type-checked. `file.open` must use an absolute path inside the workspace. `input.*` is disabled under `--no-input`. Talks only to the Warpium instance under test. |

## What it cannot do

- Run arbitrary shell commands through a tool. Every subprocess uses a fixed program and an
  argument list, never `shell=True`.
- Read or write files outside `~/warpium-mac-test/` and the Warpium log file. Paths go through
  `realpath`, then a containment check, so `..` segments and symlinks pointing outside are rejected.
- Capture other apps' windows or the full screen.
- Send input to any app other than the Warpium process it launched, or one running from the
  workspace.
- Bind to `0.0.0.0` or `::`. Wildcard binds are refused outright.
- Touch `/Applications`, your daily Warpium or Warp data, or your other repos.

## Network and authentication

- The server binds `127.0.0.1:17654` by default. On Docker Desktop for Mac, containers reach
  host loopback services through `host.docker.internal`. Docker Desktop forwards that traffic
  from the host side, so a loopback-only bind is reachable from containers but not from your
  LAN. Note that **every** container on the machine can reach it, which is why the token
  matters.
- A bearer token is required on every HTTP request, including the MCP handshake and unknown
  paths. On first run it is generated with `secrets.token_urlsafe(32)` into
  `~/.config/warpium-mac-mcp/token` (dir 0700, file 0600, `O_EXCL|O_NOFOLLOW`). The server
  refuses to start if that file is a symlink, belongs to another user, or is group/world
  accessible. Tokens are compared with `hmac.compare_digest`.
- Requests that carry an `Origin` header are rejected, so browser pages can't reach the server.
  The Host header must be `127.0.0.1`, `localhost`, `[::1]` or `host.docker.internal` (plus
  `--allow-host` values) with the right port. Together these block DNS rebinding.
- Rotate the token with `uv run warpium-mac-mcp rotate-token`.

## Input guard

Implemented in `src/warpium_mac_mcp/ui.py` and unit-tested with a fake platform.

- **Immediately before every single event** (each mouse down/up, each key down/up, each text
  chunk), the frontmost application is read from the window server (`lsappinfo front`). If its
  PID is not the Warpium under test, the call stops with an error and nothing more is sent.
- Clicks and scrolls also require that the topmost on-screen window at that point belongs to
  Warpium. A notification banner, menu bar, Dock or another app's floating window covering the
  point causes a refusal. Coordinates outside the target window are rejected.
- Keyboard events are posted with `CGEventPostToPid(<warpium pid>)` by default. Even if focus
  changes between the check and the event, the keystroke can't land in another app. If Warpium
  ignores pid-targeted events on your macOS version, `--key-delivery hid` posts to the global
  HID tap instead, still behind the frontmost check.
- System shortcuts (`cmd+space`, `cmd+tab`, `ctrl+cmd+q`, `cmd+alt+esc`) are refused.

## Test-app isolation

- Debug builds launched with `isolate_data=true` (the default) get `WARP_DATA_PROFILE=mcp-test`.
  That gives them separate UserDefaults, Application Support, keychain items and a separate
  `~/.warp-oss-mcp-test` config dir (`crates/warp_core/src/{paths.rs,channel/state.rs}`).
- Release builds and CI artifacts ignore `WARP_DATA_PROFILE`, and share the oss-channel data
  (`~/.warp-oss`, the app's Application Support and keychain items) with any other Warpium/WarpOss
  of the same bundle id you use daily. Use the debug build when isolation matters.
- The test app runs as you, and its terminal sessions are ordinary shells.

## Audit log

Every tool call is appended as one JSON line to `~/.config/warpium-mac-mcp/audit.log` (0600),
with timestamp, tool name, arguments, success or error, and duration. `type_text(redact=true)` and
`warpctrl(redact=true)` replace the typed text or params with `<redacted>`. Rejected
authentication attempts are logged with the peer address. Local-control credentials obtained
from Warpium are held in memory for one request and never logged.

## macOS permissions

Grant these to **the terminal app that runs the server** (Terminal, iTerm2, Warp, ...), then
restart that terminal:

- **Screen Recording** (System Settings > Privacy & Security > Screen Recording): needed by
  `screencapture -l` for window screenshots and to read window titles.
- **Accessibility** (System Settings > Privacy & Security > Accessibility): needed to post
  synthetic mouse and keyboard events, and as a fallback to bring Warpium to the front.

These permissions apply to everything run from that terminal, not just this server. If you'd
rather not grant them to your main terminal, run the server from a dedicated terminal app that
you use only for this.

`uv run warpium-mac-mcp doctor` reports whether both are granted.

## Running it

- Default: run it in the foreground in a terminal; Ctrl+C stops it and cancels running jobs.
  A launchd plist example is in `launchd/`. It is optional and never installed automatically.
- Run the server from a checkout you have reviewed, **not** from the workspace clone it manages.
  `repo_sync` rewrites that clone to whatever branch the agent asks for.
