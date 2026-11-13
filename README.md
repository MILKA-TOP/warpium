<p align="center">
  <img width="880" alt="Warpium: a lightweight IDE for AI-driven mobile development, built on Warp" src="docs/images/warpium-banner.png" />
</p>

<p align="center">
  <b>A lightweight IDE for AI-driven mobile development, built on Warp.</b><br />
  <sub>Легковесная IDE для AI-мобильной разработки на базе Warp</sub>
</p>

<p align="center">
  <a href="https://github.com/MILKA-TOP/warpium/actions/workflows/warpium-build.yml"><img alt="Warpium Build" src="https://github.com/MILKA-TOP/warpium/actions/workflows/warpium-build.yml/badge.svg" /></a>
  <a href="https://github.com/MILKA-TOP/warpium/releases"><img alt="Releases" src="https://img.shields.io/github/v/release/MILKA-TOP/warpium?include_prereleases&label=release" /></a>
  <a href="LICENSE-AGPL"><img alt="License: AGPL v3 / MIT" src="https://img.shields.io/badge/license-AGPL--3.0%20%2F%20MIT-blue" /></a>
</p>

<p align="center">
  <img width="1024" alt="Warpium product preview: an agent conversation next to a code review diff" src="docs/images/warpium-preview.png" />
</p>

## About

Warpium is a lightweight IDE for AI-driven mobile development, built on [Warp](https://github.com/warpdotdev/warp), the open-source agentic development environment born out of the terminal. It builds Warp's open-source (`oss`) channel under its own name, icon, and bundle identifier, and adds features aimed at mobile (Android/Kotlin) development with AI agents on top.

All credit for Warp goes to its original authors at [warpdotdev/warp](https://github.com/warpdotdev/warp). Warpium tracks upstream and keeps its own changes small so that upstream updates merge cleanly.

> [!NOTE]
> Warpium is an unofficial, personal project. It is not affiliated with, endorsed by, or supported by Warp (Denver Technologies, Inc). Please do not report Warpium issues to the Warp team.

## Planned features

- Fix "open in new tab" so the new tab starts in the expected working directory.
- `.gitignore`-aware file search with include/exclude glob filters.
- A JetBrains-style git log view.
- Notifications when new upstream Warp changes are available.
- Pasting images into CLI agents that run inside Docker containers.
- Kotlin LSP support and IDE-like code navigation.

## Getting a build

<img align="right" width="96" alt="Warpium app icon" src="docs/images/warpium-icon.png" />

Builds are produced by the [Warpium Build](.github/workflows/warpium-build.yml) GitHub Actions workflow. Only Apple silicon (arm64) macOS builds are published for now.

- **Latest development build:** open the [Actions tab](https://github.com/MILKA-TOP/warpium/actions/workflows/warpium-build.yml), pick a successful run on `master`, and download the `Warpium-<version>-<commit>-macos-arm64` artifact (a zipped `.dmg`).
- **Releases:** pushing a Warpium version tag (see [Versioning](#versioning)) publishes a [GitHub Release](https://github.com/MILKA-TOP/warpium/releases) with `Warpium-<version>-arm64.dmg` attached.

CI builds are currently cross-compiled for arm64 on Intel runners, which can't run the arm64 binary to generate `settings_schema.json`. As a result, the bundled `modify-settings` agent skill is not available in CI builds; local builds on Apple silicon include it.

Warpium builds are ad-hoc signed and **not notarized** by Apple, so macOS will refuse to open them at first. After copying `Warpium.app` to `/Applications`, clear the quarantine flag:

```bash
xattr -dr com.apple.quarantine /Applications/Warpium.app
```

Warpium uses its own bundle identifier (`dev.milka.Warpium`), so it can be installed next to the official Warp app. On macOS its preferences, `~/Library/Application Support/dev.milka.Warpium` data, and keychain entries are separate from Warp's, while `~/.warp-oss` (for example `settings.toml`, themes, and skills) is still shared with upstream's OSS build. Warpium also keeps the OSS build's `warposs://` URL scheme and `warp-oss` CLI name, so avoid installing it next to an upstream `WarpOss.app`: sign-in redirects may open the wrong app.

Only macOS builds are produced and tested. Linux and Windows packaging scripts are inherited from upstream and are not maintained for Warpium.

## Versioning

A Warpium version is the upstream Warp release it is based on plus a fork counter as one extra dot component, so upstream `a.b.c` becomes Warpium `a.b.c.d`:

| | Upstream Warp | Warpium |
| --- | --- | --- |
| Release tag, About page | `v0.2026.06.03.09.49.stable_00` | `v0.2026.06.03.09.49.stable_00.1` |
| `CFBundleShortVersionString` / `CFBundleVersion` | `0.2026.06.03.09.49.00` | `0.2026.06.03.09.49.00.1` |

The single source of truth is [`WARPIUM_VERSION`](WARPIUM_VERSION) (`UPSTREAM_TAG` and `FORK_COUNTER`). [`script/warpium/version`](script/warpium/version) prints the derived values (`--tag`, the default, or `--plist`), and CI exports the tag as `WARPIUM_VERSION_TAG` for `script/bundle`: it is baked into the app for the About page and converted into the bundle version by `script/update_plist`. It is deliberately not passed as `GIT_RELEASE_TAG`, which would make the app behave like an official Warp release (remote-server version checks, changelogs, update prompts). Every CI build carries this version; builds made between releases show the current version, and their artifact name adds the commit.

**Releasing a new Warpium version on the same upstream base:**

```bash
# 1. Bump FORK_COUNTER in WARPIUM_VERSION (e.g. 1 -> 2) and merge that to master.
# 2. Tag the merged commit with the derived version and push the tag:
git tag "$(script/warpium/version)"
git push origin "$(script/warpium/version)"
```

The workflow only reacts to tags with the extra fork component, and fails if the pushed tag doesn't match `WARPIUM_VERSION`.

**After merging upstream Warp into `master`:** find the newest upstream release tag whose commit is already part of `master`, prefer a `stable_` tag, set it as `UPSTREAM_TAG`, and reset `FORK_COUNTER` to `1`:

```bash
git fetch https://github.com/warpdotdev/warp 'refs/tags/v*:refs/tags/upstream/v*' --no-tags
git tag --merged master --list 'upstream/v*.stable_*' --sort=-version:refname | head -n 1
```

Upstream stopped publishing release tags to the public repository after June 2026, so the upstream part may be older than the upstream commit Warpium is actually built from.

## Building locally

Warpium builds from source the same way as Warp's OSS channel. On macOS you need Xcode 26, Homebrew, and `rustup`. The Rust toolchain version is pinned in `rust-toolchain.toml`.

```bash
./script/bootstrap      # platform setup (Xcode, brew packages such as protobuf, cargo tools)
./script/run            # build and run a debug Warpium.app
```

To produce a release `.dmg` like the CI build:

```bash
brew install create-dmg
./script/install_cargo_bundle
./script/install_cargo_release_deps --no-build-deps
export WARPIUM_VERSION_TAG="$(script/warpium/version)"
./script/bundle --channel oss --arch aarch64 --selfsign --dmg-name-suffix "$WARPIUM_VERSION_TAG-arm64"
```

The `.dmg` and `Warpium.app` are written to `target/release-lto/bundle/osx/`.

See [AGENTS.md](AGENTS.md) for the upstream engineering guide (code style, testing, and platform notes).

## Branding

The Warpium logo is an original mark: a geometric "W" with an amber dot, a nod to the "-ium" of a chemical element. Its SVG sources live in [`branding/`](branding) (app icon, small-size icon, README banner, and `.dmg` background), and the in-app logos are in `app/assets/bundled/svg/warpium-*.svg`. After editing a source, regenerate the PNG, `.ico`, and `.icns` files with:

```bash
uv run script/warpium/render_icons.py
```

The macOS 26 (Liquid Glass) icon is `app/channels/oss/icon/AppIcon.icon`, compiled by `script/compile_icon` during bundling.

## Licensing

Warpium keeps upstream Warp's licensing unchanged:

- The UI framework crates (`warpui_core` and `warpui`) are licensed under the [MIT license](LICENSE-MIT).
- The rest of the code is licensed under the [GNU AGPL v3](LICENSE-AGPL).

Under the AGPL, the complete source code for every Warpium build is available in this repository. The Warpium wordmark is set in Roboto (Apache License 2.0).

## Trademarks

"Warp" and the Warp logo are trademarks of their respective owner. Warpium uses the name only to describe where it comes from; it is not an official Warp product, and its logo is not derived from Warp's.
