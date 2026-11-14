# Warpium developer guide

Fork-specific notes for working on Warpium, the `MILKA-TOP/warpium` fork of
[warpdotdev/warp](https://github.com/warpdotdev/warp). Upstream's engineering guide is
[AGENTS.md](AGENTS.md); this file covers only what differs in the fork. Both are loaded for
agents through the fork-only [CLAUDE.md](CLAUDE.md). The user-facing [README](README.md)
should stay free of this material.

## Fork conventions

- Pull requests always target `MILKA-TOP/warpium` `master`, or a stacked `claude/*` branch.
  Never open them against `warpdotdev/warp`: pass `--repo MILKA-TOP/warpium` to `gh`.
- Never push to `master` directly. Work on a branch (`claude/<topic>` for agents).
- All development goes through agents that follow the repo skills (`gui-ui-guidelines`,
  `rust-unit-tests`, `add-feature-flag`, `logging-and-error-reporting`, ...), the validation
  order in AGENTS.md, and `/code-review` before a PR is opened or updated.
- Keep fork changes small and local so upstream merges stay clean. Prefer new fork-only
  files over edits to upstream ones (that is why this guide is not part of AGENTS.md).
- The `warp` app crate needs `protoc`, which the dev container doesn't have, so it can't be
  compiled there. Other crates can be checked locally; CI is the compile check for the app.
- To build, run and screenshot the app on the owner's real Mac, use the `warpium-mac` MCP
  server when it is connected (its tools are `mcp__warpium-mac__*`). It runs Warpium inside an
  isolated Tart macOS VM so testing never touches the owner's desktop. Its source is
  `tools/mac-mcp/` (on the `claude/mac-mcp` branch until that is merged).

## What is still shared with upstream's OSS build

Warpium is Warp's `oss` channel with its own bundle name (`Warpium`) and bundle id
(`dev.milka.Warpium`). These stay as upstream has them:

- the `~/.warp-oss` directory (settings, themes, skills)
- the `warposs://` URL scheme, so sign-in redirects can reach an upstream `WarpOss.app` if
  both are installed
- the `warp-oss` binary and CLI name
- Linux and Windows packaging: inherited, not built or maintained for Warpium

User-visible names come from `Channel::app_display_name()` (`"Warpium"` on Oss).

## Importing configuration from Warp

Warpium imports and keeps syncing the official Warp app's configuration (settings, themes,
workflows, keybindings, skills, …) one way, on launch and from the command palette. Fork-only code
lives in `crates/warp_config_import` (protoc-free, unit-tested with
`cargo test -p warp_config_import`) and `app/src/warpium/`. The category table and sync rules are
in [docs/warpium/warp-config-import.md](docs/warpium/warp-config-import.md).

## Upstream Warp update notification

Warpium tells you when upstream `warpdotdev/warp` `master` has commits the running build lacks, so
you can sync the fork. Fork-only code lives in `crates/warpium_upstream_check` (protoc-free,
unit-tested with `cargo test -p warpium_upstream_check`) and `app/src/warpium/upstream_check.rs`.

- **Base commit.** Each build embeds `WARPIUM_UPSTREAM_BASE`, the merge base of the build commit
  with upstream `master` (the newest upstream commit it contains). The preflight job of Warpium
  Build computes it with [`script/warpium/upstream_base`](script/warpium/upstream_base) through the
  GitHub compare API. Builds without it (a failed lookup, or a local build that didn't export it)
  never check. Commits are compared rather than tags because upstream stopped publishing public
  release tags after June 2026.
- **Check.** Every `warpium.upstream_check_interval_hours` (default 6, 1–168), counted across
  launches, and at launch when the build's base has no saved result; unauthenticated:
  `GET /repos/warpdotdev/warp/branches/master` revalidated with its `ETag`, then, only when the
  head moved, `GET /repos/warpdotdev/warp/compare/<base>...<head>` (page 2, which
  leaves out the changed-files list) for the commit count. Errors back off from 15 minutes,
  doubling up to the interval; GitHub's rate-limit headers are honored. State, including the
  last result (so the banner survives a relaunch) and the dismissed head, is kept in private
  preferences under `WarpiumUpstreamCheckState`.
- **Notification.** A workspace banner, "Upstream Warp has N new commits since this build", with
  **View changes** (the GitHub compare page from the base), **Sync fork** (opens the fork's GitHub
  page, which has GitHub's **Sync fork** button) and a close button that hides it until upstream
  moves again. It appears once upstream is `warpium.upstream_check_min_commits` (default 1)
  commits ahead.
- **Command palette:** "Check for upstream Warp updates" checks now and reports the result in a
  toast even when nothing is new; "Enable/Disable upstream Warp update notifications" toggles
  `warpium.upstream_check` (default on). OSS channel only.
- **Syncing from the CLI:** `gh repo sync MILKA-TOP/warpium` does what the **Sync fork** button
  does: it updates the fork's `master` on GitHub from upstream (a fast-forward, or a merge commit
  when the fork has its own commits and there are no conflicts). Both write to `master` directly,
  so they are for the owner, not agents. On conflicts, merge `warpdotdev/warp` `master` in a branch
  and a pull request instead. Either way, then follow [After merging upstream
  Warp](#after-merging-upstream-warp).
- **Trying it:** run a build with an older upstream commit as its base.
  `WARPIUM_UPSTREAM_BASE_OVERRIDE` set at runtime overrides the embedded one; a base with no saved
  result is checked at launch:

  ```bash
  WARPIUM_UPSTREAM_BASE_OVERRIDE=<older upstream sha> /Applications/Warpium.app/Contents/MacOS/warp-oss
  ```

### Upstream issue (GitHub Actions)

`.github/workflows/warpium-upstream-watch.yml` ("Warpium Upstream Watch") runs daily and on demand.
While the fork's `master` lacks upstream commits it opens, or updates, a single issue titled
"Upstream Warp has new commits" with the count and the compare link, and closes it once the fork is
in sync. It needs issues enabled on the repository (otherwise it only writes the count to the run
summary). Opt out by setting the repository variable `WARPIUM_UPSTREAM_ISSUE` to `false`.

## Versioning

A Warpium version is the upstream Warp release it is based on plus a fork counter as one
extra dot component, so upstream `a.b.c` becomes Warpium `a.b.c.d`:

| | Upstream Warp | Warpium |
| --- | --- | --- |
| Release tag, About page | `v0.2026.06.03.09.49.stable_00` | `v0.2026.06.03.09.49.stable_00.1` |
| `CFBundleShortVersionString` / `CFBundleVersion` | `0.2026.06.03.09.49.00` | `0.2026.06.03.09.49.00.1` |

- [`WARPIUM_VERSION`](WARPIUM_VERSION) is the single source of truth: `UPSTREAM_TAG` and
  `FORK_COUNTER`.
- [`script/warpium/version`](script/warpium/version) prints the derived tag (`--tag`, the
  default) or the plist version (`--plist`).
- CI exports the tag as `WARPIUM_VERSION_TAG` for `script/bundle`. It is baked into the app
  for the About page (`ChannelState::display_version()`) and turned into the bundle version
  by `script/update_plist`. Builds between releases show the current version; their artifact
  name adds the commit.

**Why not `GIT_RELEASE_TAG`:** `ChannelState::app_version()` gates official-release
behaviour. On the OSS channel a fork tag there would break the remote-server version
handshake (the server never reports a `….1` version, so the client deletes it and loops) and
would turn on the changelog and "Check for updates" UI. A separate variable keeps everything
else identical to the untagged OSS build.

### Releasing

Run **Actions → Warpium Release → Run workflow** on `master`, or:

```bash
# First release of the current UPSTREAM_TAG/FORK_COUNTER (no bump):
gh workflow run warpium-release.yml --repo MILKA-TOP/warpium --ref master -f bump=false
# Every later release: bump FORK_COUNTER (e.g. 1 -> 2) first.
gh workflow run warpium-release.yml --repo MILKA-TOP/warpium --ref master -f bump=true
```

`.github/workflows/warpium-release.yml` ("Warpium Release"):

1. With `bump=true`, increments `FORK_COUNTER` in `WARPIUM_VERSION` and pushes a
   `Release Warpium <version>` commit to `master` as `github-actions[bot]`. That works
   because `master` has no branch protection; the push fails if `master` moved after the run
   started. This one-line version bump is the only automated commit to `master`; agents still
   never push there. If `master` gets protected, or you want the bump reviewed, change
   `FORK_COUNTER` in a PR instead, merge it and run with `bump=false`.
2. Tags the commit with `script/warpium/version` and pushes the tag. If the tag already exists
   on the same commit (a retry), it is reused; on another commit the run fails and asks for
   `bump=true`.
3. Calls Warpium Build (`workflow_call` with `release_tag`), which builds that tag and runs
   its `release` job. A separate call is needed because tags pushed with `GITHUB_TOKEN`
   don't trigger other workflows; for the same reason the bump commit gets no `master` build.

The `release` job uploads `Warpium-<version>-arm64.dmg` and `Warpium-<version>-arm64.dmg.sha256`
and publishes the release (never a prerelease), titled `Warpium <version>`. It is marked
**latest** when it is the highest Warpium version, so re-running it for an older release, which
replaces that release's assets and notes, doesn't move "latest" back. The newest release is at
<https://github.com/MILKA-TOP/warpium/releases/latest>.

The notes come from [`script/warpium/release_notes.sh`](script/warpium/release_notes.sh): the
PRs merged into `master` since the previous release (checked with the compare API, so upstream
PR numbers never appear), the upstream tag, install steps, the checksum and known limitations.
Update its known-limitations list when the README's "Good to know" changes. Preview it locally
with an authenticated `gh`:

```bash
script/warpium/release_notes.sh --ref master            # notes for the next tag
script/warpium/release_notes.sh --tag v0.2026.06.03.09.49.stable_00.1  # an existing tag
```

Pushing a Warpium tag by hand (`git push origin "$(script/warpium/version)"`) still builds
and publishes the same release through the tag trigger of Warpium Build.

### After merging upstream Warp

Find the newest upstream release tag whose commit is already part of `master` (prefer a
`stable_` tag), set it as `UPSTREAM_TAG`, and reset `FORK_COUNTER` to `1`:

```bash
git fetch https://github.com/warpdotdev/warp 'refs/tags/v*:refs/tags/upstream/v*' --no-tags
git tag --merged master --list 'upstream/v*.stable_*' --sort=-version:refname | head -n 1
```

Upstream stopped publishing release tags to the public repository after June 2026, so the
upstream part of the version may be older than the upstream commit Warpium is built from.

## CI

`.github/workflows/warpium-build.yml` ("Warpium Build") is fork-only; upstream's workflows are
gated to `warpdotdev` and don't run here. Don't change whether workflows are enabled.

- **Triggers:** check the `on:` block of the workflow for the current list. At the time of
  writing: pushes to `master`, pull requests into `master` or into `claude/**` (stacked PRs),
  Warpium tags, manual runs, and calls from Warpium Release. Changes to `**/*.md`, `specs/**` and `tools/**` alone don't
  trigger a build, so a PR that only touches tooling such as `tools/mac-mcp` gets no macOS build.
- **Jobs:** a cheap `ubuntu-latest` preflight computes the version, the artifact name and the
  upstream base commit (`WARPIUM_UPSTREAM_BASE`, see
  [Upstream Warp update notification](#upstream-warp-update-notification)), then
  the macOS job builds on `macos-15-intel` and cross-compiles arm64 with `--selfsign`
  (about 1.5 h). It checks the plist version and the app icon in the built bundle.
- **Cross-compile caveat:** the Intel runner can't run the arm64 binary that generates
  `settings_schema.json`, so CI builds lack the bundled `modify-settings` agent skill. Local
  builds on Apple silicon include it.
- **Artifacts:** `Warpium-<version>-<sha8>-macos-arm64` for branch and PR builds,
  `Warpium-<version>-macos-arm64` for tags; each contains `Warpium-<version>-arm64.dmg`.
- **Releases:** the `release` job runs for a `workflow_call` from Warpium Release (see
  [Releasing](#releasing)) or a pushed tag with the extra fork component
  (`v*.*.*.*.*.*.*_*.*`), so mirrored upstream tags can't publish a release. Preflight fails if
  the tag doesn't match `WARPIUM_VERSION`.

## Local release build

On an Apple silicon Mac with the tools from `./script/bootstrap`:

```bash
brew install create-dmg
./script/install_cargo_bundle
./script/install_cargo_release_deps --no-build-deps
export WARPIUM_VERSION_TAG="$(script/warpium/version)"
# Optional: enables the upstream Warp update notification (needs the commit pushed to GitHub).
export WARPIUM_UPSTREAM_BASE="$(script/warpium/upstream_base)"
./script/bundle --channel oss --arch aarch64 --selfsign --dmg-name-suffix "$WARPIUM_VERSION_TAG-arm64"
```

The `.dmg` and `Warpium.app` are written to `target/release-lto/bundle/osx/`.

## Branding

The mark is two overlapping slanted parallelograms; the primary app icon is the white mark on
a violet-to-blue gradient. [`branding/README.md`](branding/README.md) has the palette, the
icon variants, the wordmark font (Manrope, OFL, committed as outlines) and the reference
images.

- `script/warpium/generate_svgs.py` writes every SVG from one parametric definition:
  `branding/*.svg`, the in-app logos and marks (`app/assets/bundled/svg/warpium-*.svg`) and the
  layers of `app/channels/oss/icon/AppIcon.icon`. It needs the Manrope TTF (`--font`).
- `script/warpium/render_icons.py` renders the PNG, `icon.ico` and `Warpium.icns` icons, the
  README banner and icon, and the `.dmg` background; with `--preview-screenshot` it also
  re-composites the README product preview.
- `AppIcon.icon` is the macOS 26 Liquid Glass icon, compiled by `script/compile_icon` with
  `actool` during bundling. `Warpium.icns` (referenced from `app/Cargo.toml`) is the fallback,
  and the `.dmg` volume icon.

```bash
uv run script/warpium/generate_svgs.py --font /path/to/Manrope.ttf
uv run script/warpium/render_icons.py
```
