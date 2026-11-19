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
  `tools/mac-mcp/` (on the `claude/mac-mcp` branch until that is merged). That VM is now
  `muhtar-test`, the only VM on the owner's Mac, owned by lipt-mocker's `tools/mac-mcp`; the
  Warpium server runs in it on 17654 next to the Muhtar server on 17655 (shared VM mode, see
  `tools/mac-mcp/README.md`, "Using the Muhtar VM"). Don't test Warpium and Muhtar UI there
  at the same time, and use CI artifacts rather than in-guest builds (disk).

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

- **Project.** First match wins: the tab's locked [project root](#tab-project-root) (a locked
  tab never falls back to its location), the nearest ancestor of the tab's location with a
  `.warpium/` directory, then its git repository (a `.git` file counts, for worktrees). The
  location is the folder of the focused editor's file (code or notebook pane), else the working
  directory of the tab's active session. Remote sessions have no project; a tab with neither a
  session nor a file (only settings, say) keeps the previous project. The toolbar follows tab
  switches, pane focus, editor file switches (through the pane title) and finished commands.
- **Config.** The team config is `<root>/.warpium/toolbar.toml` (`[[button]]` entries with an
  `id` and one of `command`, `workflow`, `launch_config`, `tab_config`, `url` or `action`; see
  `TEAM_TEMPLATE` in `crates/warpium_project_toolbar/src/template.rs`). Personal overrides are
  `~/.warp-oss/projects/<folder-name>-<hash of the root path>/toolbar.toml`, merged by `id`: same
  id overrides the fields it sets, `hidden = true` removes the button, a new id adds one. Both are
  watched and reloaded on change; an invalid file shows a warning button whose tooltip has the
  error and which opens the file at the error's line (the TOML span, or the offending
  `[[button]]` header for validation errors).
  `action` ids are the parameterless UI actions of the `warpctrl` catalog.
- **Layout.** Buttons with `run_config = true` and the launch configurations with a pane inside
  the project go in the run-configurations dropdown, with a run button; the selection is
  remembered per project. The other buttons sit in a flexible title-bar slot, added only while
  the toolbar has something to show: it gets a share of the width like one more tab (with
  vertical tabs, half of the space the title-bar search bar would otherwise take). The buttons
  that don't fit move, in order, into the overflow menu.
- **Placement.** `overflow = true` keeps a button in the overflow menu (after the buttons that
  don't fit); the overflow button then always shows.
- **Editor.** `app/src/warpium/project_toolbar_editor.rs`, a modal owned by the workspace
  (`ProjectToolbarOverlays` in `app/src/workspace/view/warpium_project_toolbar.rs`). Opened from
  the "+" at the end of the toolbar, "+ Add button" (what a project without buttons shows, so any
  project can get some), "Customize toolbar…" in the overflow and run-configurations menus, a
  button's right-click menu (Edit…, Duplicate, Move left/right, Hide, Delete; all but Edit save
  right away, and Delete on a personal override only drops the override) and the palette's "Customize project toolbar". Its model is
  `warpium_project_toolbar::EditorModel`: both files' buttons in toolbar order (team-file buttons,
  then personal-only ones; moves stay within a group), each with a scope. Project scope writes the
  team file; Personal scope writes a personal entry: for a project button, an override with only
  the fields that differ (so the team's command stays the team's and keeps needing trust unless
  the user changed it; the team entry is left alone), otherwise a full button. Hiding writes
  `hidden = true` in the personal file; entries the team file hides itself are kept but not
  shown. A copy of a project button goes in the team file. Test run of a button that runs the
  team's own command needs the team file to be trusted.
  Writes go through `toml_edit`: each button's table is updated in place, only changed keys are
  rewritten (keeping trailing comments), tables move with their comments when reordered, the
  file's header comment stays on top, the first button of a commented template goes below the
  comments, and a missing file is created only when it gets a button. Every button is validated
  first (errors show inline and select the button), and the result is parsed and merged before
  anything is written. Saving refuses if either file changed on disk since the editor opened.
- **Trust after a GUI save.** Writing the team file from the editor is the user's own edit: when
  the version it replaces was trusted (or had no commands, or there was no file), the new version
  is trusted at once (`ToolbarUserState::record_gui_save`), so there is no prompt. An untrusted
  version stays untrusted (saving one button must not vouch for a teammate's commands), and any
  later external change asks again as before.
- **Variables.** Commands can use `${projectRoot}`, `${file}` and `${fileDir}` (the focused code
  or notebook editor's file), `${selection}` (its selection, else the focused terminal's),
  `${cwd}` (the active session's directory, else the root) and `${prompt:Label}` (asked in a
  dialog, `project_toolbar_prompt.rs`, before running; Test run too). Each value is quoted as one
  word for the shell that runs it (the session's, or `$SHELL`'s family for a new tab or split);
  any other `${...}` (such as `${HOME}`) is left for the shell, and `$${` is a literal `${`. A missing file or selection shows a toast and runs nothing. Variables
  apply to `command` only, not `cwd` or `env`, and trust compares the unexpanded command.
- **Merge leniency.** A personal override without an action whose team button no longer exists
  is ignored rather than invalidating the toolbar.
- **Trust.** Commands whose action comes from the team file run only after the user trusts that
  exact file version (its SHA-256). The prompt lists every team command with its target, `cwd` and
  `env` in a scrollable list (its buttons stay in view), and comes back whenever the file changes.
  It lists the team file's own commands, without personal overrides. When an earlier version was
  trusted, new and changed commands (by button id; a change in label, command, target, `cwd` or
  any `env` value counts) come first, marked "New" or "Changed", then the
  unchanged ones under their own heading, and the intro counts new, changed, unchanged and removed
  commands. Personal commands and non-command actions never ask. Trust, a fingerprint of each
  trusted command (to diff against) and run-configuration selections are kept in private
  preferences under `WarpiumProjectToolbarState`, never in the project.
- **Running a command.** `current` types it into the active session wrapped so that `cwd` and
  `env` don't leak into that session (a subshell in POSIX shells, `pushd`/`begin` in fish, a
  script block in PowerShell). `panel` runs it in the tab's [bottom panel](#bottom-panel), reusing
  the button's idle sub-tab. When another kind of pane, such as an editor, is focused, the
  tab's most recently focused session takes it if its working directory is inside the project. A
  busy or remote session, or no such session, falls back to a new tab. `new_tab`,
  `split_right` and `split_down` start a new session in `cwd` with `env` set.
- **Setting:** `warpium.project_toolbar` (default on), in Settings → Features → Warpium and as
  "Enable/Disable project toolbar" in the command palette, next to "Edit project toolbar" and
  "Edit personal toolbar overrides". OSS channel only.

### IDE run configurations

The project's JetBrains (Android Studio, IntelliJ IDEA) run configurations are listed in the
run-configurations dropdown under an "IDE run configurations" heading, after Warpium's own
entries. Fork-only code lives in `crates/warpium_ide_run_configs` (protoc-free, unit-tested with
`cargo test -p warpium_ide_run_configs` on synthetic XML: reading, macro expansion, module
resolution, mapping, compounds, trust) and `app/src/warpium/project_toolbar_ide.rs` (a child module
of the toolbar view: loading, the menu section, trust and running).

- **Sources** (only read, never written): shared `<root>/.run/*.run.xml` and
  `<root>/.idea/runConfigurations/*.xml` (a `ProjectRunConfigurationManager` component or a bare
  `<configuration>`), and personal configurations in the `RunManager` component of
  `<root>/.idea/workspace.xml`. Templates (`default="true"`) are skipped; `temporary="true"` ones
  only appear when the project has no other configuration. A name and type seen twice keeps the
  first (shared before personal). A `workspace.xml` that is in the git index (a substring check of
  `.git/index` for the path, which can miss index version 4) counts as shared. Files over 8 MB,
  invalid XML and configurations without a name or type are skipped with a warning, logged and
  listed as a disabled entry whose tooltip names them.
- **Mapping.** `$PROJECT_DIR$`, `$ProjectFileDir$`, `$MODULE_DIR$`, `$MODULE_WORKING_DIR$` and
  `$USER_HOME$` are expanded. IDE module names (`MyApp.app.main`, `My Project.core.data.unitTest`)
  map to the Gradle project whose directory has a build script, dropping leading parts and, only
  when the name doesn't match as written, a trailing source set.
  - `GradleRunConfiguration`: `./gradlew [-p <externalProjectPath>] <taskNames> <scriptParameters>`
    plus the `-D` system properties of `vmOptions`, run from the nearest wrapper at or above the
    project path (inside the root), with its `env`; `gradle` in the project path without one.
  - `ShConfigurationType`: `[interpreter options] '<script>' <options>` (or the script text when
    `EXECUTE_SCRIPT_FILE` is false) in `SCRIPT_WORKING_DIRECTORY`, else the script's folder, with
    `envs`.
  - `AndroidRunConfigurationType`: the Android run flow (`RunConfigKind::AndroidApp`'s), with the
    launch option: `default_activity`, `specific_activity` (`am start -n <pkg>/<ACTIVITY_CLASS>`),
    `deep_link` (`am start -a VIEW -d`) or `do_nothing` (install only). Configurations that don't
    deploy, or deploy from a bundle, are unsupported. The module has to be one of the Android
    application modules the toolbar found.
  - `JUnit` and `AndroidJUnit`: `./gradlew :mod:<task> --tests '<class>[.<method>]'` (or
    `'<package>.*'`), the task from the source set: `test` (`testDebugUnitTest` for Android JUnit and
    `unitTest`), or `<name>Test` for KMP targets (not `commonTest`). Other test scopes are
    unsupported. `AndroidTestRunConfigurationType`: `:mod:connectedDebugAndroidTest` with the
    runner's `class`/`package` argument.
  - `CompoundRunConfigurationType`: its members' steps in order, when every member is supported
    (nesting up to 8 levels). Command members after the first open in a new tab, as each gets its
    own tab in the IDE.
  - Kotlin and Java main classes, Kotlin scripts and every other type are listed disabled with the
    reason in the tooltip.
  Commands run like a toolbar command with `target = "current"`; `${` is escaped as `$${`, so
  shell parameter expansions aren't read as toolbar variables.
- **Trust.** Commands of shared configurations need trust like team toolbar commands: a fingerprint
  of every shared command (name, command, `cwd`, `env`) is trusted at once, kept in
  `WarpiumProjectToolbarState` as `trusted_ide_run_configs`/`trusted_ide_commands`, and the prompt
  lists them with New/Changed marks after a change. Personal configurations never ask. Android runs
  ask for the project's Gradle build trust as before.
- **Selection.** Remembered per project like other entries; until the user picks one, the
  configuration `RunManager`'s `selected` names (`Gradle.detekt`, `Android App.app`) is the default.
- **Reloading.** `.run/`, `.idea/` and `.idea/runConfigurations/` are watched with the toolbar's
  watcher (non-recursive); changes to `.run/**`, `.idea/runConfigurations/**` or
  `.idea/workspace.xml` reread everything on a background thread.
- **Setting:** `warpium.ide_run_configs` (default on, OSS channel only), in Settings → Features →
  Warpium and as "Enable/Disable IDE run configurations in the project toolbar" in the command
  palette.

## Bottom panel

A per-tab dock below the split layout for panes whose processes should keep running out of the
way (JetBrains-style tool window, roadmap item 14; design in
[docs/warpium/research/bottom-panel.md](docs/warpium/research/bottom-panel.md)). Fork-only code
lives in `crates/warpium_bottom_panel` (protoc-free, `cargo test -p warpium_bottom_panel`: sub-tab
order, selection, expanded/collapsed/maximized state, reordering, origins and reuse of a command's
sub-tab, unread output, chip status, the saved JSON, restore position, drop side and the fresh
terminal's directory), `app/src/pane_group/warpium_bottom_dock.rs` (the dock in `PaneGroup`:
move/restore/close, hooks, drag and drop, the sub-tab menu, bindings), `warpium_bottom_dock_render.rs`
(the panel and its chips), `warpium_bottom_dock_persistence.rs` (snapshot and restore),
`app/src/workspace/view/warpium_bottom_panel.rs` (window-wide actions, moving sub-tabs between
tabs, Logcat in the panel), `app/src/warpium/bottom_panel.rs` (setting gate and workspace
bindings), `app/src/persistence/warpium_bottom_panel.rs` (blocks of docked terminals) and
`app/src/terminal/view/warpium_bottom_panel.rs` (menu items, chip status, output generation).
App tests: `pane_group::tests::warpium_bottom_dock_tests` and the `*warpium*` sqlite tests.

- **Setting.** `warpium.bottom_panel` (default on; `WarpiumBottomPanelSettings`, its own group in
  `sync_settings.rs`), in Settings → Features → Warpium and as "Enable/Disable bottom panel" in the
  palette (keymap flag `WarpiumBottomPanel`). Turning it off restores every docked pane to the
  layout; restoring and closing keep working while it is off. The development feature flag and
  cargo feature are gone.
- **Lifecycle.** A docked pane leaves the layout tree but stays in `PaneGroup::pane_contents`
  without being detached, like an off-tree child agent pane, so its terminal manager and PTY keep
  running and its subscriptions (shell exit, titles) keep working. Quit and close-tab warnings
  count its running commands because they walk `pane_contents`. Closing the tab closes the dock.
  Terminal and Logcat panes can be docked.
- **States.** Empty: nothing is drawn. Expanded: a strip of sub-tab chips over the selected pane,
  resizable from its top edge, or maximized over the layout (the layout stays alive). Collapsed: the
  strip only; clicking a chip expands on it. Each chip shows the kind icon, title (a toolbar
  button's label, else the pane title), a running dot or ✓/✗ with the last exit code
  (`TerminalView::warpium_sub_tab_status`) and an unread dot. Unread output compares
  `TerminalView::warpium_output_generation` (bumped in `handle_terminal_wakeup`, a three-line
  upstream edit) with the generation seen when the sub-tab was last visible. Docked panes don't
  notify the pane group, so a 1 s timer re-renders the chips when their fingerprint changes (it
  stops while the dock is empty).
- **Commands.** Workspace-level (so they work with focus in the Project Explorer): `cmd-j` toggle
  (empty → new terminal in the panel; expanded → collapse and focus the layout; collapsed →
  expand and focus the selected sub-tab), "New terminal in bottom panel", next/previous sub-tab
  `cmd-ctrl-]`/`cmd-ctrl-[` (`cmd-alt-[`/`]` fold code in the editor) and maximize/restore
  `cmd-ctrl-j` (`cmd-shift-J` queues agent prompts). Pane-level: "Move pane to bottom panel",
  "Restore pane to layout"; terminal context and pane header menus offer the same. A chip's
  right-click menu: Restore pane to layout, Move to tab (each other tab, or a new tab), Close.
- **Drag and drop.** Chips are draggable: onto another chip to reorder, onto a layout pane to
  restore it on the side it was dropped, onto the tab bar to move it into that tab's panel (or a new
  tab after the last one). A layout pane's header dragged onto the expanded panel docks it, and a
  docked pane's header dragged onto a layout pane restores it there; both apply on drop (no live
  preview), and the layout's last pane isn't docked by dragging.
- **Rules.** Restoring puts the pane back next to its previous neighbor when that pane is still
  visible, else right of the layout pane focused last. `exit` in a docked terminal closes only its
  sub-tab. Closing or moving away the layout's last visible pane while the dock has panes opens a
  fresh terminal in the layout instead of closing the tab, and so does docking the last one; it
  starts in the tab's locked project root, else the replaced pane's directory. Splits from a docked
  pane split the layout pane focused last; layout navigation from a docked pane returns focus to the
  layout. Focusing a docked pane (from the panel, session navigation or a closed neighbor) shows the
  panel on it.
- **Moving between tabs.** `PaneGroup::take_docked_pane_for_move` (detach `Moved`, never touches the
  layout, so the source tab survives) and `adopt_pane_into_bottom_panel` (attach off-tree), wired by
  `BottomPanelEvent::MoveToTab`; the sub-tab keeps its title and origin.
- **Toolbar `target = "panel"`.** Runs the command in a sub-tab with origin
  `Command { key: "toolbar:<button id>" }` titled after the button. The next run reuses that
  button's sub-tab when it is idle (local, nothing executing, empty input), typed in like `current`
  (wrapped so `cwd`/`env` don't leak); otherwise a new sub-tab "Label (2)". It selects the sub-tab
  (expanding the panel) without taking focus. With the panel off it falls back to a new tab.
- **Android.** With the panel on, Run uses a panel sub-tab per module (`android-run:<module>`,
  reused while idle) and Logcat opens in the active tab's panel. Logcat stays one per window
  (`LogcatPaneManager`): one adb stream per window, and a pane per tab would duplicate it. Opening
  it from another tab moves the window's Logcat into this tab's panel (unless that would close the
  tab holding it as its only pane; then that tab is shown).
- **Persistence.** `TabSnapshot::warpium_bottom_panel` (`warpium_bottom_panel::PanelSnapshot`) is
  saved as JSON in `panels.warpium_bottom_panel` (migration
  `2026-11-19-000000_add_warpium_bottom_panel_to_panels`): entries (terminal uuid as hex and cwd, or
  Logcat), titles, origins, selection, expanded/maximized and height. Reading is lenient (unknown
  entries are dropped, bad JSON drops only the panel). Docked terminals have no `terminal_panes`
  row, so their blocks are read by uuid (`add_docked_terminal_blocks`). Restored terminals get their
  blocks and directory back (no AI conversation or profile restore); Logcat restores only if the
  window has none yet and the Android tools are on. Older databases restore without a panel.
- **Not done.** Undo for closing a docked sub-tab (closing discards it right away); splits inside
  the panel; dragging a docked pane's header onto the tab bar (use the chip instead).
- **Upstream touch points** (one-line hooks into the fork files, check after upstream merges):
  `pane_group/mod.rs` (`PaneGroupAction::WarpiumBottomPanel`, `Event::WarpiumBottomPanel`, the
  `warpium_bottom_dock` field, `close_pane`, `remove_pane_for_move`, `focus_pane`,
  `update_pane_history`, `handle_focus_change`, `add_pane_with_options`, `handle_pane_view_event`,
  `handle_pane_event`, `handle_action`, `keymap_context`, `render`, `reveal_and_focus_pane`),
  `pane_group/focus_state.rs` (docked set), `pane_group/pane/mod.rs` (`PaneEvent::WarpiumBottomPanel`),
  `pane_group/pane/view/mod.rs` (`PaneDropTargetData::pane_id`), `terminal/view.rs` (output
  generation, menu items, `TerminalAction::WarpiumBottomPanel`), `terminal/view/action.rs`,
  `terminal/view/pane_impl.rs` (header menu), `workspace/action.rs` and `workspace/view.rs`
  (`WorkspaceAction::WarpiumBottomPanel`, the setting toggle and keymap flag, the pane group event,
  restore and snapshot), `app_state.rs`, `persistence/sqlite.rs` (save/read, docked blocks),
  `crates/persistence` (model, schema), `settings_view/features_page.rs` (the setting widget),
  `settings/init.rs` and `test_util/settings.rs` (registration).

## Tab project root

Each tab has a project root that the Project Explorer, global search, code review, the project
toolbar and the code index all read through one accessor, `PaneGroup::project_root()`. It has two
modes (`warpium_project_root::ProjectRoot`):

- **Follow** (upstream behaviour): the explorer's roots come from the repositories of the tab's
  terminal working directories and open editors
  (`WorkingDirectoriesModel::refresh_working_directories_for_pane_group`).
- **Locked** to one or more folders, the first being the primary one: the explorer shows exactly
  those folders whatever the terminals do. Terminals in other host folders, inside a container
  (`docker compose run … zsh`, whose paths don't exist on the host) or in an SSH session never add,
  remove or re-root anything (remote repositories an SSH session navigates to aren't added
  either). Terminals inside the project are still attached to their root and repository, and the
  repositories of terminals inside a locked folder are reviewable even when the folder itself
  isn't a repository, so code review and "cd to directory" work for them
  (`app/src/pane_group/warpium_locked_project.rs`). The locked folders' git repositories are
  detected when the project is locked, since no terminal may have visited them. Single-root
  consumers (project toolbar, code index) use the primary folder.

Fork-only code lives in `crates/warpium_project_root` (protoc-free, unit-tested with
`cargo test -p warpium_project_root`: the mode, adding and removing folders, which roots a locked tab
shows and which terminal belongs to each, where the active terminal is, the lock target and the
persisted form), `app/src/warpium/project_root.rs` (actions and palette bindings),
`app/src/workspace/view/warpium_project_root.rs` (applying changes, folder pickers, terminal
placement) and `app/src/workspace/view/warpium_project_header.rs` (the explorer header).

- **Locking.** "Open in new tab" in the explorer and **Open Folder as Project…** (folder picker;
  `cmd-alt-shift-O` because Warp binds `cmd-shift-O` to `/open-repo`) open a new tab locked to the
  folder. The explorer header's pin button toggles Follow ↔ Locked; locking takes the explorer root
  holding the active terminal, else the most recent explorer root, else the terminal's directory
  (`lock_target`). While locked, the header shows the primary folder's name, a folder button
  (**Change project folder…**, which locks to the picked folder alone) and a plus button (**Add folder
  to project…**); each extra folder is listed with a remove button. A folder inside or around an
  existing root isn't added (it would show the same files twice). Palette: "Open Folder as
  Project…", "Lock project to current folder", "Unlock project (follow terminals)", "Change project
  folder…", "Add folder to project…" (the lock/unlock/add entries depend on the
  `WarpiumProjectLocked` keymap context flag).
- **Outside the project.** While locked, the header shows "Terminal is outside the project" when
  the active terminal is in a host folder outside every root, and "… (not a local folder)" when its
  directory isn't on this machine (a remote session, or a local session reporting a path that
  doesn't exist, as inside a container). Nothing else changes.
- **Persistence.** With the tab in sqlite: the primary folder in the existing
  `tabs.pinned_explorer_root` column (added for the earlier "Open in new tab" pin) and the others in
  `tabs.project_extra_roots`, a JSON array of strings. Rows written before either column restore as
  Follow or locked to their pinned folder; builds without the second column still restore the
  primary folder. A locked project is restored as saved even when a folder is missing at launch
  (an unmounted volume); unlock it to follow the terminals again.
- **Future work:** container ↔ host path mappings (for example in `.warpium/project.toml`), so a
  terminal inside a container whose working directory maps into the project counts as inside it;
  locking from the Projects list.

## Kotlin/Java code index (Go to Class / Symbol / Definition / Implementation, Find Usages)

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
  file's imports, the supertypes each class, interface and object declares (for Go to
  Implementation) and the set of identifiers its code uses (outside comments and strings, for Find
  Usages). Local declarations inside function bodies are left out. The module and source set
  (`commonMain`, `iosMain`, `main`, `test`, …) come from the path, so test and generated code can
  rank lower and `expect` declarations link to their `actual`s by qualified name. Modules named as
  test fixtures (`core/testing`, `*-test`, `*-testing`, `test-fixtures`, `testFixtures`, …) count as
  test code.
- **Search.** Tiers: exact > prefix > camel humps from the first word (`UVM`, `UsViMo` →
  `UserViewModel`) > camel humps from a later word (`VM`) > subsequence. Within a tier, project
  sources rank above tests and generated code, then shorter names. `pkg.Name` narrows by package or
  enclosing class; `String.shout` finds extension functions by receiver.
- **Project search (Search Everywhere).** In a Kotlin/Java project, `cmd-O` (Warp's Files palette)
  opens the palette with **All / Classes / Symbols / Files** tabs, as Android Studio does. Click a
  tab, or press `tab` / `shift-tab` in the search field to cycle them (with a filter applied, tab
  did nothing before); the query carries over. **Go to Class** (`ctrl-cmd-O`) and **Go to Symbol**
  (`ctrl-alt-cmd-O`), both editable and listed in the command palette, open the same palette on
  their tab. While it is open, a tab's shortcut switches to that tab, or closes the palette when
  it is already on it (`cmd-O` counts as All's and Files'). Classes and Symbols are the Go to
  Class/Symbol searches (`CodeSymbolDataSource`), Files is Warp's file search, and All
  (`QueryFilter::CodeAll`, `AllTabDataSource` in `app/src/warpium/search_everywhere.rs`) lists
  the best 8 of each under a heading (`Classes · 8 of 37`), classes first, symbols without types,
  then files. The tab order, the merge and the tabs decision are in
  `crates/warpium_search_everywhere` (`cargo test -p warpium_search_everywhere`). Enter opens the
  declaration at its name, or the file. `cmd-O` keeps the plain Files palette unless the setting
  is on (OSS channel) and the project (see **Project root** below) has Gradle or Maven build files
  at its root or was indexed before (a snapshot exists), so other repositories are never crawled;
  it then starts indexing, and drops the tabs once a finished index turns out to hold no sources.
  JetBrains' `cmd-alt-O` is taken by Warp (`/open-repo`).
- **Go to Definition** (`cmd-B`, cmd-click, or the editor's context menu) and **Find Usages**
  (`alt-F7`) in `.kt`, `.kts` and `.java` editors, both editable. The editor sends its text
  (unsaved edits included) and cursor to the workspace, which resolves it on a background executor
  (`code_index::navigate`). One declaration opens at its name; several, or usages, are listed in a
  palette (`QueryFilter::CodeNavigation`) that filters by path or line text. On a declaration's own
  name, Go to Definition shows its usages instead, as in the JetBrains IDEs. The bindings only
  apply where the editor's keymap context has `WarpiumCodeNavigation` (navigable file, setting on),
  and cmd-click is only offered when no language server serves the file.
- **Go to Implementation** (`alt-cmd-B`, as in the JetBrains keymap, or the context menu; editable;
  `crates/code_index/src/hierarchy.rs`): the project types that extend or implement a type, also
  indirectly; the members overriding a function or property (Kotlin members need `override`; Java
  methods only the name, since `@Override` is optional, so overloads count too); the `actual`s of an
  `expect` declaration. A supertype as written counts when it is qualified to the type's name or
  the file sees the type (same file or package, import, star import), or when no other type has
  the name. Nothing is precomputed: a query scans the per-file supertype lists once per inheritance
  level (well under a millisecond on 100k-symbol projects).
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
  matched by name are marked `≈`. As in the JetBrains IDEs, usages of the members overriding a
  member (at most 200, looked up for at most 4 targets) and of `expect`/`actual` counterparts are
  listed too, after the others under a **Via implementations/overrides** heading: a mention in a
  resolved file counts there when it resolves to an override or counterpart, elsewhere when the
  file never names the class of the declaration itself. A file that names both (say `Repo` and
  its implementation `DbRepo`) is resolved, so `DbRepo().load()`, or `db.load()` after
  `val db = DbRepo()`, counts as via while `repo.load()` with `repo: Repo` stays direct. A receiver
  whose type isn't written nearby (`makeRepo().load()`, chained calls) resolves by name and is
  listed as a direct usage, since there is no type inference. Mentions that are indexed declarations of
  the name (an override, an overload) are dropped before deciding to parse a file, so files that
  only implement an interface are never parsed. At most 400 files are parsed per query and files over 128 KB never
  are (parsing is most of the cost, about 1 ms per 7 KB); past that, mentions in files naming the
  class count as approximate.
- **Project root.** When a Go to palette or the project search opens: the tab's locked [project root](#tab-project-root)
  (its primary folder), else the
  repository of the active session, else its directory if that holds Gradle or Maven build files.
  The home directory and `/` are never indexed. One project is held at a time; switching cancels
  the previous build between batches and drops its index. The watcher is registered on the
  enclosing git repository, because `repo_metadata` routes events to the deepest registered
  directory. Registrations are never removed, since `DirectoryWatcher` has no removal API.
- **Threads and memory.** Builds run on a dedicated thread with a bounded rayon pool (half the
  cores, at most 8) that exists only for the build, parsing in batches of 512 files. The palette
  searches on a background executor under a shared read lock. Saves are reparsed off the main
  thread from `repo_metadata` watcher events.
  Names, packages and paths are interned into one string buffer with a hash table of ids (about
  21 bytes per string beyond its text), and the tables are shrunk to fit after a build, rescan or
  load: on 100k-symbol corpora the index holds about 20 MB of heap and a warm process about 36 MB
  of RSS.
  Go to Definition and Find Usages index the project holding the file when the current project
  doesn't (`code_index::project_root_for_file`). Inside one of the tab's locked
  [project root](#tab-project-root) folders (the deepest holding the file), the choice never leaves
  that folder: the outermost directory with Gradle settings between the file and the folder, else
  the nearest git repository there, else the locked folder itself (a plain folder of Kotlin files
  without git or build files works). Elsewhere: the outermost directory with Gradle settings
  inside the file's git repository (anywhere, outside one), else the repository.
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
Tools: `find_class`, `find_symbol` (optional `kind`), `file_outline`, `find_definition`,
`find_usages` (overrides' usages follow under "Via implementations/overrides") and
`find_implementations`. The last three take `path`, a 1-based `line`, and either a 1-based `column` or the
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

The binary ships in `Warpium.app/Contents/MacOS/` and "Install MCP servers for AI agents" registers
it with Claude Code, Codex, Kimi Code or arc (see [MCP servers for coding agents](#mcp-servers-for-coding-agents)).

### Benchmark (phase 0)

[`script/warpium/code_index_bench`](script/warpium/code_index_bench) shallow-clones the corpus
(nowinandroid, Thunderbird, DuckDuckGo, Signal) at pinned commits into the user cache dir and runs
`crates/code_index/examples/bench.rs` in release mode. For each root it reports cold full-index time,
snapshot size, warm start (load, and load plus rescan), p50/p95/max latency of ~100 Go to Class and
Go to Symbol queries (a fixed list plus names sampled from the corpus), Go to Definition latency on
~550 references sampled from the corpus (names after a dot and capitalized names; the share
resolved is low where most references are to libraries, which aren't indexed), Find Usages latency
for 50 sampled classes and functions (with how many came through overrides), Go to Implementation
latency on the same declarations, update time after editing a file, lines of source, and peak RSS. A separate process measures the warm start, so its peak RSS is what the index
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

## Android devices, run and Logcat

Phase A of running Android apps from Warpium: devices, AVDs, running an app module and a native
Logcat pane. The emulator
runs in its own window; embedding it in a pane (and mirroring physical devices) is phase B, see
[docs/warpium/research/embedded-emulator.md](docs/warpium/research/embedded-emulator.md).
Fork-only code lives in `crates/warpium_android` (protoc-free, unit-tested with
`cargo test -p warpium_android` against a scripted fake adb server and binary logcat fixtures),
`app/src/warpium/android.rs` (`AndroidModel`: SDK, device list, selection, AVDs),
`app/src/warpium/logcat.rs` (the pane's view), `app/src/pane_group/pane/warpium_logcat_pane.rs`
and `app/src/workspace/view/warpium_android.rs`. The pane adds `IPaneType::WarpiumLogcat` and
`LeafContents::WarpiumLogcat` (never persisted, like the network log pane).

- **SDK.** `find_sdk`: `<project>/local.properties` `sdk.dir`, `ANDROID_HOME`, `ANDROID_SDK_ROOT`,
  then `~/Library/Android/sdk` (`~/Android/Sdk` on Linux); a directory counts only with
  `platform-tools/adb`. Without one, an `adb` on `PATH`. The project is the project toolbar's.
  Apps started from the Dock don't get shell variables, so the default location matters most.
- **adb.** `AdbClient` speaks the adb server's smart-socket protocol on `127.0.0.1:5037`
  (`ANDROID_ADB_SERVER_PORT` honored): `host:version`, `host:devices-l`, `host:track-devices-l`
  (falling back to `host:track-devices` on old servers), and `host:transport:<serial>` followed by
  `shell:` or `exec:`. No `adb` process is started per request; `adb start-server` runs only when
  nothing listens. Every call blocks, so it runs on dedicated threads (`WorkerHandle` stops them
  when dropped, by shutting their socket down, without blocking the UI thread).
- **Devices.** One tracker thread follows `track-devices-l` and reads each device's properties once
  it is online (`ro.build.version.sdk`/`release`, manufacturer, model, `ro.boot.qemu.avd_name` or
  `ro.kernel.qemu.avd_name`) in a single `shell:` call. Emulators (`emulator-*`), USB and Wi-Fi
  (`host:port` and mDNS `adb-…._adb-tls-connect._tcp`) are told apart by serial; states include
  offline, unauthorized and no permissions. Reconnects back off from 1 to 30 s. The selected device
  is remembered in private preferences (`WarpiumAndroidSelectedDevice`). A remembered device that
  hasn't been connected since launch is replaced by the first online one; one unplugged meanwhile
  stays selected. One-shot requests time out after 10 s of silence. Nothing starts until an Android project opens in the project toolbar, or the Logcat pane, its
  device menu or an Android
  palette command needs devices.
- **AVDs.** Listed from `<name>.ini` and `<name>.avd/config.ini` in `ANDROID_AVD_HOME`,
  `ANDROID_USER_HOME/avd`, `ANDROID_SDK_HOME/.android/avd` and `~/.android/avd` (display name and API
  level), falling back to `emulator -list-avds`. Launching runs `emulator -avd <name>` (its own
  window; it outlives Warpium) and polls the adb server every second until an emulator reports that
  AVD name and `sys.boot_completed=1` (5 minute limit; an early emulator exit is reported).
- **Logcat stream.** `exec:logcat -B -b main -b system -b crash -T 5000` (repeated `-b`, which
  Android 6 and older need): binary `logger_entry` records
  (v1–v4 headers), parsed incrementally on the stream's thread with interned tags, so multi-line
  messages and stack traces stay intact. Batches go to the UI at most every 50 ms (4096 entries at
  most). A stream that ends while its device is online reconnects after 2 s. Reconnecting to the
  same device (a cable blip, a reboot after a crash) keeps what is shown and skips the replayed
  backlog up to the newest shown entry's time; switching devices clears the pane and the package's
  known pids.
- **Buffer and filters.** `LogBuffer` keeps 200,000 entries and the list of rows (one per message
  line) that pass the filters, appended incrementally and rebuilt only when a filter or the
  package's pids change. Filters: minimum level, tags (comma-separated substrings, `-tag` hides),
  text (case-insensitive) or regex over tag and message, and a package. Package pids come from
  ActivityManager's `Start proc` lines in the stream (a new pid rebuilds the rows, since a forked
  process logs a few lines before system_server announces it) and from polling
  `ps -A -o PID,NAME; ps` every 3 s (toybox and the old toolbox format); every pid the package
  ever had stays included, so a crash stays visible after the restart. The package filter starts
  with the project's first app module's id (see `warpium_android::project`; plugins declared with
  `apply false`, as in root build scripts, don't make a module an app). An invalid regex keeps the
  previous text filter while the other filters apply.
- **Rendering.** A `UniformList` lays out only the visible rows; the list follows new rows until
  the user scrolls up or presses the mouse in it (a click or a scrollbar drag), and keeps its place when old rows are evicted. Pause collects new entries
  aside and applies them on resume.
- **Long lines.** Rows scroll horizontally (trackpad swipe, shift-scroll, or the horizontal
  scrollbar), whole rows including the time/pid/level/tag columns. The list is the child of a
  dual-axis `NewScrollable` (vertical: the list's own scroll state through
  `logcat_list::VerticalListAdapter`; horizontal: clipped, like the terminal's wide alt-screen),
  laid out as wide as the widest row. `LogBuffer::max_row_columns` keeps that width up to date as
  rows arrive (evictions don't lower it until the next filter rebuild), so no frame measures the
  buffer. Widths are in monospace columns (tabs 4, CJK and emoji 2). The **Soft wrap** toggle in
  the pane's toolbar (off by default) instead splits message lines wider than the pane into
  several rows (`LogBuffer::set_wrap_columns`, at least 20 columns; a narrower pane still scrolls
  horizontally), keeping rows uniform for the list. Re-wrapping re-splits only the shown entries,
  when the pane's width or the font changes by a column, and keeps the top visible entry in
  view. A stack frame split by soft wrap stays a link on each of its rows.
- **Stack frames.** `at [module/]com.foo.Bar$Inner$1.baz(Bar.kt:42)` lines get a link. Clicking it
  looks up `com.foo.Bar.Inner`, then `com.foo.Bar`, in the code index (Kotlin file facades such as
  `BarKt` are skipped), prefers a matching file with the frame's file name or that name next to it,
  and otherwise walks the project for `Bar.kt` under `com/foo/` (honoring `.gitignore`, skipping
  `build/`), off the UI thread; then opens the file at the line.
- **Device picker and run configurations.** When the project toolbar's project has Android
  application modules (`find_app_modules`: `settings.gradle(.kts)` includes whose build script
  applies the application plugin directly, through a version catalog or a convention plugin, but
  not with `apply false`), found on a background thread when the project changes, the toolbar adds
  a run configuration per module (`RunConfigKind::AndroidApp`, key `android:<gradle path>`) and a
  device chip before the run-configurations chip. Its menu lists devices and AVDs (picking an AVD
  launches it) and **Open Logcat**.
- **Run.** The Run button with an Android run configuration first asks, once per project, to
  trust the project's Gradle build (the same prompt as team toolbar commands; recorded in
  `WarpiumProjectToolbarState` as `trusted_android_builds`), since it runs the repository's
  `gradlew` and build scripts. With no online device but an AVD booting, the run waits and goes to
  whichever device is selected and online first (the booted emulator, or one the user picked
  meanwhile; a booted emulator doesn't take over such a choice). With no device at all, a toast and
  the device menu. Otherwise a background thread reads the package's `lastUpdateTime`
  (`dumpsys package`, retried until the device answers), and then the workspace types
  `./gradlew :<module>:installDebug` (`gradle` without a wrapper) into the active session like a
  toolbar command with `target = "current"` (a busy session falls back to a new tab), with
  `ANDROID_SERIAL` set, so the build stays visible and works in any shell. The thread polls every
  second for up to 10 minutes; when the build rewrites `build/outputs/apk/debug/output-metadata.json`
  its `applicationId` replaces the guessed package (the previous build's metadata, else the build
  script's `applicationId` or `namespace` plus the debug `applicationIdSuffix`). When the install
  time changes, the app is started with `cmd package resolve-activity --brief` + `am start -n`
  (Android 7+), falling back to `monkey -p <pkg> -c android.intent.category.LAUNCHER 1`, and
  `AndroidEvent::AppLaunched` points Logcat at the started package. A failed start shows a toast;
  a build that never installs is given up silently, and the next run replaces the wait. Because
  the install is detected on the device, another install of the same package during the wait
  (after a failed build) also starts the app. Logcat opens without taking focus, filtered to the
  package.
- **Command palette:** "Open Logcat", "Select Android device", "Launch Android Virtual Device (AVD)"
  (both open the pane's device menu) and "Enable/Disable Android tools (devices, logcat, run)",
  which toggles `warpium.android` (default on, OSS channel only).

## Embedded Android emulator (in progress)

Roadmap item 13 phase B: showing a running emulator inside a pane through its built-in gRPC
endpoint. The design and the PR plan are in
[docs/warpium/research/embedded-emulator.md](docs/warpium/research/embedded-emulator.md). Behind
`FeatureFlag::WarpiumEmbeddedEmulator` (dogfood; on in Warpium debug builds, which stand in for a
dogfood channel, see `app/src/bin/oss.rs`) and the `warpium.android` setting. View only so far: no
input, and no launching (PR 3 and PR 4 of the plan).

`crates/warpium_emulator` (protoc-free; `cargo test -p warpium_emulator`) discovers running
emulators from their `pid_<pid>.ini` files, talks to them with `tonic` (frames, input, buttons,
rotation, keepalive) and holds the frame mailbox and pointer mapping. Its tests run against an
in-process fake emulator (`test_support`, also behind the `test-support` feature). The protobuf
code in `src/proto/generated.rs` is committed: after updating the vendored
`proto/emulator_controller.proto`, run `script/warpium/gen-emulator-proto`, which compiles it with
`protox` from a separate generator workspace (`crates/warpium_emulator/codegen`), so neither
`protoc` nor a build script is needed. `session` runs one emulator connection on a
`warpium-emulator-<port>` thread (connect and token check, frame stream, keepalive every 2 minutes
while streaming, restart on resize after 150 ms); `viewer` is the pane's state machine (when to
stream, when to pause). Both are unit-tested against the fake.

**Devices pane** (`app/src/warpium/emulator.rs`, `app/src/pane_group/pane/warpium_devices_pane.rs`,
`app/src/workspace/view/warpium_emulator.rs`; `IPaneType::WarpiumDevices`,
`LeafContents::WarpiumDevices`, never persisted):

- **Opening.** "Show running emulator in Warpium" in the palette, or the same entry in the project
  toolbar's device menu when the selected device is an online emulator. One pane per window, split
  right; each running emulator (from discovery) is a sub-tab. Discovery re-runs when the pane opens,
  on its refresh button and whenever adb's device list changes; an emulator that vanished shows
  "The emulator exited".
- **Frames.** `streamScreenshot` RGBA8888 at the frame area's physical pixel size (the emulator
  scales and never exceeds its display), latest frame only, shown at most 60 times a second. Each
  frame becomes a new `StaticImage` (`StaticImage::from_rgba`, the one upstream addition): the
  renderer's texture cache is keyed by the image's `Arc`, so every frame uploads a new texture and
  the previous one is freed at the end of the next frame. PR 5 replaces this with a stable texture.
- **Pausing.** A tab streams only while it is painted: while streaming, the frame element asks the
  window to repaint every 200 ms, and a tab not painted for 500 ms (another tab or pane tab in
  front, a hidden window) cancels its stream; the next paint resumes it. A visible, static
  emulator therefore costs about five window repaints a second.
- **States.** Connecting, waiting for the screen, screen off, and failures: rejected token, TLS-only
  endpoint (`grpc.server_cert`), unreachable port (with Retry) and exited (with Close tab).
- **Frame stats** at debug level every 5 s while frames arrive:
  `[Devices] emulator-5554 frames received=… dropped=… shown=… painted=… avg_copy_ms=… size=…`
  (`RUST_LOG=warp::warpium::emulator=debug`).
- **Trying it without an emulator** (debug builds only): "[Debug] Start fake Android emulator"
  starts the crate's fake in-process (1080×2400, about 30 synthetic frames a second, listed as
  `emulator-5900`, `-5902`, …) and shows it; "… with a stale token" and "… that requires TLS" show
  the error states, and "[Debug] Stop fake Android emulators" the exited state. The fake's frames
  are a gradient whose blue channel is the frame number.

## MCP servers for coding agents

Warpium ships two stdio MCP servers next to its executable (`Warpium.app/Contents/MacOS/`, built by
`script/macos/bundle` for the `oss` channel and checked by the macOS CI job): `warpium-code-index`
(above) and `warpium-logcat`, which lets an agent read the device logs of the app it is fixing.

### `warpium-logcat`

Crate `crates/warpium_logcat_mcp` (protoc-free; `cargo test -p warpium_logcat_mcp`), reusing
`warpium_android`'s adb client, `LogBuffer` and stack-frame parsing and `code_index` for frames.

```text
warpium-logcat mcp [--root DIR] [--adb PATH]
```

`--root` is the Android project (stack-frame resolution, SDK lookup through `local.properties`);
without it the current directory is used when it is a Gradle build. `ANDROID_SERIAL` picks the
default device, `ANDROID_ADB_SERVER_PORT` the adb server.

- **Capture.** Started lazily per device on the first call that needs it: a background logcat
  stream into a 100k-entry in-memory ring (`LogBuffer`), seeded with the device's recent backlog,
  so `since` queries work. Reconnects don't repeat the replayed backlog. Package scoping tracks
  which process owned each pid over which stretch of the stream (ActivityManager start/death
  lines, `ps` for an app already running), so it follows restarts and leaves out a reused pid.
  A call runs to completion before the next request is read, so `wait_for_log` holds the server
  for up to its timeout.
- **Tools.** `list_devices` (serial, kind, state, model, API); `get_logs` (device, package,
  `min_level`, `tag` with `-Tag` excludes, `text` or `regex`, `since` as `2m`/`90s`/a timestamp/
  `mark`, `limit` default 200, at most 2000; newest last, with a `truncated` flag); `get_crashes`
  (FATAL EXCEPTION, ANR and native crash blocks with their stack traces, project frames resolved to
  `path:line` through the code index); `wait_for_log` (blocks up to `timeout_s`, at most 120 s,
  for a matching line that arrives after the call, or since `since`; returns it with context);
  `clear_logs`, which only sets a mark (`since: "mark"` and the default of later calls), never
  `logcat -c`.
- **Privacy.** Log queries need `package` unless `all_packages: true` is passed; messages and
  output are capped (8 KiB per line, 200 KiB per result); nothing is written to disk.

### Warp's agent: `warpium.logcat_mcp`

Setting `warpium.logcat_mcp` (default off, OSS channel only; Settings → Features → Warpium, or
"Enable/Disable Logcat MCP server for Warp's agent" in the palette). While on, `app/src/warpium/agent_mcp.rs`
keeps a `warpium-logcat` entry in Warp's global MCP config (`~/.warp-oss/.mcp.json`) pointing at
the bundled binary with `--root` of the project toolbar's project at the time it was toggled
(kept when no project is open); turning it off removes the entry. Other servers in the file are
never touched.

### Installing into other agents

"Install MCP servers for AI agents (Logcat, code index)" (palette, or **Install** next to the
setting) writes both servers, user-triggered only; "Uninstall …" removes them. The logic is
generic over a list of `{name, command, args}` servers in `crates/warpium_agent_mcp`
(`cargo test -p warpium_agent_mcp`):

- **arc project** (the project root has `.arc/`): only `<root>/.arc/private/mcp.json`
  (`{"mcpServers": …}`), arc's git-ignored private layer, where machine-specific absolute paths
  belong (arc warns about literal values in committed layers). arc reads it when it starts an
  agent, so agents launched through `arc` pick the servers up on their next run; harness folders
  (`.claude/`, `.kimi-code/`, Codex config) are left to arc.
- **Otherwise**, each agent found (CLI on `PATH` or the usual install dirs, or its config dir):
  Claude Code in the project's local scope (`projects.<root>.mcpServers` in `~/.claude.json`, via
  `claude mcp add/remove --scope local` when the CLI is found), user scope without a project;
  Codex `[mcp_servers.<name>]` in `$CODEX_HOME/config.toml` (default `~/.codex`, edited with
  `toml_edit` so comments stay); Kimi Code `mcpServers` in `$KIMI_CODE_HOME/mcp.json` (default
  `~/.kimi-code`, legacy `~/.kimi` when only that exists). Codex and Kimi configs are shared by all
  projects, so their entries have no `--root`; the servers then work on the directory the agent
  runs in.
- Writes are idempotent (unchanged entries aren't rewritten), keep foreign entries and the file's
  permissions, go through a temp file and rename, and leave unreadable configs alone with an
  error. A toast lists where the servers were installed or removed.

Manual route (e.g. a dev build without bundled binaries):

```bash
cargo build --release -p warpium_logcat_mcp -p code_index
claude mcp add --scope local warpium-logcat -- "$PWD/target/release/warpium-logcat" mcp --root /path/to/project
```

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
  the macOS job builds on `macos-15-intel` and cross-compiles arm64 with `--selfsign`.
  It checks the plist version and the app icon in the built bundle.
- **Profiles:** pull requests build a **debug** app (`script/bundle --debug`, Cargo `dev`
  profile, no debuginfo): much faster than release, good enough for functional checks in the
  test VM, but not for performance measurements, and `debug_assertions` are on (overflow
  checks and `debug_assert!` can panic where a release build wouldn't). Master pushes build
  **release** (ThinLTO, about 1.5 h) and **debug** in parallel; tags, releases and manual runs
  build release only (a manual run can pick debug). Each profile has its own Rust cache, saved
  only by master runs, so PR builds restore master's warm debug cache. Measure performance on
  a master or release build.
- **Dogfood features:** branch and PR builds add the cargo features in the bundle step's
  `DOGFOOD_FEATURES` (none right now) so features in development can be tested from the
  artifact; release builds leave them out.
- **Unit tests:** `.github/workflows/warpium-tests.yml` ("Warpium Tests") runs on
  `ubuntu-latest` for the same triggers, in parallel with the macOS build: all tests of the
  fork crates (`crates/warpium_*`, `crates/code_index`) plus the `warp` crate's tests whose path
  contains `warpium` (`cargo nextest run -p warp --lib -E 'test(/warpium/)'`). Name fork test modules
  so they match (e.g. `warpium_*` files, `app/src/warpium/**`). Upstream's suite isn't run.
- **Cross-compile caveat:** the Intel runner can't run the arm64 binary that generates
  `settings_schema.json`, so CI builds lack the bundled `modify-settings` agent skill. Local
  builds on Apple silicon include it.
- **Artifacts:** `Warpium-<version>-<sha8>-macos-arm64` for branch builds,
  `Warpium-<version>-macos-arm64` for tags, with `-debug` appended for debug builds (so PR
  artifacts end in `-debug`); each contains `Warpium-<version>-arm64.dmg`. The mac-mcp
  `install_artifact` tool picks the release artifact of a master run unless told otherwise.
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
