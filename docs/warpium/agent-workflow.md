# How agents work on Warpium

Working rules for the coordinating agent session and the implementation agents it starts. They
complement the Fork conventions in [WARPIUM.md](../../WARPIUM.md) and AGENTS.md's validation order.
The plan itself is in [roadmap.md](roadmap.md).

## Roles

- **Coordinator** (the main session): picks the next roadmap item, starts implementation agents,
  reviews their PRs, verifies them in the test VM, merges, cuts releases, and keeps
  [roadmap.md](roadmap.md) up to date. The owner has authorized it to merge after green CI, review
  and a VM check where one is possible, and to drive the roadmap autonomously; the owner may still
  ask for a release or a merge explicitly.
- **Implementation agents** (one per feature, in their own git worktree): research, implement, test,
  open the PR and make CI green. Every agent prompt says:
  - **never merge a PR and never push to `master`**;
  - **don't spawn subagents or forks**;
  - run cargo as `CARGO_BUILD_JOBS=4 nice -n 19 cargo …`;
  - follow the repo skills that apply (`gui-ui-guidelines`, `rust-unit-tests`,
    `logging-and-error-reporting`, `add-feature-flag`, …) and self-review before the PR;
  - finish with a report: PR number, what was done, what was left out, and a manual test script for
    the VM.

## Resources in the dev container

The container shares the owner's Mac (`/workspace` is a bind mount of the Mac's disk, which is
nearly full) and freezes under load.

- At most two implementation agents at a time.
- Put large cargo target directories under `/tmp` (the scratchpad), not in the repository, and
  delete them afterwards.
- Remove an agent's worktree once its work is pushed or merged (check for uncommitted or unpushed
  changes first); keep an eye on `df -h /workspace`.
- The app crate (`warp`) needs `protoc`, which the container lacks: CI is its compile check. App unit
  tests are compiled only by "Warpium Tests", so watch trait imports and visibility in test modules.

## Pull requests

- Branch `claude/<topic>`, PR against `MILKA-TOP/warpium` `master` with `--repo MILKA-TOP/warpium`.
- One comprehensive PR per feature (see [roadmap.md](roadmap.md)).
- Before merging: CI green ("Warpium Build" and "Warpium Tests"), the diff reviewed, and the debug
  artifact checked in the test VM by the agent's test script.
- When a PR conflicts after another merge, merge `origin/master` into it. Conflicts in
  `app/Cargo.toml`, `Cargo.lock` and `settings_view/features_page.rs` usually mean both sides added an
  entry: keep both, and verify with `cargo metadata --locked` and `rustfmt --check`. For files where
  git interleaves added blocks, rebuild the file from `master` and reapply the PR's hunks instead of
  concatenating conflict sides.

## Test VM (`warpium-mac` MCP server)

See [tools/mac-mcp/README.md](../../tools/mac-mcp/README.md) and WARPIUM.md's Fork conventions.

- There is only one VM on the owner's Mac, `muhtar-test`, owned by lipt-mocker's `tools/mac-mcp`;
  the Warpium server runs in it next to the Muhtar server. Never create, clone or snapshot another VM
  (each costs about 50 GB).
- Test from CI artifacts (`install_artifact` with the PR's Warpium Build run id), not in-guest
  builds. A debug artifact is about 230 MB.
- Screenshots can lag a frame behind; take another before concluding something didn't happen. Call
  `focus_app` again when another window (System Settings, notifications) takes focus.
- The app log `~/Library/Logs/warp-oss.log` is replaced on every launch. To catch a crash, run
  `nohup tail -n0 -F ~/Library/Logs/warp-oss.log > /tmp/wlog &` in a Warpium terminal first,
  reproduce, relaunch and read `/tmp/wlog` (`grep -i panicked`).
- Fake `docker`/`ssh` scripts on `PATH` (`/usr/local/bin`, create it with `sudo mkdir -p`) let you
  test container features without Docker; make sure the fake keeps `-it` invocations running.
- When the server stops answering: the owner restarts it (`vm/start.sh`, `vm/provision.sh`; a stuck
  guest is rebooted with `guest_ssh <ip> 'sudo shutdown -r now'`), then the session reconnects with
  `/mcp`. A VPN route overlapping the vmnet subnet (`192.168.64.0/24`) cuts the VM off; see the
  scripts' Fix A / Fix B output.
