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

User-visible names come from `Channel::app_display_name()` (`"Warpium"` on Oss). The first-run
onboarding slides take their OSS logo (the full-colour mark), welcome title and subtitle,
"Customize your …" title and "Get started" button from
`crates/onboarding/src/branding.rs`. Text that refers to Warp's own service keeps the Warp name: its
account, Terms of Service, AI features and plans, Warp Agent, Warp Drive and the Warp plugins for
CLI agents.

Monochrome SVGs drawn with the `Icon` element (such as `warpium-mark-neutral.svg`) must be white:
`Icon` tints by the image's red channel, so black shapes render as nothing.

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
- **Notification.** A workspace banner, "N new upstream Warp commits", followed by the latest
  upstream commit date and a reminder that these are source changes, with nothing to install, with
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

## Warpium self-update

Warpium offers and installs new **Warpium** releases from the fork's GitHub Releases. It never
offers, fetches or installs official Warp builds, and never contacts Warp's update servers: Warp's
own updater stays off for the OSS channel. `app/src/bin/oss.rs` sets `autoupdate_config: None`, and
for `Channel::Oss` `app/src/autoupdate/` doesn't start its polling loop (so update checks never
run), refuses to fetch channel versions from Warp's servers or build release asset URLs, and returns
no changelog (the OSS channel has none in Warp's channel versions). Warp's **Check for updates**
(command palette and Settings) runs the Warpium update check instead.
`crates/warpium_self_update/src/oss_autoupdate_guard_tests.rs` fails if any of the guards in
`app/src/autoupdate/` change, and `self_update::init` debug-asserts the config at runtime.
Fork-only code lives in `crates/warpium_self_update` (protoc-free, unit-tested with
`cargo test -p warpium_self_update`) and `app/src/warpium/self_update.rs`. OSS channel on Apple
silicon Macs only (releases ship arm64 disk images).

- **Current version.** `WARPIUM_VERSION_TAG`, baked in by CI (see [Versioning](#versioning)).
  Builds without it (local `./script/run` builds) never offer updates.
- **Check.** On launch and then every `warpium.update_check_interval_hours` (default 6, 1–168),
  counted across launches, while `warpium.update_check` is on (default):
  `GET /repos/MILKA-TOP/warpium/releases/latest`, unauthenticated, revalidated with its `ETag`.
  Back-off, rate-limit handling and the HTTP plumbing are shared with the
  [upstream check](#upstream-warp-update-notification). Drafts, prereleases and tags that aren't
  Warpium versions are ignored. Versions compare numerically, upstream part first (date, then
  channel build) and then the fork counter, so `….stable_00.10` > `….stable_00.2`. State, including
  the latest release and the skipped tag, is kept in private preferences under
  `WarpiumSelfUpdateState`.
- **Offer.** A workspace banner, "Warpium update available", with **Update now**, **Release
  notes**, **Skip this version** (never offered again; a newer release is) and a close button
  (hides it until the next launch). It takes priority over the upstream-commits banner, which is
  informational and about source code. **Check for Warpium updates** in the command palette always
  reports a result in a toast and brings back a skipped or closed offer; **Enable/Disable Warpium
  update checks** toggles `warpium.update_check`.
- **Banner text.** The banner elides its text from the end to make room for its actions, and a full
  tag is about 30 characters, so headings are short ("Warpium update available", "Downloading
  update… 42%", "Update ready", "Couldn't update Warpium") and versions go in the description by
  their short name, the upstream release date and time and the fork counter (`2026.06.03.0949.2`
  for `v0.2026.06.03.09.49.stable_00.2`). It leaves out the upstream channel, so when the release's
  short name is the running version's, full tags are shown. Toasts show full tags.
- **Install.** Warp's macOS updater can't be reused: it downloads only from Warp's release servers
  and requires Warp's Apple team id in the code signature, which ad-hoc signed Warpium builds can't
  have. Warpium has its own path, modelled on it:
  1. **Update now** downloads `Warpium-<tag>-arm64.dmg` and its `.sha256` into
     `warpium-update/<pid>/<tag>/` in the app's cache directory, with progress in the banner.
     Download URLs are built from the tag (never taken from the API response or saved state), so
     they always point at this repository's releases;
  2. refuse unless the SHA-256 (and the size GitHub reports) matches;
  3. check that the app can be replaced where it is: a writable folder, not App Translocation;
  4. `hdiutil attach -nobrowse -noautoopen -readonly` at a private mountpoint;
  5. require `CFBundleIdentifier` `dev.milka.Warpium` and `CFBundleShortVersionString` equal to the
     release's plist version;
  6. `ditto` the app to a hidden sibling, `.Warpium-update-<pid>.app`, check it again,
     `codesign --verify --deep` (integrity of the ad-hoc signature) and clear
     `com.apple.quarantine` so Gatekeeper doesn't block it; detach the image. The running app is
     untouched so far, and the banner says "Update ready" with **Restart now**: Warpium
     never restarts by itself;
  7. the swap happens while the app terminates, after any quit confirmation, so the running app
     never uses a replaced bundle. **Restart now** quits (cancellable, like any quit) and relaunches;
     an ordinary quit installs the staged version without relaunching. The staged copy is swapped
     with the running bundle atomically (`renamex_np(RENAME_SWAP)`, as Warp does), the old bundle
     becomes `.Warpium-previous.app`, and on a restart a shell waits for the old process to exit,
     then runs `open -n` on the bundle, as Warp's updater does;
  8. the first launch of the new version deletes the backup (retried on later launches if that
     fails). Every launch also removes downloads and staged copies left by processes that are gone.
     Releases older than the updater (`v0.2026.06.03.09.49.stable_00.1` and earlier) don't do this,
     so installing one this way (for example while trying the updater with the override below)
     leaves `.Warpium-previous.app` next to `Warpium.app`; delete it by hand.

  Any failure leaves the running app untouched and shows the error with **Download manually** (the
  release page) and **Try again**, which reuses a verified download; when the app can't be replaced
  where it is, **Reveal DMG** shows the verified download instead. To roll back by hand, quit Warpium and put
  `.Warpium-previous.app` back (it is next to `Warpium.app` until the new version has launched).
- **Trying it:** make a build pretend to be older, so it is offered the latest release.
  `WARPIUM_UPDATE_CURRENT_VERSION_OVERRIDE` set at runtime overrides the embedded version:

  ```bash
  WARPIUM_UPDATE_CURRENT_VERSION_OVERRIDE=v0.2026.05.01.00.00.stable_00.1 \
    /Applications/Warpium.app/Contents/MacOS/warp-oss
  ```

  **Update now** and then **Restart now** install the latest release over the app it was started
  from (so test on a copy, or in the `warpium-mac` VM). The installed release must
  match the release's plist version, so the override only changes what is offered, not what is
  accepted.

## Project toolbar

A JetBrains-style toolbar in the top-right corner of the title bar, for the active tab's project.
Fork-only code lives in `crates/warpium_project_toolbar` (protoc-free, unit-tested with
`cargo test -p warpium_project_toolbar`: the config format, the override merge, project
detection, trust bookkeeping, run-configuration entries and the overflow fit),
`app/src/warpium/project_toolbar.rs` (the view, file watching, persisted state),
`app/src/warpium/project_toolbar_row.rs` (the element that moves buttons into the overflow
menu), `app/src/workspace/view/warpium_project_toolbar.rs` (running the actions) and
`app/src/pane_group/warpium_project_toolbar.rs` (splits).

- **Project.** First match wins: the tab's pinned Project Explorer root, the nearest ancestor of
  the local session's working directory with a `.warpium/` directory, then its git repository
  (a `.git` file counts, for worktrees). Remote sessions have no project.
- **Config.** The team config is `<root>/.warpium/toolbar.toml` (`[[button]]` entries with an
  `id` and one of `command`, `workflow`, `launch_config`, `tab_config`, `url` or `action`; see
  `TEAM_TEMPLATE` in `crates/warpium_project_toolbar/src/template.rs`). Personal overrides are
  `~/.warp-oss/projects/<folder-name>-<hash of the root path>/toolbar.toml`, merged by `id`: same
  id overrides the fields it sets, `hidden = true` removes the button, a new id adds one. Both are
  watched and reloaded on change; an invalid file shows a warning button that opens it.
  `action` ids are the parameterless UI actions of the `warpctrl` catalog.
- **Layout.** Buttons with `run_config = true` and the launch configurations with a pane inside
  the project go in the run-configurations dropdown, with a run button; the selection is
  remembered per project. The other buttons sit in a flexible title-bar slot, added only while
  the toolbar has something to show: it gets a share of the width like one more tab (with
  vertical tabs, half of the space the title-bar search bar would otherwise take). The buttons
  that don't fit move, in order, into the overflow menu.
- **Merge leniency.** A personal override without an action whose team button no longer exists
  is ignored rather than invalidating the toolbar.
- **Trust.** Commands whose action comes from the team file run only after the user trusts that
  exact file version (its SHA-256). The prompt lists every team command with its target, `cwd` and
  `env`, and comes back whenever the file changes. Personal commands and non-command actions
  never ask. Trust and run-configuration selections are kept in private preferences under
  `WarpiumProjectToolbarState`, never in the project.
- **Running a command.** `current` types it into the active session wrapped so that `cwd` and
  `env` don't leak into that session (a subshell in POSIX shells, `pushd`/`begin` in fish, a
  script block in PowerShell); a busy or remote session falls back to a new tab. `new_tab`,
  `split_right` and `split_down` start a new session in `cwd` with `env` set.
- **Setting:** `warpium.project_toolbar` (default on), in Settings → Features → Warpium and as
  "Enable/Disable project toolbar" in the command palette, next to "Edit project toolbar" and
  "Edit personal toolbar overrides". OSS channel only.

## Kotlin/Java code index (Go to Class / Symbol / Definition, Find Usages)

An in-process declaration index for Kotlin and Java projects, KMP and plain JVM alike: phases 0
to 2 of [the research report](docs/warpium/research/kotlin-navigation-and-toolchain.md). No
Gradle, JVM or language server is involved. Fork-only code lives in `crates/code_index`
(protoc-free, unit-tested with `cargo test -p code_index`), `app/src/warpium/code_index.rs` (model,
watcher, snapshots, navigation requests), `app/src/warpium/code_index_palette.rs` (palette rows)
and `app/src/warpium/code_navigation.rs` (the editor's request type).

- **What is indexed.** `.kt` and `.java` files under the project root, honoring `.gitignore` (also
  outside git) and skipping hidden and `build/` directories, plus each Gradle module's
  `build/generated/` sources (`R.java`/`BR.java` excluded). Declarations: classes, interfaces,
  objects, companion objects, enums and entries, annotations, records, type aliases, functions
  (with extension receivers), properties (including `val`/`var` constructor parameters),
  constructors and nested types, with package, file, span, visibility, `expect`/`actual`, the
  file's imports and the set of identifiers its code uses (outside comments and strings, for Find
  Usages). Local declarations inside function bodies are left out. The module and source set
  (`commonMain`, `iosMain`, `main`, `test`, …) come from the path, so test and generated code can
  rank lower and `expect` declarations link to their `actual`s by qualified name. Modules named as
  test fixtures (`core/testing`, `*-test`, `*-testing`, `test-fixtures`, `testFixtures`, …) count as
  test code.
- **Search.** Tiers: exact > prefix > camel humps from the first word (`UVM`, `UsViMo` →
  `UserViewModel`) > camel humps from a later word (`VM`) > subsequence. Within a tier, project
  sources rank above tests and generated code, then shorter names. `pkg.Name` narrows by package or
  enclosing class; `String.shout` finds extension functions by receiver.
- **Palettes.** **Go to Class** (`ctrl-cmd-O`) and **Go to Symbol** (`ctrl-alt-cmd-O`), both
  editable and listed in the command palette. JetBrains' `cmd-O`/`cmd-alt-O` are taken by Warp
  (Files palette, `/open-repo`). Enter opens the declaration in the editor at its name.
- **Go to Definition** (`cmd-B`, cmd-click, or the editor's context menu) and **Find Usages**
  (`alt-F7`) in `.kt`, `.kts` and `.java` editors, both editable. The editor sends its text
  (unsaved edits included) and cursor to the workspace, which resolves it on a background executor
  (`code_index::navigate`). One declaration opens at its name; several, or usages, are listed in a
  palette (`QueryFilter::CodeNavigation`) that filters by path or line text. On a declaration's own
  name, Go to Definition shows its usages instead, as in the JetBrains IDEs. The bindings only
  apply where the editor's keymap context has `WarpiumCodeNavigation` (navigable file, setting on),
  and cmd-click is only offered when no language server serves the file.
- **Definition resolution** (`crates/code_index/src/resolve`). The current file is parsed; names
  resolve through local variables and parameters, enclosing classes (with companion objects and
  their declared supertypes), the file, explicit imports (aliases too), the package, star imports
  and finally every declaration with the name (marked *name only*). `receiver.member` uses the
  receiver's type when it is written nearby: a declared or constructed type of a local, parameter
  or property, a class name (static access), `this`, or `new Foo()`. Callable references
  (`Type::member`) and fully qualified names work. An `expect`/`actual` group resolves to the
  `actual` of the reference's own source set, else the `expect`, with the others as counterparts.
  Platform source sets the reference can't see rank last.
- **Usages** (`crates/code_index/src/usages.rs`). Candidate files are the ones whose identifier set
  holds the name. A type or top-level declaration counts in files that can see it (same package,
  import, star import); a member counts in files that name its class (a receiver type or supertype
  has to be written down for exact resolution). Files that also see or declare another declaration
  with the name are parsed and each mention resolved; otherwise every mention counts. Usages only
  matched by name are marked `≈`. At most 400 files are parsed per query and files over 128 KB never
  are (parsing is most of the cost, about 1 ms per 7 KB); past that, mentions in files naming the
  class count as approximate.
- **Project root.** When a Go to palette opens: the tab's pinned Project Explorer root, else the
  repository of the active session, else its directory if that holds Gradle or Maven build files.
  The home directory and `/` are never indexed. One project is held at a time; switching cancels
  the previous build between batches and drops its index. The watcher is registered on the
  enclosing git repository, because `repo_metadata` routes events to the deepest registered
  directory. Registrations are never removed, since `DirectoryWatcher` has no removal API.
- **Threads and memory.** Builds run on a dedicated thread with a bounded rayon pool (half the
  cores, at most 8) that exists only for the build, parsing in batches of 512 files. The palette
  searches on a background executor under a shared read lock. Saves are reparsed off the main
  thread from `repo_metadata` watcher events.
  Go to Definition and Find Usages index the project holding the file (its git repository, else
  the outermost directory with Gradle settings) when the current project doesn't.
- **Snapshot.** `<user cache dir>/dev.milka.Warpium/code-index/<hash of the root>/index.bin`
  (`~/Library/Caches/…` on macOS): a versioned binary format (`FORMAT_VERSION`) with a deduplicated
  string table. Nothing is written inside the project. On open, the snapshot is loaded (the palette
  can search right away), the tree is rescanned by size and modification time, and only changed
  files are reparsed. The snapshot is rewritten after a rebuild and 10 s after the last change. Bump
  `FORMAT_VERSION` whenever extraction rules change.
- **Setting:** `warpium.code_index` (default on, OSS channel only), toggled from the command
  palette with "Enable/Disable Kotlin/Java code index (Go to Class/Symbol)".

### MCP server for agents

`warpium-code-index` serves the index over MCP (stdio) as a candidate replacement for `ast-index`.
Tools: `find_class`, `find_symbol` (optional `kind`), `file_outline`, `find_definition` and
`find_usages`. The last two take `path`, a 1-based `line`, and either a 1-based `column` or the
identifier's `name` on that line (agents usually know the name, not the column). Results are compact
text lines with kind, name, package, `file:line` and source set, with `expect`/`actual` counterparts
listed under each hit; usages are `path:line:column` plus the line, `≈` when only the name
matched. It shares the app's snapshot for the same root and rescans before a call when
its last scan is more than 2 s old.

```bash
cargo build --release -p code_index --bin warpium-code-index
claude mcp add code-index -- "$PWD/target/release/warpium-code-index" mcp --root /path/to/project
# Without MCP:
target/release/warpium-code-index search --root /path/to/project --types UVM
```

Not done yet: shipping the binary in `Warpium.app` and registering it in Warpium's MCP settings.

### Benchmark (phase 0)

[`script/warpium/code_index_bench`](script/warpium/code_index_bench) shallow-clones the corpus
(nowinandroid, Thunderbird, DuckDuckGo, Signal) at pinned commits into the user cache dir and runs
`crates/code_index/examples/bench.rs` in release mode. For each root it reports cold full-index time,
snapshot size, warm start (load, and load plus rescan), p50/p95/max latency of ~100 Go to Class and
Go to Symbol queries (a fixed list plus names sampled from the corpus), Go to Definition latency on
~550 references sampled from the corpus (names after a dot and capitalized names; the share
resolved is low where most references are to libraries, which aren't indexed), Find Usages latency
for 50 sampled classes and functions, update time after editing a file, and peak RSS. A separate process measures the warm start, so its peak RSS is what the index
keeps resident.

```bash
script/warpium/code_index_bench                   # all four corpora
script/warpium/code_index_bench --combined        # also all four as one ~260k-symbol root
script/warpium/code_index_bench thunderbird       # one corpus
CODE_INDEX_BENCH_ARGS="--threads 4" script/warpium/code_index_bench
# A private project: label it so its path stays out of the output, and share aggregates only.
cargo run --release -p code_index --example bench -- --name private /path/to/project
```

Inside the dev container, keep projects for timing off the `/workspace` bind mount. Directory walks
there are about 40 times slower than on a local disk, which inflates rescan and cold-build times.

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
