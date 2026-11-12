# Warpium

Warpium is a personal fork of [Warp](https://github.com/warpdotdev/warp), the open-source agentic development environment born out of the terminal. It builds Warp's open-source (`oss`) channel under its own name and bundle identifier, with a small set of personal quality-of-life changes on top.

All credit for Warp goes to its original authors at [warpdotdev/warp](https://github.com/warpdotdev/warp). This fork tracks upstream and tries to keep its own changes small so that upstream updates merge cleanly.

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

Builds are produced by the [Warpium Build](.github/workflows/warpium-build.yml) GitHub Actions workflow. Only Apple silicon (arm64) macOS builds are published for now.

- **Latest development build:** open the [Actions tab](https://github.com/MILKA-TOP/warpium/actions/workflows/warpium-build.yml), pick a successful run on `master`, and download the `Warpium-<commit>-macos-arm64` artifact (a zipped `.dmg`).
- **Tagged releases:** pushing a `v*` tag publishes a [GitHub Release](https://github.com/MILKA-TOP/warpium/releases) with the `.dmg` attached.

CI builds are currently cross-compiled for arm64 on Intel runners, which can't run the arm64 binary to generate `settings_schema.json`. As a result, the bundled `modify-settings` agent skill is not available in CI builds; local builds on Apple silicon include it.

Warpium builds are ad-hoc signed and **not notarized** by Apple, so macOS will refuse to open them at first. After copying `Warpium.app` to `/Applications`, clear the quarantine flag:

```bash
xattr -dr com.apple.quarantine /Applications/Warpium.app
```

Warpium uses its own bundle identifier (`dev.milka.Warpium`), so it can be installed next to the official Warp app. On macOS its preferences, `~/Library/Application Support/dev.milka.Warpium` data, and keychain entries are separate from Warp's, while `~/.warp-oss` (for example `settings.toml`, themes, and skills) is still shared with upstream's OSS build. Warpium also keeps the OSS build's `warposs://` URL scheme and `warp-oss` CLI name, so avoid installing it next to an upstream `WarpOss.app`: sign-in redirects may open the wrong app.

Only macOS builds are produced and tested. Linux and Windows packaging scripts are inherited from upstream and are not maintained for Warpium.

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
./script/bundle --channel oss --arch aarch64 --selfsign --dmg-name-suffix arm64
```

The `.dmg` and `Warpium.app` are written to `target/release-lto/bundle/osx/`.

See [AGENTS.md](AGENTS.md) for the upstream engineering guide (code style, testing, and platform notes).

## Licensing

Warpium keeps upstream Warp's licensing unchanged:

- The UI framework crates (`warpui_core` and `warpui`) are licensed under the [MIT license](LICENSE-MIT).
- The rest of the code is licensed under the [GNU AGPL v3](LICENSE-AGPL).

Under the AGPL, the complete source code for every Warpium build is available in this repository.

## Trademarks

"Warp" and the Warp logo are trademarks of their respective owner. Warpium uses the name only to describe where it comes from; it is not an official Warp product.
