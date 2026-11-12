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
  `--no-build` blocks `build`, `run_integration_test`, `install_artifact` (a CI artifact is
  fork code too) and launching local builds. Only an app you put in `~/warpium-mac-test/apps/`
  yourself can then be launched.
- Treat the token like an SSH key. Stop the server (Ctrl+C) when you're not using it.

## What it can do

| Area | Capability | Limits |
|---|---|---|
| Repo | Clone/fetch `https://github.com/MILKA-TOP/warpium` into `~/warpium-mac-test/warpium` and check out a branch | Validated branch name. Refuses a clone whose `origin` is anything else. Refuses a dirty clone unless `discard_local_changes=true`, which resets tracked files and removes untracked, non-ignored files inside the clone only. Because a build can write to `.git/`, every sync (inside the job) first refuses a `.git` that is a symlink, a gitfile or has a `commondir`, then rewrites `.git/config` to a canonical one (written with `O_EXCL\|O_NOFOLLOW`), which drops planted filters, `insteadOf` rewrites, `fsmonitor` and includes. All git calls run with `core.hooksPath=/dev/null`, `core.fsmonitor=false`, `GIT_CONFIG_GLOBAL=/dev/null` and `GIT_CONFIG_NOSYSTEM=1`. |
| Build | `cargo build -p warp --bin warp-oss [--release] [--features ...]` | Fixed command. Profile, bin and feature names are allowlisted or validated against `app/Cargo.toml`. `CARGO_TARGET_DIR` is forced into the clone. |
| Tests | `cargo run -p integration --bin integration -- <name>` | Test name must be registered in `crates/integration/src/bin/integration.rs`. |
| Artifacts | `gh run download <run_id> --repo MILKA-TOP/warpium`, mount the .dmg read-only, copy the .app to `~/warpium-mac-test/apps/` | Numeric run id. The run must be a successful run of `.github/workflows/warpium-build.yml` whose head repository is `MILKA-TOP/warpium`, so pull requests from other forks are refused. The artifact is chosen from the run's artifact listing. Never writes to `/Applications`. Clears quarantine only on that private copy. Disabled by `--no-build`. |
| App | Launch `target/<profile>/warp-oss` from the clone, or the installed test app; quit (SIGTERM/SIGKILL) | Env limited to exactly `RUST_LOG`, `RUST_BACKTRACE`, `RUST_LIB_BACKTRACE`. Args limited to `--flag[=value]`, with no positionals and no `--warpctrl`. Only processes whose executable is inside the workspace are ever targeted or killed. Your daily Warpium in `/Applications` is never touched. |
| Logs | Read `~/Library/Logs/warp-oss.log`, the launched app's stdout capture, job logs | Path jail (see below). `warp-oss.log` is shared by every oss-channel app, so it is only returned while the app this server launched is still running, no other Warpium/WarpOss process is running, and the file was created within 2 minutes of that launch (each app rotates the log at startup). `file.open` can't open it, because paths handed to the app are jailed to the workspace only. |
| Screen | Capture **one Warpium window** at a time (`screencapture -l <window id>`) | Window must belong to a target PID. There is no full-screen capture. |
| Input | Mouse click/scroll, key combos and text, only into Warpium | See "Input guard". Can be disabled with `--no-input`. |
| warpctrl | Warp's local-control actions (tabs, panes, sessions, settings, themes, surfaces, input insert, file open) | Only names from the action catalog. Parameters are type-checked. `file.open` must use an absolute path inside the workspace. `input.*` is disabled under `--no-input`. Talks only to the Warpium instance under test: before an instance is used, an authenticated `instance.inspect` must report the selected instance id and pid (checked once per instance), so a stale or forged discovery record can't redirect actions to another Warp. |

Builds, tests, `gh` and the launched app get a **scrubbed environment**: `PATH`, `HOME`, `USER`,
`SHELL`, `TMPDIR`, `TERM`, `LANG`/`LC_*`, Cargo/rustup/Xcode variables and proxy settings only.
TLS trust variables (`SSL_CERT_FILE`, ...) are kept too. API keys, cloud credentials and tokens
in the server's environment are not passed on. `GH_TOKEN`/`GITHUB_TOKEN` go only to the `gh`
calls of `install_artifact`. Code
compiled from the fork can still read your files as you; this only keeps secrets in env vars
out of reach. `app_launch(env=...)` accepts only the three `RUST_*` logging variables. A
`WARP_*` prefix rule would not be safe: the app reads variables that name an executable to
spawn (`WARP_SHELL_PATH`), redirect its data (`WARP_DATA_PROFILE`) or carry secrets.

## What it cannot do

- Run arbitrary shell commands through a tool. Every subprocess uses a fixed program and an
  argument list, never `shell=True`.
- Read or write files outside `~/warpium-mac-test/` and the Warpium log file. Paths go through
  `realpath`, then a containment check, so `..` segments and symlinks pointing outside are rejected.
- Capture other apps' windows or the full screen.
- Send input to any app other than the Warpium process it launched, or one running from the
  workspace. Process executables are read from the kernel (`proc_pidpath`), not from `ps`/argv[0],
  which any process can spoof.
- Bind to `0.0.0.0` or `::`. Wildcard binds are refused outright.
- Touch `/Applications` or your other repos, or kill, screenshot or type into your daily
  Warpium. Data isolation for **release builds and artifacts** is limited; see "Test-app
  isolation".

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
  Warpium. This is re-checked before every mouse event, including each down/up of a double
  click. A notification banner, menu bar, Dock, another app's floating window or a transparent
  overlay covering the point causes a refusal. Transparent windows count because they can still
  receive clicks. If a helper app keeps an invisible full-screen overlay, clicks are refused, and
  the error names the owning app so you can quit it. Coordinates outside the target window are
  rejected.
- Keyboard events are posted with `CGEventPostToPid(<warpium pid>)` by default. Even if focus
  changes between the check and the event, the keystroke can't land in another app. If Warpium
  ignores pid-targeted events on your macOS version, `--key-delivery hid` posts to the global
  HID tap instead, still behind the frontmost check. **`hid` is weaker:** Spotlight, Alfred,
  Raycast and Dock/menu-bar keyboard focus take keystrokes without changing the frontmost app,
  so if one of them is open, hid-delivered text reaches it. Prefer `pid`.
- System shortcuts are refused by rules that still match when extra modifiers are added:
  launchers (`cmd/alt/ctrl+space`), app switching (`cmd+tab`, `cmd+alt/ctrl+backtick`),
  log out and lock (`cmd+shift/ctrl/alt+q`), force quit (`cmd+alt+esc`), screenshots
  (`cmd+shift+3..6`), function keys with modifiers (keyboard-navigation focus moves), Mission
  Control and Spaces (`ctrl+arrows`), and Dock hiding (`cmd+alt+d`). The `fn`/globe modifier
  is not accepted at all. Bare `f11`/`f12` (Show Desktop / Notification Center) and `ctrl+digit`
  (switch Desktop) are refused too.

## Test-app isolation

- The server always sets `WARP_DATA_PROFILE=mcp-test`, and the client cannot change or unset
  it. Debug builds honor it, which gives them separate UserDefaults, Application Support,
  keychain items and a separate `~/.warp-oss-mcp-test` config dir
  (`crates/warp_core/src/{paths.rs,channel/state.rs}`).
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
