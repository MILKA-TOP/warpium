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

```bash
# 1. Bump FORK_COUNTER in WARPIUM_VERSION (e.g. 1 -> 2) and merge that to master.
# 2. Tag the merged commit with the derived version and push the tag:
git tag "$(script/warpium/version)"
git push origin "$(script/warpium/version)"
```

The workflow publishes a GitHub Release with `Warpium-<version>-arm64.dmg` attached.

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
  writing: pushes to `master`, pull requests into `master`, Warpium tags, and manual runs;
  changes to `**/*.md` and `specs/**` alone don't trigger a build. Stacked pull requests into
  `claude/**` branches don't build unless the workflow lists them (PR #5 proposes adding them).
- **Jobs:** a cheap `ubuntu-latest` preflight computes the version and the artifact name, then
  the macOS job builds on `macos-15-intel` and cross-compiles arm64 with `--selfsign`
  (about 1.5 h). It checks the plist version and the app icon in the built bundle.
- **Cross-compile caveat:** the Intel runner can't run the arm64 binary that generates
  `settings_schema.json`, so CI builds lack the bundled `modify-settings` agent skill. Local
  builds on Apple silicon include it.
- **Artifacts:** `Warpium-<version>-<sha8>-macos-arm64` for branch and PR builds,
  `Warpium-<version>-macos-arm64` for tags; each contains `Warpium-<version>-arm64.dmg`.
- **Releases:** only tags with the extra fork component (`v*.*.*.*.*.*.*_*.*`) trigger one,
  so mirrored upstream tags can't publish a release. Preflight fails if the tag doesn't match
  `WARPIUM_VERSION`.

## Local release build

On an Apple silicon Mac with the tools from `./script/bootstrap`:

```bash
brew install create-dmg
./script/install_cargo_bundle
./script/install_cargo_release_deps --no-build-deps
export WARPIUM_VERSION_TAG="$(script/warpium/version)"
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
