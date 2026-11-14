# Importing configuration from Warp

Warpium is Warp's `oss` channel under its own bundle id, so it keeps its configuration apart from
an installed official Warp app. This page maps where each Warp channel keeps user configuration
on macOS, and what Warpium's import (`crates/warp_config_import`, `app/src/warpium/`) does with
each category.

## Where each channel keeps its data (macOS)

| | Stable | Preview | Dev | Warpium (`oss`) |
| --- | --- | --- | --- | --- |
| Bundle id / user defaults domain | `dev.warp.Warp-Stable` | `dev.warp.Warp-Preview` | `dev.warp.Warp-Dev` | `dev.milka.Warpium` |
| `data_dir()` = `config_local_dir()` | `~/.warp` | `~/.warp-preview` | `~/.warp-dev` | `~/.warp-oss` |
| `warp_home_config_dir()` (skills, `.mcp.json`) | `~/.warp` | `~/.warp` | `~/.warp-dev` | `~/.warp-oss` |
| Application Support / cache | `~/Library/Application Support/dev.warp.Warp-Stable` | `…/dev.warp.Warp-Preview` | `…/dev.warp.Warp-Dev` | `…/dev.milka.Warpium` |
| SQLite (`secure_state_dir()`) | `~/Library/Group Containers/2BBY89MBSN.dev.warp/Library/Application Support/dev.warp.Warp-Stable/warp.sqlite` | same group, `…Warp-Preview` | same group, `…Warp-Dev` | no app group (self-signed): `~/Library/Application Support/dev.milka.Warpium/warp.sqlite` |
| Keychain service | `dev.warp.Warp-Stable` | `dev.warp.Warp-Preview` | `dev.warp.Warp-Dev` | `dev.milka.Warpium` |

Sources: `crates/warp_core/src/paths.rs` (`macos_config_dir_name_for`, `base_warp_config_dir_name`,
`secure_state_dir`), `ChannelState::data_domain()`, `app/Cargo.toml` bundle metadata,
`LaunchMode::secure_storage_service_name`. Preview's `~/.warp-preview` is populated once with
symlinks into `~/.warp` (`app/src/preview_config_migration.rs`), except `settings.toml`.

Settings come in two stores (`app/src/settings/init.rs`):

- **Public settings** live in `settings.toml` (the `SettingsFile` feature, on in every current
  build) under their `toml_path`, and hot-reload through the `WarpConfig` file watcher. Older
  builds kept them in user defaults; the first launch of a `settings.toml` build migrates them.
- **Private settings** (`private: true`) always live in the channel's user defaults domain under
  their `storage_key`, as JSON strings. They are mostly UI state (dismissed banners, counters) plus
  a few real preferences such as workflow aliases.
- **Secrets** (auth tokens, BYO API keys) are in the keychain, never in either store.
- **Cloud-synced settings** (`sync_to_cloud`) also round-trip through Warp Drive when signed in;
  the local copy is still the file or user defaults entry above.

Because Warpium is built from the same code, every setting has the same `toml_path` and
`storage_key` in both apps; no key needs translating. Only path values are rewritten (below).

## Categories

| Category | Warp location | Warpium location | Format | Import | Why |
| --- | --- | --- | --- | --- | --- |
| Public settings | `<data_dir>/settings.toml` | `~/.warp-oss/settings.toml` | TOML | Imported and synced, per setting | Merged one setting at a time; see below |
| Private settings | user defaults `dev.warp.Warp-*` | user defaults `dev.milka.Warpium` | JSON strings | Imported and synced, per key, except a denylist | Read through `UserDefaultsPreferencesStorage` with Warp's domain as the suite, written through `SettingsManager` |
| Agent / execution profiles | inside `settings.toml` (`agents.profiles…`) | same | TOML | Synced with public settings | File-backed profiles are a settings collection |
| Themes | `<data_dir>/themes/` | `~/.warp-oss/themes/` | YAML + images | Synced | Theme paths in settings are stored relative to this dir |
| Workflows | `<data_dir>/workflows/` | `~/.warp-oss/workflows/` | YAML | Synced | |
| Launch configurations | `<data_dir>/launch_configurations/` | `~/.warp-oss/launch_configurations/` | YAML | Synced | |
| Tab configs | `<data_dir>/tab_configs/`, `default_tab_configs/` | same names under `~/.warp-oss` | TOML | Synced | |
| Custom model routers | `<data_dir>/custom_model_routers/` | `~/.warp-oss/custom_model_routers/` | YAML | Synced | No credentials in the format |
| Keybindings | `<data_dir>/keybindings.yaml` | `~/.warp-oss/keybindings.yaml` | YAML | Synced | |
| Skills | `<home_config_dir>/skills/` | `~/.warp-oss/skills/` | Markdown + scripts | Synced | `.git` and `node_modules` are skipped; file modes are kept |
| Global MCP config | `<home_config_dir>/.mcp.json` | `~/.warp-oss/.mcp.json` | JSON | Synced only if it has no inline credentials | Skipped when any server has `env`/`headers` values or token/secret/password/API-key fields |
| Launch at login | `general.login_item`, `AppAddedAsLoginItem` | | | Skipped | Both apps would register as login items |
| Account / machine state | private keys such as `DidNonAnonymousUserLogIn`, `AIRequestQuotaInfoSetting`, `LastSelectedEnvironmentId`, `LastSelectedAuthSecret`, `IapRoutingEnabled`, `UserAppInstallationDetected`, debug switches | | | Skipped | Tied to the account or the Warp build |
| LSP servers, worktrees | `<data_dir>/pyright`, `rust-analyzer`, …, `worktrees/` | | binaries, git worktrees | Skipped | Generated, large, re-created on demand |
| IAP token cache | `<home_config_dir>/staging/iap_cache.jwt` | | JWT | Skipped | Secret |
| Keychain | service `dev.warp.Warp-*` | | | Skipped | Auth tokens and API keys; only if asked later |
| SQLite: Warp Drive cache, notebooks, env var collections, AI conversations | `warp.sqlite` | | SQLite | Skipped (follow-up) | Mostly a cache of account-owned cloud objects; sign in to Warp Drive in Warpium instead |
| SQLite: command history, window/session restore | `warp.sqlite` | | SQLite | Skipped (follow-up) | Stable's schema is newer than Warpium's; writing into a live, differently migrated DB isn't safe |
| Warp Drive cloud data | server | server | | Out of scope | Needs sign-in; arrives when signed in to the same account |
| TUI settings | `~/.warp_cli*` | `~/.warp_cli-oss` | TOML | Skipped | Not used by Warpium |

## How the sync works

On every GUI launch while `warpium.sync_from_warp` is `true` (the default), and whenever
**Import settings from Warp now** runs from the command palette:

1. The first import picks the most recently used installation among Stable, Preview and Dev,
   judged by the modification time of its user defaults plist, `settings.toml` and data
   directory, and logs the choice. Later runs stay with that installation (recorded in the
   manifest) so Warpium doesn't flip between channels; if it is uninstalled, the most recent
   remaining one is imported as a fresh first import. To switch on purpose, delete the manifest.
2. Off the main thread, each file category is synced, and the `settings.toml` merge is computed.
3. On the main thread, the merged `settings.toml` is written (only if Warpium's file did not
   change in between; otherwise the merge is recomputed) and immediately reloaded into the
   settings models. Private settings are then synced through `SettingsManager`, so they apply
   without a restart. Keybindings are reloaded when `keybindings.yaml` changed; themes,
   workflows, launch and tab configs, skills and MCP config are picked up by the existing file
   watchers.
4. The manifest `~/.warp-oss/.warp-import-manifest.json` records a content hash of everything
   imported, as the base for the next run.

Each item (file, `settings.toml` setting, private key) follows a 3-way rule against the base:

| Warp since last import | Warpium since last import | Result |
| --- | --- | --- |
| unchanged | unchanged or changed | keep Warpium's |
| changed | unchanged | take Warp's |
| changed | changed differently | keep Warpium's, report a conflict |
| new | absent | take Warp's |
| new | already has a different one | conflict (on the very first import: take Warp's) |
| removed | unchanged | setting: reset in Warpium; file: kept |
| absent | Warpium-only | never touched |

The very first import (no manifest yet) lets Warp's version win over anything Warpium already
had, so a fresh Warpium matches Warp. A corrupt manifest does not count as a first import.

`settings.toml` is merged per setting: every registered setting path is one unit even when its
value is a table, so struct- and enum-shaped settings never mix fields from both apps. Settings
Warpium doesn't know (from a newer Warp) are merged leaf by leaf, so they flow over too.
Comparisons ignore formatting and comments, and Warpium's own formatting and comments are kept.
String values that point into Warp's directories (`/Users/me/.warp/…`, `~/.warp/…`,
`~/.warp-preview/…`) are rewritten to `~/.warp-oss/…`.

Safety:

- Every Warpium file that gets overwritten, `settings.toml` included, is first copied to
  `~/.warp-oss/.import-backup/<YYYYmmdd-HHMMSS>/`.
- All writes are atomic (temporary file and rename).
- A Warpium `settings.toml` with a syntax error is never touched.
- `WARPIUM_IMPORT_DRY_RUN=1` logs what would change without writing anything.
- Files over 2 MB and categories over 2,000 files are skipped; symlinked directories are
  followed, but each real directory only once.

On launch a toast appears only when something was imported or updated; the command palette
action always shows one. Linux and Windows are not supported (Warp's layout differs there).

Logs use the `[Warp import]` prefix: the summary at info, item names only in dogfood logs or at
debug level.

## Feature flag

There is no `FeatureFlag`: the user-facing setting `warpium.sync_from_warp` is the switch, and a
flag would only add an upstream-file edit. The command palette offers
"Enable/Disable syncing settings from Warp on launch" and "Import settings from Warp now".

## Follow-ups

- Command history and other local SQLite data, once a schema-compatible path exists (for example
  reading Stable's DB read-only and inserting into Warpium's through its persistence layer).
- Optional import of keychain items (BYO API keys) on explicit request.
- A Settings page widget for the toggle.
- Skip re-hashing unchanged files by recording size and mtime in the manifest (each launch
  currently reads and hashes every file of every category).
