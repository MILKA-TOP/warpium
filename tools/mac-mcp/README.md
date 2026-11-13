# warpium-mac-mcp

An MCP server you run **on your Mac**. Claude Code inside a Docker container connects to it
over HTTP (`host.docker.internal`) and uses it to build, launch, observe, drive and test
Warpium on the real machine. That way the agent can reproduce UI bugs and verify fixes it
can't see from Linux.

This is a fork-only tool. It doesn't touch the Rust code. **Read [SECURITY.md](SECURITY.md)
first**: this server can build and run code from the fork, and it can type into a terminal.

There are two ways to run it:

- **In a macOS VM (recommended).** The server and Warpium run in a throwaway
  [Tart](https://tart.run) ([openai/tart](https://github.com/openai/tart)) VM on your Mac.
  Clicks, keystrokes and app windows stay on the VM's own screen, so you can keep working while
  the agent tests, and the VM can't see your files, keychain or GitHub login. See [VM setup](#vm-setup-recommended).
- **Directly on your Mac (fallback).** Simpler, but the agent drives your real desktop: it
  takes focus and moves the cursor while it works. See [Host mode](#host-mode-fallback-quickstart).

## VM setup (recommended)

All scripts are in `vm/`, run **on the Mac host** from `tools/mac-mcp`, are idempotent and
print what they do. They need an Apple Silicon Mac, uv (`brew install uv`) and Tart, which
`setup.sh` installs with Homebrew if `tart` isn't already on PATH.

**Status: written and syntax/shellcheck-tested, but not yet run on a real Mac.** Report the
output of the failing step if something breaks.

### Cost

| | Default | Override |
|---|---|---|
| vCPU / RAM | 4 / 8 GiB, only while the VM runs | `WARPIUM_VM_CPU`, `WARPIUM_VM_MEMORY_MB` |
| Download | ~24 GB once (`macos-tahoe-vanilla`, cached in `~/.tart/cache`) | `WARPIUM_VM_IMAGE` |
| Disk | 50 GB sparse image using ~25-30 GB at first; plan for ~60 GB free | `WARPIUM_VM_DISK_GB` (grow only) |
| Golden copy / resets | a few GB each: APFS clones share unchanged blocks | |

The image is the cirruslabs *vanilla* macOS (`ghcr.io/cirruslabs/macos-*-vanilla`, still the
images the openai/tart docs use; macOS 26 Tahoe on a macOS 26+ host, Sequoia on macOS 15):
auto-login, SSH and passwordless sudo, nothing else installed. The Xcode images (~60+ GB)
aren't needed because the VM runs CI `.dmg` artifacts, not local builds.

### Steps

```sh
cd ~/src/warpium-mcp/tools/mac-mcp
vm/setup.sh               # Tart, image, VM "warpium-test", SSH key, hardening, provisioning
vm/grant-permissions.sh   # one-time: opens the VM's screen; enable Terminal in 2 settings panes
vm/start.sh               # headless VM + server + SSH forward on host 127.0.0.1:17654
vm/snapshot.sh            # save the provisioned VM as "warpium-test-golden"
vm/start.sh               # (snapshot stops the VM)
vm/gh-token.sh            # optional, for install_artifact: fine-grained read-only token
vm/print-client-config.sh # prints `claude mcp add ...` for the container (contains the token)
```

What each step does:

1. **`setup.sh`** prints which Homebrew it found (prefix and architecture; an Intel/Rosetta
   Homebrew in `/usr/local` on Apple Silicon gets a warning and is called via `arch -x86_64`),
   installs Tart unless `tart` is already on PATH (`brew install openai/tools/tart`; Tart moved
   there from `cirruslabs/cli`, and on macOS 15 hosts it skips the macOS 26-only `softnet`
   dependency), checks that `tart --version` runs, clones the image into the VM
   `warpium-test` and sets CPU/RAM. It creates a dedicated key
   `~/.config/warpium-mac-mcp/vm_ed25519` and installs it in the guest using the image's
   default `admin`/`admin` password (once). Then it hardens the guest's network logins: SSH
   becomes key-only for `admin` (no password, keyboard-interactive or root logins; checked
   with `sshd -T`), Screen Sharing and Remote Management are turned off, and it checks from
   the host that port 5900 is closed and that SSH refuses the password. The `admin`/`admin`
   password itself stays, because auto-login and the guest's login keychain depend on it; it
   then only works at the VM's own screen (see "Network modes" for the trade-off and
   `WARPIUM_VM_CHANGE_PASSWORD=1`). Finally it runs `provision.sh`. Every step is re-checked
   on each run, so after a failure just run `vm/setup.sh` again.
2. **`provision.sh`** installs Homebrew, uv, git and gh in the guest and copies this checkout
   of `tools/mac-mcp` into `~/warpium-mac-mcp` with rsync over SSH (nothing is cloned or
   logged in inside the guest). It installs a launchd agent that opens the server in
   **Terminal.app** in the guest's auto-login GUI session, so the server has a window server
   session and the permissions belong to Terminal. **Re-run it after every `git pull`** to
   update the guest's server.
3. **`grant-permissions.sh`**: macOS doesn't let a script grant Screen Recording or
   Accessibility while SIP is on, so this is one manual step. The script stops the headless
   VM and opens its screen in a window. In the VM, enable **Terminal** under System Settings >
   Privacy & Security > **Screen & System Audio Recording** and > **Accessibility** (the
   password is `admin` if macOS asks), then shut the VM down. Do this before
   `snapshot.sh` so every reset keeps the grants. (The alternative, disabling SIP in the guest
   and writing its TCC database, also needs a manual Recovery boot and breaks with TCC schema
   changes, so it isn't used.)
4. **`start.sh`** runs `tart run --no-graphics warpium-test` in the background (log:
   `~/.config/warpium-mac-mcp/vm/warpium-test.log`), waits for `tart ip` and SSH, checks the
   guest server answers, and starts
   `ssh -N -L 127.0.0.1:17654:127.0.0.1:17654 admin@<vm-ip>`. The container keeps using
   `http://host.docker.internal:17654/mcp` unchanged. It refuses if something else (e.g. the
   host-mode server) already listens on 17654.
5. **`gh-token.sh`** logs gh in inside the VM for `install_artifact`, with a **fine-grained
   token** you create for it: resource owner MILKA-TOP, only the `MILKA-TOP/warpium`
   repository, permission *Actions: Read-only*, short expiry. Your own gh login never enters
   the VM.
6. **`print-client-config.sh`** fetches the guest's bearer token over SSH and prints the
   `claude mcp add` command. The token differs from the host-mode one, so remove the old entry
   first (`claude mcp remove warpium-mac --scope user`), or keep the existing entry by
   provisioning with the host's token: `WARPIUM_VM_REUSE_HOST_TOKEN=1 vm/provision.sh`.

### Daily use

```sh
vm/start.sh      # before a session (idempotent; re-run if the forward died)
vm/stop.sh       # shuts the VM down and closes the forward (--forward-only keeps the VM)
vm/reset.sh --yes  # throw the VM away and recreate it from warpium-test-golden
```

`reset.sh` gives every test session a clean macOS with the server and permissions already in
place. After `git pull`, run `vm/provision.sh` (with the VM started) and, if you like,
`vm/snapshot.sh --force` to refresh the golden copy.

### Troubleshooting

- **Logs:** VM console `~/.config/warpium-mac-mcp/vm/warpium-test.log`, forward
  `~/.config/warpium-mac-mcp/vm/forward.log`, guest server
  `~/.config/warpium-mac-mcp/server.log` inside the VM.
- **Shell in the guest:**
  `ssh -i ~/.config/warpium-mac-mcp/vm_ed25519 -o UserKnownHostsFile=~/.config/warpium-mac-mcp/vm_known_hosts -o HostKeyAlias=warpium-test admin@$(tart ip warpium-test)`
- **See the VM's screen:** `vm/stop.sh && tart run warpium-test` (opens a window; add
  `--net-bridged=<if>` in bridged mode). Screen Sharing (VNC) is turned off in the guest.
- **`doctor` says a permission is not granted:** the permissions shown come from the running
  server (it records them at start and on every `doctor` call). Re-run
  `vm/grant-permissions.sh`; if Accessibility stays off, run
  `tccutil reset Accessibility com.apple.Terminal` in the guest and enable it again.
- **"Hardening guest logins" failed** (for example `eDSAuthFailed` from an older
  `setup.sh` that tried to change the password): re-run `vm/setup.sh`. It continues where it
  stopped, and removes a `vm_admin_password` file whose password the guest never took.
- **Host key changed** after recreating the VM from a new image: `setup.sh` forgets the old key
  automatically; otherwise
  `ssh-keygen -R warpium-test -f ~/.config/warpium-mac-mcp/vm_known_hosts`.
- **"host port 17654 is taken":** stop the host-mode server (Ctrl+C) or a stale forward
  (`vm/stop.sh --forward-only`).
- **Start over completely:** `vm/stop.sh; tart delete warpium-test; vm/setup.sh`.
- **VM not reachable / VPN route conflict:** `setup.sh` and `start.sh` check, right after
  `tart ip`, that `route -n get <vm-ip>` goes through a vmnet `bridge*` interface (or the
  bridged interface in bridged mode). If it doesn't, they stop with the route that wins and
  the conflicting `netstat -rn -f inet` lines. The usual cause is a corporate VPN or a LAN
  that also uses `192.168.64.0/24`: an unscoped route like `192.168.64  192.168.0.1  UGSc  en0`
  beats the host's interface-scoped entry on `bridge100`, so pings and SSH to the guest time
  out. There are two fixes:
  1. **Bridged networking (recommended):**
     `vm/stop.sh && WARPIUM_VM_NET=bridged vm/setup.sh`. The VM gets an address from your
     LAN router, outside the VPN route. See [Network modes](#network-modes) for what that
     exposes.
  2. **Move the vmnet subnet:** `vm/fix-subnet.sh [192.168.251.1]` stops `warpium-test`.
     It refuses to run while other Tart VMs are running unless you pass `--force`. It then
     checks that the new /24 has no route and, after you confirm (`--yes` skips the
     question), runs
     `sudo defaults write /Library/Preferences/SystemConfiguration/com.apple.vmnet Shared_Net_Address -string 192.168.251.1`
     (and `Shared_Net_Mask 255.255.255.0`). Finally it restarts the VM and checks that the
     VM got an address in the new subnet. The `defaults` domain must not end in `.plist`:
     `defaults` adds the extension, so `.../com.apple.vmnet.plist` would write
     `com.apple.vmnet.plist.plist`, which vmnet ignores. The script removes such a stray
     file. To check the real file, run
     `plutil -p /Library/Preferences/SystemConfiguration/com.apple.vmnet.plist`. On
     macOS 26 the VM has been seen keeping a `192.168.64.x` address even with a correct
     plist. That is either a stale `/var/db/dhcpd_leases` entry, which the script detects
     when `bridge100` moved but `tart ip` did not, or macOS not honouring the setting. In
     the second case, use bridged mode.
  The default suggested subnet comes from `WARPIUM_VM_SUGGEST_SUBNET`.
  `WARPIUM_VM_SKIP_ROUTE_CHECK=1` disables the check. If the route is fine but sshd still
  doesn't answer, the guest is still booting or stuck. Check
  `~/.config/warpium-mac-mcp/vm/warpium-test.log`, then `vm/stop.sh && tart run warpium-test`
  to see its screen. The vanilla image has no Tart guest agent, so `tart exec` doesn't
  work.

### Network modes

`WARPIUM_VM_NET` selects how the VM is networked. The value you pass to `setup.sh` or
`start.sh` is saved in `~/.config/warpium-mac-mcp/vm/net_mode` (and `WARPIUM_VM_BRIDGE_IF`
in `.../vm/bridge_if`) once the VM runs with it. Later runs of `start.sh`, `provision.sh`
and the other scripts reuse the saved values. A running VM keeps its mode until
`vm/stop.sh`.

| Mode | `tart run` flag | Address | Notes |
|---|---|---|---|
| `nat` (default) | none | `192.168.64.x` from vmnet's DHCP | Only the host can reach the guest. Breaks when a VPN/LAN route covers the vmnet subnet. |
| `bridged` | `--net-bridged=<if>` | from your LAN router | `tart ip --resolver=arp` (ARP table) finds the address. The interface comes from `route -n get default`; when a VPN `utun` owns the default route, the script uses the first `en*` interface that has a default route instead. Override it with `WARPIUM_VM_BRIDGE_IF` (`tart run --net-bridged=list`). |
| `softnet` | `--net-softnet` | `192.168.64.x`, as `nat` | Tart's Softnet adds isolation on top of the same vmnet NAT, so it does **not** avoid a 192.168.64.0/24 route conflict. Tart's headless start can't ask for sudo, so the `softnet` binary must be setuid root; the scripts print the `chown`/`chmod` commands. |

**Security of bridged mode.** The guest is a separate machine on your LAN, reachable by
anyone on that network:

- **SSH:** `setup.sh` installs the dedicated key and makes SSH key-only:
  `PasswordAuthentication no`, `KbdInteractiveAuthentication no`, `PermitRootLogin no`,
  `AllowUsers admin`, checked with `sshd -T` in the guest and with a password login attempt
  from the host, which must be refused.
- **Screen Sharing:** the image has Screen Sharing (VNC, port 5900) on, accepting
  `admin/admin`. `setup.sh` disables it (`launchctl disable` + `bootout` of
  `com.apple.screensharing`, `kickstart -deactivate -stop` for Remote Management) and checks
  from the host that ports 5900 and 3283 are closed while SSH still answers. `tart run` windows (`grant-permissions.sh`) use
  Tart's own display, not Screen Sharing, and keep working.
- **The admin password stays `admin/admin`.** Changing it breaks auto-login unless
  `/etc/kcpassword` is rewritten, leaves the login keychain on the old password (keychain
  dialogs that block UI tests) and touches the SecureToken. With SSH key-only and Screen
  Sharing off, no network service takes the password, so it only works at the VM's own
  screen and for `sudo`, which is passwordless for `admin` anyway. The trade-off: if a
  network login is turned back on inside the guest (for example Screen Sharing in System
  Settings, or by code the agent runs as `admin`, which has root through sudo), it accepts
  a well-known password. `WARPIUM_VM_CHANGE_PASSWORD=1 vm/setup.sh` also replaces it with
  a random one, updating `/etc/kcpassword` and the login keychain; the password is saved to
  `~/.config/warpium-mac-mcp/vm_admin_password` (mode 600) only after the guest uses it.
- **MCP server:** it binds the guest's `127.0.0.1` only (`serve --host 127.0.0.1`; the
  server refuses wildcard binds). It is reached only through the host's SSH forward, which
  listens on the host's `127.0.0.1`. It also requires the bearer token.
- **Exposure window:** on a fresh image, the guest is on the LAN with `admin/admin` for SSH
  and Screen Sharing from boot until `setup.sh` has made SSH key-only and turned Screen
  Sharing off, usually a minute or two. `setup.sh` warns and asks before it boots a VM
  bridged that it hasn't hardened yet. `WARPIUM_VM_BRIDGED_FRESH_OK=1` skips the question.
  Run the first setup on a network you trust, or in `nat` mode where it works.
- **Finding the address:** Tart reads the VM's address from the host's ARP table. Tart's
  docs say this normally works for macOS guests. If `setup.sh` stops with "no IP", check
  the router's DHCP client list or `arp -a`.
- Use bridged mode only on networks where that exposure is acceptable. Some networks
  (802.1X, Wi-Fi with client isolation, MAC limits) don't give the VM an address at all.

## Host mode (fallback): quickstart

The rest of this section runs the server directly on your Mac. Don't run it inside the
Warpium you are testing (quitting or relaunching that app would kill the server); use
Terminal.app or another terminal.

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
(no building, tests or artifact downloads; only an app you place in
`~/warpium-mac-test/apps/` can be launched), `--key-delivery hid` (weaker, see SECURITY.md).

### 4. Grant macOS permissions to the terminal app that runs the server

macOS gives these permissions to the **app that runs the server** (e.g. Terminal.app, Warp,
iTerm2), not to the server itself:

- System Settings > Privacy & Security > **Screen & System Audio Recording**: enable that app.
- System Settings > Privacy & Security > **Accessibility**: enable that app.
- Quit and reopen the terminal, then start the server again.

If Accessibility still shows "not granted" in `doctor` after you enabled it, reset the entry
and add the app again: `tccutil reset Accessibility <bundle id>` (e.g. `com.apple.Terminal`,
`dev.warp.Warp-Stable`, `com.googlecode.iterm2`), then re-enable it and restart the terminal.

Don't run the server inside the Warpium being tested: relaunching or quitting the test app
would kill the server, and the permission would have to go to the test app.

### 5. Check prerequisites

```sh
uv run warpium-mac-mcp doctor                  # --no-port-check while the server runs
```

`doctor` separates two things. **Required** checks (`FAIL` lines, `all_required_ok`) are what
installing artifacts, screenshots and input need. The **local-build toolchain** (rustup,
cargo, protoc, Xcode/Metal; `bld` lines, `build_ready`) is only needed for `build` and
`run_integration_test`, so a missing toolchain doesn't fail doctor when you use CI artifacts.
The first `protoc`/`xcrun` call after boot can be slow; a check that takes more than 60 s
(20 s in the MCP `doctor` tool, to keep one call bounded) is reported as "timed out, run
doctor again" rather than missing.

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
of git (`tools/mac-mcp/.gitignore` ignores `.mcp.json` there).

**The output contains the bearer token.** Never commit it, and never paste it into a shared
chat, issue or PR (that includes a chat with an agent whose transcript others can read). To
rotate the token: delete `~/.config/warpium-mac-mcp/token` (or run
`uv run warpium-mac-mcp rotate-token`), restart the server, then in the container
`claude mcp remove warpium-mac --scope user` and add it again with the new command.

**Proxy in the container.** Claude Code in the container may send HTTP through a proxy
(`HTTP_PROXY`/`HTTPS_PROXY` set, e.g. squid). The proxy can't reach `host.docker.internal`,
so the MCP connection fails with **`HTTP 503`** (from the proxy, not from this server).
Exporting `NO_PROXY` in your shell is not enough, because the `claude` process doesn't get it.
Set it in Claude Code's own settings, `~/.claude/settings.json` in the container:

```json
{
  "env": {
    "NO_PROXY": "host.docker.internal,localhost,127.0.0.1",
    "no_proxy": "host.docker.internal,localhost,127.0.0.1"
  }
}
```

(merge with an existing `env` block and keep any hosts already listed there), or set both
variables under `environment:` in your docker-compose service. Restart Claude Code, then check
that `claude mcp list` shows `warpium-mac: ... ✔ Connected`.

Connectivity check from the container. `401` means you reached the server and auth works:

```sh
curl -s --noproxy host.docker.internal -o /dev/null -w '%{http_code}\n' -X POST http://host.docker.internal:17654/mcp
```

### 7. Smoke test (ask the agent, or call the tools yourself)

The same smoke test works in VM mode; there, steps 2-3 need a local toolchain the VM doesn't
have, so use `install_artifact(run_id)` and `app_launch(source="artifact")` instead.

1. `doctor`: `all_required_ok` is true and both permissions are granted (`build_ready` only
   matters for steps 2-3).
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
9. `app_logs(tail=50)` shows `~/Library/Logs/warp-oss.log`. It is refused while another
   Warpium is running, because they share that file.
10. `app_quit()`.
11. Check `~/.config/warpium-mac-mcp/audit.log` for those calls.

## Tools

| Tool | What it does |
|---|---|
| `doctor` | Checks toolchain, gh, Xcode/Metal, protoc, permissions, workspace, log and discovery paths. |
| `repo_sync(branch, discard_local_changes=false)` | Fetches the fork and checks out `branch` at `origin/branch` in the workspace clone (job). Refuses a dirty clone unless told to discard local changes. |
| `build(profile, bin, features)` | `cargo build -p warp --bin warp-oss` (job). Default features are `gui,warp_control_cli`. |
| `job_status(job_id?)`, `job_logs(job_id, tail, grep)`, `job_cancel(job_id)` | Inspect and cancel background jobs. |
| `install_artifact(run_id, artifact_name?)` | `gh run download` of the Warpium .dmg from a successful fork `warpium-build.yml` run, e.g. run `43262238821` gives `Warpium-a70e8571-macos-arm64`. The artifact is picked from the run's listing. Mounts the .dmg read-only and copies the .app into `~/warpium-mac-test/apps/` (job). |
| `run_integration_test(name, real_display)` | `cargo run -p integration --bin integration -- <name>` (job). |
| `app_launch(source, env, args, profile)` | Runs the dev binary (`build`) or the installed artifact (`artifact`). env accepts `RUST_LOG`/`RUST_BACKTRACE`/`RUST_LIB_BACKTRACE` only. Always sets `WARP_DATA_PROFILE=mcp-test`. |
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

In VM mode the server binds the guest's `127.0.0.1`, and `vm/start.sh` forwards the host's
`127.0.0.1:17654` to it over SSH, so everything below applies to the forward unchanged.

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
(local-control client), `server.py` (MCP tools), `doctor.py`, `cli.py`, `vmtools.py`
(helpers for the VM scripts). `vm/` holds the Tart scripts; the tests run `bash -n` and
shellcheck (installed as a dev dependency) on them.
