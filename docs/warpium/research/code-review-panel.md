# Code Review panel → JetBrains Commit tool window (roadmap item 23)

Research and design, 2026-12-08. Branch `master` at base `3055f5acd3c`. No product
code was written; all evidence is `file:line` against that base. IntelliJ IDEA's
Commit tool window and Git tool window are the behavior reference.

Covers the owner's list: group by directory / repository, expand all / collapse
all, collapsed-by-default file list, unversioned files, rollback of selected
files, stash tab, amend commit, and open performance.

---

## 1. TL;DR

1. **Why the panel is slow to open** (ranked, measured — details in §4):
   - **R1. The full diff load is `2N+2` sequential `git` process spawns** for N
     changed files (`diff_state/local.rs:1794-1820`): one `git diff` **and** one
     `git show` per file, awaited one after another. Measured on this repo
     (36.7k worktree files): 100 files ≈ **1.44 s** sequential vs **0.53 s** at
     8-way parallelism, and the diff half collapses to a single `git diff` call.
   - **R2. Opening recomputes `git status` three to four times**, each a full
     worktree walk: the diff load (`local.rs:1771`), the diff-state metadata
     refresh (`local.rs:2889`), the git-chip status model (`git_repo_model/local.rs:241-258`),
     and again inside `diff_metadata_against_specific_branch`
     (`local.rs:2159`). Measured `git status --porcelain=2` on this repo:
     **~480 ms** per call in the dev container (bind mount; ~20-40 ms on a local
     disk — the multiplication, not the constant, is the bug).
   - **R3. `on_open` unconditionally triggers a full reload** of everything
     (`code_review_view.rs:761-770`): diffs, metadata, branches dropdown, plus
     the LSP footer — no "nothing changed since last load" fast path.
   - **R4. When diffs arrive, the view builds a full editor + 6 button views per
     file on the main thread** (`code_review_view.rs:2583-2721`), whether or not
     the file is ever looked at (~800 view constructions for 100 files in one
     event), and every file's HEAD content is fetched and kept in memory.
   - **R5. `detect_main_branch` fires up to 7 sequential `git` spawns** and runs
     in three separate loads on open (`util/git.rs:92-130`).
   - Fixes: one batched `git diff` (+ `cat-file --batch` or bounded-parallel
     base fetch), lazy per-file content/diff loading keyed off the
     collapsed-by-default list, one shared status snapshot per refresh round, a
     dirty-flag skip on open, and editor/view creation on first expand. §4.9.
2. **Collapsed by default is a one-line semantic flip** plus lazy loading:
   `RepositoryState::should_auto_expand_file` (`code_review_view.rs:565-579`)
   already defaults to expanded; the per-file override map and chevron toggle
   exist. JB: changes list, click file → diff. Mapping: single click on a file
   row expands the inline diff (§5.3).
3. **Untracked files already flow end-to-end** — `git status --porcelain=2`
   `?` entries are parsed (`local.rs:2328-2337`), included in Head-mode diffs
   (`local.rs:1788`), and watcher events for them arrive as `added`
   (`repo_metadata/src/watcher.rs:564-566`). What is missing is only the JB
   presentation: an "Unversioned Files" group node at the bottom (§5.4).
4. **Rollback of selected files is mostly built.** The discard dialog already
   has multi-select checkboxes (`DiscardDialogState.selected_files`,
   `code_review_view.rs:504-516, 5440-5447`), batched restore for non-renamed
   paths (`git restore --staged --worktree -- p1 p2 …`, `local.rs:634-649`), and
   #42's `warpium_git::rollback` plumbing (`crates/warpium_git/src/rollback.rs`).
   The work is: row multi-select in the list, a "Rollback…" toolbar action (JB
   name), a batch entry point in `warpium_git`, and rename/untracked semantics
   (§5.5).
5. **No stash UI exists anywhere**; only `git stash push -u -m … -- <paths>` as
   a side option of the discard dialog (`local.rs:583-624`). The stash tab is
   new crate code (`stash list/create/apply/pop/drop/diff` in `warpium_git`)
   plus a panel sub-tab (§5.6).
6. **Amend is a small commit-dialog change**: an "Amend" checkbox prefilled
   from `git log -1 --format=%B`, threaded through `run_commit` as
   `git commit --amend` (`util/git.rs:665-693`) (§5.7).
7. **Feature flag: none.** Ship unflagged like the git tools (#42); the panel is
   OSS-only fork surface, and the one behavioral change (collapsed default) is
   exactly what was requested. The escape hatch is a persisted "Group by"
   setting, not a flag (§6).
8. **One implementation PR** with ordered commits and mid-PR checkpoints (§8);
   perf work lands first so the rest of the PR rides on the fast path.

---

## 2. Current-state map

### 2.1 How the panel opens (the full path)

```
/open-code-review (slash command)            app/src/search/slash_command_menu/static_commands/commands.rs:394-404
  → SlashCommandKind::OpenCodeReview         app/src/terminal/input/slash_commands/mod.rs:906-910
  → TerminalAction::ToggleCodeReviewPane     app/src/terminal/input.rs
  → pane_group::Event::OpenCodeReviewPane    app/src/pane_group/mod.rs:578
  → Workspace::open_code_review_panel_from_arg  app/src/workspace/view.rs:9863-9917
      → WorkingDirectoriesModel::get_or_create_diff_state_model  app/src/pane_group/working_directories.rs:391-428
          (creates LocalDiffStateModel → repo detection → set_active_repository)
      → Workspace::open_right_panel          app/src/workspace/view.rs
  → RightPanelView::open_code_review         app/src/workspace/view/right_panel.rs:700-740
      → cached view? WorkingDirectoriesModel::get_code_review_view  working_directories.rs:494
      → else create_code_review_view         right_panel.rs:1194-1244
          → CodeReviewView::new              app/src/code_review/code_review_view.rs:1115-1365
      → CodeReviewView::on_open              code_review_view.rs:713-771
  (Also: "Toggle code review" keybinding → WorkspaceAction::ToggleRightPanel → toggle_right_panel,
   workspace/mod.rs:798-800, app/src/workspace/view.rs:25656-25659)
```

`on_open` (`code_review_view.rs:713-771`) then, in order:

1. subscribes to `DiffStateModel` events (`:719`);
2. subscribes to the per-repo git status model (`:720`) — creating it if absent,
   which kicks **its own metadata load** (`git_repo_model/local.rs:50-51`);
3. subscribes to the GitHub repo model (`:721`);
4. `fetch_branches_and_setup_dropdown` (`:723`) — another git call chain;
5. creates the LSP `CodeFooterView` for the repo (`:734-737`);
6. `set_code_review_metadata_refresh_enabled(true)` → immediate metadata
   refresh (`diff_state/local.rs:891-900`, `905-933`);
7. `load_diffs_for_current_repo(false, true, …)` — **the big one** (`:767-770`).

The view is cached per (pane group, repo) in `WorkingDirectoriesModel`
(`working_directories.rs:296`), so reopening the panel reuses it and re-runs
steps 1-7 every time.

### 2.2 The diff load (the expensive part)

`load_diffs_for_current_repo` (`diff_state/local.rs:536-569`) aborts any
in-flight load and spawns `load_diffs_for_repo` (`:1608-1627`), which for Head
mode runs `diff_state_against_head` (`:1787-1828`):

1. `file_statuses_against_head` (`:1766-1785`): one
   `git status --untracked-files=all --branch --porcelain=2 -z`.
2. `get_binary_files` (`:2356-2358`, `:2849`): one `git diff --numstat HEAD`.
3. **A sequential `for` loop over every changed file** (`:1798-1820`), each
   iteration awaiting:
   - `get_file_diff` (`:2400-2563`): one `git diff --patch-with-raw -z --no-color
     HEAD -- <file>` (untracked: `git diff --no-index /dev/null <file>`), then
     patch parsing;
   - `get_file_content_at_head` (`:2361-2395`): one `git show HEAD:<file>`
     fetching the **entire base blob**;
   - `is_file_autogenerated` (`code_review/mod.rs:115-165`) on the content.

Result: `GitDiffWithBaseContent` — hunks **and** full base text for every file,
held in memory for the life of the state.

Base-branch modes are the same shape (`diff_state_against_specific_branch`,
`:2058-2127`) after `git merge-base` (`:1483-1487`), plus a second status run
for untracked files (`:2039-2043`).

### 2.3 Metadata loads that duplicate the status walk

On open, three independent loads each re-derive repo facts:

| Load | Trigger | Git calls (sequential within the load) |
| --- | --- | --- |
| Diff metadata | `refresh_diff_metadata_for_current_repo` (`local.rs:905-933`) via `load_metadata_for_repo` (`:1531-1582`) | `detect_main_branch` (1-7), `detect_current_branch` (1-2), `rev-parse HEAD`, `diff_metadata_against_head` (status + numstat, `:2885-2904`), `diff_metadata_against_specific_branch` (merge-base + name-status + status + numstat, `:2130-2180`), `rev-parse @{u}` + unpushed `rev-list` (`:1551-1571`) |
| Git-chip status | `LocalGitRepoStatusModel::new`/`refresh_metadata` (`git_repo_model/local.rs:50-51`, `119-130`, `241-258`) | `detect_main_branch`, `detect_current_branch_display`, `diff_metadata_against_head`, `branch_tracking_status` (2 calls, `:180-234`) |
| Branches dropdown | `fetch_branches_and_setup_dropdown` (`code_review_view.rs:723`) → `get_all_branches_with_known_main` (`util/git.rs:1122`) | `detect_main_branch` again + branch listing |

All of this is async (off the main thread — good), but every call is a separate
`git` process (`warp_util::git::run_git_command`, `crates/warp_util/src/git.rs:8-55`),
each paying spawn + config + index costs, and the chains are sequential.

### 2.4 Watcher-driven updates

- `LocalGitRepoStatusModel` subscribes to `repo_metadata` watcher events and
  throttles metadata refresh to one per 5 s (`git_repo_model/local.rs:79-100`);
  `should_refresh_metadata` (`:152-170`) filters ignored files, and index-lock
  events are suppressed while the lock exists (`:316-333`).
- `LocalDiffStateModel.set_active_repository` (`diff_state/local.rs:944-1024`)
  registers its own watcher (a second watch on the same repo) and its own 5 s
  throttle (`:1015-1023`). `handle_file_update` (`:1031-1118`) maps events to a
  full reload on `commit_updated`, a per-file invalidation queue
  (`enqueue_file_invalidations`, `:1123-1157`, a `SyncQueue`) for file edits,
  and a full reload on `.gitignore` changes. Per-file invalidation itself runs
  up to 3 git calls per file (`retrieve_diff_state`, `:1932-1961`).
- While the panel is closed, diff loads are suppressed
  (`metadata_refresh_enabled`, `:1055-1057`); metadata still refreshes at 5 s
  for the chips.

### 2.5 View construction when diffs arrive

`NewDiffsComputed` → `invalidate_all` (`code_review_view.rs:2467-2580`):

- drops all state and **recreates the viewported list state** (`:2537`);
- `build_view_state_for_file_diffs` (`:2583-2721`) iterates every file and, on
  the main thread, per file:
  - creates a `LocalCodeEditorView` + inner `CodeEditorView` with global buffer
    (`create_code_review_model_with_global_buffer`, `:2997-3096`) and two
    subscriptions;
  - creates **six** `ActionButton` views (chevron, open, discard, context, copy,
    `:2630-2700`);
  - runs `apply_diff_to_code_editor` (`:3415-3497`): `set_base` → the editor
    diff machinery computes hunks off the main thread (`similar`), but buffer
    wiring still happens here.

Expansion today: `should_auto_expand_file` (`:565-579`) returns true unless the
file is binary, unrenderable, or autogenerated — **the panel opens with every
diff expanded**, and even collapsed files already own their editor.

Rendering is viewported (`List::new(self.viewported_list_state)`,
`:4506-4511`; per-item `render_diff_at_index`, `:2723-2740`), so only visible
rows build elements — but the *view handles* above exist for all files.

### 2.6 Existing pieces the design reuses

- **Per-file Discard with confirmation + optional stash-before-discard**:
  dialog `code_review_view.rs:5459-5574` (multi-file checkboxes already
  rendered, `:5440-5447`), batched git in `discard_files_impl`
  (`diff_state/local.rs:733-838`), reload after success (`:865-869`).
- **Rollback plumbing from #42**: `warpium_git::rollback` —
  `file_state`/`rollback_args`/`rollback` (`crates/warpium_git/src/rollback.rs:19-82`),
  blocking git runner with `GIT_OPTIONAL_LOCKS=0` (`crates/warpium_git/src/process.rs:90-115`).
- **JB-style directory tree model**: `warpium_git::tree::file_tree`
  (`crates/warpium_git/src/tree.rs:42-77`) — directories first, single-child
  chains joined (`src/main/kotlin`), file counts; proven in the Git Log pane.
- **Git dialog framework**: `git_dialog/` (commit/push/PR modes, loading,
  toasts, telemetry); `run_commit` (`util/git.rs:665-693`) = `git add -A` +
  staged check + `git commit -m`.
- **Commit chain orchestration**: `git_commit_chain` (`diff_state/local.rs:1274`)
  → `run_commit_chain` (`git_actions.rs:30-53`).
- **Multi-repo status cache**: `GitRepoModels` (`git_repo_models.rs:22-91`),
  one status model per repo keyed by `LocalOrRemotePath`.
- **Panel header/toolbar slot**: `RightPanelView::render_header`
  (`right_panel.rs:951-1036`) — where Group By / Expand/Collapse / Rollback
  toolbar actions go.
- **Diff viewer for non-working-tree comparisons**: the Git Log pane's unified
  diff panel (`app/src/warpium/git_log_panels.rs`) — the pattern for viewing a
  stash's diff.
- **Settings/persistence patterns**: `warpium.*` settings group (e.g.
  `warpium.bottom_panel` in `sync_settings.rs`); per-pane-group in-memory
  selection `working_directories.rs:303` (not persisted); right-panel open
  state is **not** persisted (`pane_group/mod.rs:946-948`).

---

## 3. JetBrains reference behavior (what we copy)

From the IntelliJ Commit tool window (IDEA 2024+) and Git tool window:

1. **Changes list**: flat file list by default; **Group By → Directory**
   groups under collapsible directory nodes; **Group By → Repository** groups
   under repo nodes in multi-root projects. Toolbar: **Expand All**, **Collapse
   All**.
2. **Unversioned Files**: a separate collapsible node at the bottom listing
   untracked files (not mixed into changes); double-click opens the file;
   rollback there deletes the file (with confirmation).
3. **Rollback…**: action on selected files (Ctrl/Cmd multi-select in the
   list), confirmation dialog listing the files, then `git checkout` of exactly
   those paths; list refreshes after.
4. **Amend**: an "Amend" checkbox in the commit area; checking it prefills the
   message with the previous commit's message (editable); commit runs
   `git commit --amend`.
5. **Stashes** (Git tool window → Stashes node): lists `stash@{n}` entries with
   their messages; context menu / buttons: Apply Stash, Pop Stash, Drop Stash,
   Create Stash (via the toolbar); double-clicking a stash shows its diff in the
   editor.
6. **Speed**: the tool window opens instantly and populates incrementally; the
   diff of a file is computed when the file is opened, not for the whole list.

---

## 4. Performance: why open is slow, ranked

Measured 2026-12-08 in the dev container (bind mount — WARPIUM.md notes walks
here are ~40× a local disk; treat absolute numbers as upper bounds, the
*ratios* and *counts* as the evidence). Corpus: this repository (128k objects,
36.7k worktree files, 832 KB index) and a synthetic 100-file change set.

| # | Cause | Evidence | Measured | Fix |
| --- | --- | --- | --- | --- |
| R1 | `2N+2` sequential git spawns per diff load | `diff_state/local.rs:1794-1820` (loop), `:2400` (`get_file_diff`), `:2361` (`get_file_content_at_head`) | 100 files ≈ 1.44 s sequential; 0.53 s at 8-way; single batched `git diff HEAD` = 4 ms + one parse | §4.9 F1+F2 |
| R2 | `git status` recomputed 3-4× per open, full worktree walk each | `:1771`, `:2889`, `git_repo_model/local.rs:241-258`, `:2159` | `git status --porcelain=2` ≈ 480 ms each on this repo; `--untracked-files=no` ≈ 16 ms | §4.9 F3 |
| R3 | Unconditional full reload on every open | `code_review_view.rs:761-770` | telemetry `DiffLoadCompleted.load_duration` exists but the load itself is the cost; no dirty check | §4.9 F4 |
| R4 | Editor + 6 button views built for every file on the main thread; all base blobs fetched and resident | `code_review_view.rs:2583-2721`, `:2997-3096` | ~800 view constructions + 100 `git show` blob fetches for 100 files regardless of visibility | §4.9 F2+F5 |
| R5 | `detect_main_branch` = up to 7 sequential spawns, run by 3 loads | `util/git.rs:92-130` | 3× on open | §4.9 F3 |
| R6 | Watcher storms re-run the whole metadata chain every 5 s while dirty | `git_repo_model/local.rs:93-100`, `diff_state/local.rs:1015-1023` | each fire = ~10 spawns; commits trigger full reloads (`:1059-1065`) | §4.9 F4 |

Root theme: **the panel precomputes everything for the worst case** (all
files, full patches, full base texts, all editors) in a fresh process per file,
and rediscovers repo facts it already knew. JB does the opposite: list first,
diff on demand.

### 4.9 Proposed fixes (concrete)

- **F1 — one batched diff.** Replace the per-file `git diff` loop with a single
  `git diff --no-ext-diff --patch-with-raw -z --no-color HEAD` (Head mode;
  equivalent for merge-base mode), parsed into per-file hunks with the existing
  patch parser. This is exactly what GitHub Desktop does (the code already
  cites matching its arguments, `local.rs:2398`). Binary detection comes free
  from the same output's raw lines / existing numstat. Untracked files: keep
  them out of the batch; compute their display stats from disk (line counts,
  `num_lines_in_file_if_non_binary` already exists, `:1719-1764`) and generate
  the all-additions diff lazily on expand.
- **F2 — lazy base content and lazy editors.** `FileDiffAndContent.content_at_head`
  becomes `Option<OnceCell<...>>`-style: fetched on first expand/visibility via
  a bounded-parallel `git cat-file --batch` (one process, streaming blobs) or
  `git show` calls capped at ~8 concurrent. Editor + button views are created
  on first expand (the viewported list already re-renders rows on invalidation;
  expansion state changes are followed by `ctx.notify()`), not in
  `build_view_state_for_file_diffs`. Hunk data for the collapsed row (add/del
  counts) already comes from numstat in metadata.
- **F3 — one status snapshot + cached repo facts per refresh round.** The diff
  load and the metadata refresh share a single `git status` invocation result;
  `detect_main_branch` is memoized per repo (invalidate on
  `remote_ref_updated` / branch change events) — the three consumers pass the
  same cached value.
- **F4 — dirty-flag open path + coalesced watcher reloads.** Track a repo
  dirty flag set by watcher events (both models already receive them). If clean
  since the last successful load, `on_open` emits the cached state and only
  kicks a cheap metadata refresh; if dirty, reload as today. Debounce the
  `commit_updated` full reload through the existing 5 s throttle instead of
  reloading mid-operation.
- **F5 — keep the viewported list; make rows cheap.** Collapsed rows render
  plain elements (icon, name, dir, stats chips); no `ChildView` until expanded.

Expected result: open cost becomes ~2 git calls on a clean reopen (status +
numstat, or zero UI-blocking work when cached), and a cold load with N files
drops from `2N+2` spawns to ~3-4 spawns + bounded-parallel blob fetches that
only touch expanded files.

**Measurement plan** (for the implementation PR): before/after on a release
build in the mac-mcp VM, using (a) `DiffLoadCompleted.load_duration`
telemetry, (b) `RUST_LOG` `[GIT OPERATION]` traces (every call is already
logged, `warp_util/src/git.rs:23-26`), (c) a synthetic repo fixture with 500
changed files (script like the one used for these measurements), and (d)
first-paint time from palette action to populated list (screen recording in the
VM).

---

## 5. Design per feature

### 5.1 Group by directory / repository (JB: Group By → Directory / Repository)

**JB behavior.** Toolbar "Group By" toggle: flat list, or directory tree
(collapsible nodes, files nested), or repository nodes in multi-root projects.
Persisted per IDE, not per project.

**Warpium mapping.**

- A `GroupBy` enum `{ Flat, Directory, Repository }` in the view (per repo,
  persisted globally in settings as `warpium.code_review_group_by`, following
  the `warpium.*` settings pattern; default `Flat`).
- Toolbar: a Group By dropdown button in the panel header
  (`right_panel.rs:951-1036` slot), with Flat / Directory / Repository, plus
  "Expand All"/"Collapse All" next to it (§5.2).
- `Directory` reuses `warpium_git::tree::file_tree`
  (`crates/warpium_git/src/tree.rs:42`) for the row model: directories first,
  single-child chains joined, per-file counts — the exact JB shape already
  proven in the Git Log pane. Directory node click toggles the node; state kept
  in-memory per view (JB does not persist node expansion either).
- `Repository` groups under one repo node (display name + path); the panel is
  single-repo today (repo dropdown, `right_panel.rs:785-801`), so this is the
  structural root for a future multi-repo aggregation — the grouping code is
  keyed by repo so a "show all repos of the pane group" mode can add nodes
  without rework. Out of scope: true multi-repo diff aggregation (each repo
  needs its own `DiffStateModel`; listed as a follow-up, not part of this PR).
- The viewported `List` stays: grouping flattens into (node, file) rows the way
  the Git Log pane flattens its tree into rows, so no new list machinery.

### 5.2 Expand all / collapse all (JB toolbar actions)

**JB behavior.** Two toolbar buttons; Expand All opens every node and file,
Collapse All closes to the group roots (or the flat list top).

**Warpium mapping.** Two header buttons dispatching new
`CodeReviewAction::ExpandAllFiles` / `CollapseAllFiles`: write every path into
the existing `RepositoryState.file_expanded` map (`code_review_view.rs:536`)
and `ctx.notify()`. The `List` re-lays out via the existing invalidation path.
With grouping on, the same actions drive directory-node expansion state.

### 5.3 Collapsed by default; click file → diff

**JB behavior.** The Commit tool window shows the changes *list*; clicking a
file opens its diff (JB opens it in the editor area; Warpium has no separate
diff editor target, so the diff opens inline — the existing expanded-file
render).

**Warpium mapping.**

- Flip the default in `should_auto_expand_file` (`code_review_view.rs:565-579`)
  to false unless the user explicitly toggled (the `file_expanded` map already
  records explicit choices; the first open of a repo now simply records none).
  Binary / unrenderable / autogenerated cases stay collapsed as today.
- Make the whole file row clickable to toggle expansion (today only the chevron
  is, `:2630-2639`). The row already has `header_mouse_state`
  (`code_review_view.rs:360`).
- Lazy loading (§4.9 F2) is what makes this fast: a collapsed open renders the
  list from numstat-level data, and expansion triggers the per-file content
  fetch. Expansion shows a shimmer/loading row until the diff arrives (pattern:
  `render_loading_state`, `:3725`).
- Expand All remains for users who want the old view (§5.2); per-file override
  still wins.

### 5.4 Unversioned Files section (JB "Unversioned Files" node)

**JB behavior.** Untracked files are not mixed into changes: a separate
"Unversioned Files" group node at the bottom, collapsed, with its own rollback
(delete) and open actions.

**Warpium mapping.**

- Data: already complete — `parse_git_status` handles `?` entries
  (`local.rs:2328-2337`), Head-mode includes them (`:1788`), the watcher
  delivers them as `added` events (`repo_metadata/src/watcher.rs:564-566`), and
  the commit path stages them (`run_commit` does `git add -A`,
  `util/git.rs:671-673`).
- Presentation: when `GroupBy != Repository`-only…, an "Unversioned Files"
  group node is appended after the changes list (and inside directory grouping,
  untracked files render under it instead of their directories — JB keeps them
  strictly separate). Node collapsed by default; per-node expansion state
  in-memory.
- Stop eagerly diffing untracked files: with lazy loading (§4.9 F1) an
  untracked row shows file size / line count from disk; its all-additions diff
  is generated on expand (content read directly from disk, no
  `git diff --no-index` process per file).
- Row actions: Open, Copy path, Add as context (existing per-file actions work
  unchanged), Rollback (§5.5; for unversioned files rollback = delete, JB
  semantics).
- Commit dialog behavior (untracked included via `add -A`) is unchanged — noted
  as a deliberate deviation from JB, which requires explicitly adding
  unversioned files to a commit.

### 5.5 Rollback of selected files (JB "Rollback…")

**JB behavior.** Select files in the list (multi-select), Rollback…, confirm,
`git checkout` restores exactly those paths, list refreshes.

**Warpium mapping.**

- Selection: checkbox-free JB list uses Ctrl/Cmd-click; Warpium's list rows
  already support per-file mouse state. Add a lightweight multi-select to the
  file list (cmd-click toggles; the existing `selected_files` map in
  `DiscardDialogState` (`code_review_view.rs:508`) is the dialog-side model;
   promote it to the view's `LoadedState`). Show a selection action bar /
   enable a toolbar "Rollback…" button when the selection is non-empty.
- Action: "Rollback…" (JB naming; the existing per-file button keeps its
  Discard icon but both funnel into the same flow). Confirmation dialog: the
  existing discard dialog already renders a multi-file checklist with stats
  (`:5380-5456`); reuse it verbatim, retitle per JB ("Rollback N files?").
- Git: add `warpium_git::rollback::rollback_many(repo, paths)` — batch
  committed paths into one `git checkout HEAD -- p1 p2 …`, staged-new paths
  into one `git rm --cached --quiet -- …`, untracked paths into one
  `git clean -fd -- …` (JB deletes unversioned files on rollback; confirmed by
  the dialog), mirroring the per-file `rollback_args` (`rollback.rs:64-73`).
  App side: a `rollback_files` on `LocalDiffStateModel` modeled on
  `discard_files` (`local.rs:842-875`) — run off-thread, then the existing
  watcher/`commit_updated` + reload path refreshes the list (discard already
  reloads, `:865-869`).
- Rename handling: keep the existing per-file special-casing from
  `discard_files_impl` (`local.rs:754-817`): renamed paths fall back to the
  per-file sequence (`rm -f` new + `checkout branch -- old`).
- The stash-before-rollback checkbox stays (Warpium addition, already shipped;
  JB has no equivalent). It reuses `stash_uncommitted_changes`
  (`local.rs:583-624`).
- Unsaved-editor guard: files with unsaved editor changes get the existing
  disabled-tooltip treatment (`is_git_operation_blocked`,
  `get_discard_button_disabled_tooltip`, used at `code_review_view.rs:2588-2596`).

### 5.6 Stash tab (JB Git tool window → Stashes)

**JB behavior.** A "Stashes" list in the Git tool window: one row per
`stash@{n}` (message + date), with Create, Apply, Pop, Drop, and a diff view on
selection (the stash's combined patch shown in the editor).

**Warpium mapping.** Warpium has no Git tool window; the Code Review panel is
the git surface, so the stashes live there as a **sub-tab strip at the top of
the panel content: "Changes | Stashes"** (the panel already switches content
regions; the strip is two `ActionButton`s + persisted selection
`warpium.code_review_panel_tab`).

- Crate layer (new `crates/warpium_git/src/stash.rs`, unit-tested against real
  temp repos like the rest of the crate):
  - `list`: `git stash list --format=%gd%x1f%H%x1f%ci%x1f%B -z` →
    `Vec<StashEntry { index, message, created, oid }>`;
  - `create`: `git stash push -u -m <msg>` (reuses the existing discard-side
    semantics, `local.rs:592`);
  - `apply` / `pop` / `drop`: `git stash apply|pop|drop stash@{n}` (args
    builders returning `Vec<String>`, testable without running git);
  - `diff`: `git stash show --include-untracked --patch stash@{n}` (or
    `git diff stash@{n}^ stash@{n}` plus the untracked third parent) parsed
    into per-file patches with the crate's diff parsing.
- App layer (`app/src/code_review/stash_tab.rs`, fork-new file): a model +
  view pair listing stashes (loads lazily when the tab is selected), toolbar
  buttons Create (prompts for a message; disabled with a tooltip when
  `git_operation_in_progress`, reusing `util/git.rs:511`), Apply, Pop, Drop
  (Drop confirmed, like JB's "Drop 1 stash?"), each running off-thread via
  `ctx.spawn` and reporting in a toast, then refreshing the list and the
  Changes tab (a pop/apply mutates the working tree → the existing watcher
  path updates the diff list).
- Diff viewing: selecting a stash shows its changed files (name-status) and
  clicking a file shows the unified patch in a simple read-only viewer modeled
  on the Git Log pane's diff panel (`app/src/warpium/git_log_panels.rs`) — not
  the code review editor (that machinery compares the working tree, and
  wiring it to arbitrary commits is a bigger change than this item needs).
- All stash git runs through `warpium_git::process` conventions
  (`GIT_OPTIONAL_LOCKS=0`, `GIT_TERMINAL_PROMPT=0`, off the main thread), as
  the git tools do today.

### 5.7 Amend commit (JB "Amend" checkbox)

**JB behavior.** An "Amend" checkbox in the commit area; enabling it loads the
previous commit's message into the editor (editable) and amends on Commit,
keeping the author.

**Warpium mapping.**

- `CommitState` (`git_dialog/commit.rs:57-73`) gains `amend: bool` and an
  "Amend" checkbox under the message editor (ui_builder checkbox, like the
  existing "Include unstaged" switch, `:553-577`).
- Checking it (when not already loading) prefills the editor with
  `git log -1 --format=%B` (spawned, like the open-time changes load,
  `:168-184`) unless the user has typed; unchecking restores the placeholder.
  No HEAD → checkbox hidden (same `has_head` gate as the stash checkbox in the
  discard dialog, `code_review_view.rs:5504`).
- Confirm: `start_confirm` (`commit.rs:363-398`) passes `amend` through
  `git_commit_chain` (`diff_state/local.rs:1274`) → `run_commit_chain`
  (`git_actions.rs:30-53`) → `run_commit` (`util/git.rs:665-693`), which gains
  an `amend` flag: with amend, skip the `add -A` empty-tree guard's error
  path… specifically: still stage per "include unstaged", still run the staged
  check, then `git commit --amend -m <msg>` (author preserved by git itself;
  `--reset-author` is *not* passed). Amend with an empty working tree is
  allowed (message-only amend), matching JB.
- Button label communicates "Amend" while checked (the dialog's intent buttons
  get an "Amend" prefix, e.g. "Amend" / "Amend and push"); telemetry
  `GitOperationKind` gains an amend dimension or reuses `operation` with a
  flag, decided at implementation.

### 5.8 Summary of new/changed surface

| Area | New | Changed |
| --- | --- | --- |
| `crates/warpium_git` | `stash.rs` (`list/create/apply/pop/drop/diff` + parser) | `rollback.rs` (`rollback_many`) |
| `app/src/code_review/` | `stash_tab.rs` (model+view), `file_groups.rs` (grouping/rows/unversioned node) | `code_review_view.rs` (collapse default, row click, expand/collapse all, multi-select, toolbar, lazy editor/content), `diff_state/local.rs` (batched diff, lazy content, shared status, dirty flag, `rollback_files`), `git_repo_model/local.rs` (cached main branch), `git_dialog/commit.rs` (amend), `mod.rs` (actions) |
| `app/src/workspace/view/right_panel.rs` | — | header toolbar (Group By, Expand/Collapse, Rollback) |
| settings | `warpium.code_review_group_by`, `warpium.code_review_panel_tab` | — |

---

## 6. Feature-flag strategy

**Decision: no feature flag; ship unflagged like the git tools (#42).**

- Every feature is additive except "collapsed by default", which is the
  requested behavior, has a one-click escape hatch (Expand All, persisted), and
  only touches fork-relevant UI. A flag guarding it would be dead code the
  moment the owner approves the build.
- The fork's flag precedent (`WarpiumEmbeddedEmulator`) exists for *in-progress,
  visually broken* surfaces; this redesign replaces a working panel with a
  better one and is verified per checkpoint in the VM.
- Upstream-merge hygiene — the usual reason to isolate fork changes — is not
  helped by a flag: the diff load and view changes touch the same upstream
  functions regardless.
- Instead of a kill switch, the persisted `Group By` setting plus the existing
  per-file/per-node expansion memory give users control, and the riskiest
  perf changes (batched diff, lazy content) are behavior-preserving
  refactorings covered by the existing `diff_state` tests
  (`local_tests.rs`, 458 lines) and `code_review_view_tests.rs`.

If a checkpoint regresses badly, the fallback is reverting the offending
commits inside the PR — the commit plan (§8) is ordered so each is
independently revertable.

---

## 7. Persistence and session restore

- **Persisted (settings, fork keys):** `warpium.code_review_group_by`
  (Flat/Directory/Repository), `warpium.code_review_panel_tab`
  (Changes/Stashes). Follow the `warpium.bottom_panel` pattern:
  `sync_settings.rs` group + Settings → Features → Warpium is *not* added for
  these (JB keeps Group By as a toolbar control, not a settings page item) —
  they live behind the toolbar/sub-tab controls only.
- **In-memory only:** per-file and per-node expansion, rollback selection,
  stash list (live data). Matches JB and today's `file_expanded` handling.
- **Session restore:** today's panel is not restored at all — right-panel open
  state and repo selection live in memory (`pane_group/mod.rs:946-948`,
  `working_directories.rs:303`). The redesign does not change this; nothing in
  the design needs new persistence columns. (Reopening the panel after relaunch
  reuses the fast path from §4.9 F4.) Session-restore of the panel as a whole
  stays a separate, later item.

---

## 8. Implementation plan — one PR, ordered commits

The owner prefers one comprehensive PR. Commits are ordered so the perf work
lands first (everything after rides on the fast path), each commit keeps the
app compiling and its targeted tests green, and the marked checkpoints get a
CI-artifact VM check (mac-mcp) with screenshots.

1. **`warpium_git`: stash support + batched rollback.** `stash.rs` (list
   parser, args builders, stash diff parse) and `rollback_many`. Unit tests
   against temp repos. *Checkpoint: `cargo test -p warpium_git`.*
2. **Diff-load performance (§4.9 F1, F2, F3, F5).** Single batched `git diff`
   with per-file parse; lazy base content (`cat-file --batch`, bounded
   parallel); untracked rows from disk stats with lazy all-additions diff;
   shared status snapshot between diff load and metadata; memoized
   `detect_main_branch`. Behavior-neutral; existing `diff_state` tests must
   pass unchanged. *Checkpoint: targeted tests + measure with the synthetic
   500-file repo; expect cold load ≈ 3-5 spawns + parallel blob fetches.*
3. **Collapsed by default + click-to-expand + Expand/Collapse All (§5.2-5.3).**
   Default flip, whole-row click, toolbar buttons. *Checkpoint: VM screenshot;
   update `code_review_view_integration.rs` / integration expectations.*
4. **Group By + Unversioned Files node (§5.1, §5.4).** `file_groups.rs` using
   `warpium_git::tree`; Group By dropdown; unversioned group node; persisted
   `warpium.code_review_group_by`. *Checkpoint: VM check of all three modes.*
5. **Rollback of selected files (§5.5).** List multi-select, toolbar action,
   `rollback_files` on the diff model using `warpium_git::rollback_many`,
   confirmation dialog reuse, reload after. *Checkpoint: VM check on a fixture
   repo; staged-new + unversioned + rename cases.*
6. **Stash tab (§5.6).** Sub-tab strip, stash list model/view, create/apply/
   pop/drop with toasts and refresh, diff viewer per file. Persisted tab
   selection. *Checkpoint: VM check.*
7. **Amend (§5.7).** Checkbox, message prefill, `run_commit(amend)` threading,
   labels + telemetry. *Checkpoint: VM check of amend with staged/unstaged/none.*
8. **Open-path fast reopen (§4.9 F4) + watcher coalescing (R6)**, final
   measurements (telemetry/log-based, in the VM release build), docs: this
   file's "implemented" notes, WARPIUM.md panel section, PR changelog entry.

Mid-PR checkpoints make sense after **3** (core interaction model is decided),
**5** (all working-tree mutations land), and **8** (perf claims verified). If
any checkpoint fails review, its commit range is reverted without disturbing
the rest.

**Testing.** Crate: `cargo test -p warpium_git`. App: existing
`code_review_view_tests.rs`, `diff_state/local_tests.rs`, `local_tests.rs`
(git_repo_model) stay green; new unit tests for grouping rows, unversioned
partitioning, stash list parsing, rollback batching (in the fork test files
matching the `warpium` path filter CI runs). Integration: extend
`app/src/integration_testing/code_review/mod.rs` for collapse-default and
toolbar actions.

---

## 9. Open questions

1. **Single vs. double click to open a diff.** JB single-click selects,
   double-click opens in an editor; Warpium has no separate diff editor, and
   the inline expand replaces "open". Decision: single click toggles inline
   (with a 150 ms hover delay nowhere — immediate); revisit if it conflicts
   with text selection in expanded diffs (click target is the collapsed row
   only).
2. **Repository grouping with a single repo.** JB's Repository grouping is
   meaningful only in multi-root projects; the panel is per-repo. Decision:
   implement the repo node as the structural root (correct today, enables a
   later multi-repo mode) rather than building multi-repo aggregation now.
   Flag for owner if multi-repo aggregation was the intent behind the feature.
3. **Stash diff viewer depth.** JB shows the stash diff in a full editor with
   per-hunk navigation. Decision: start with the Git-Log-pane-style unified
   patch viewer (read-only, per file); upgrading it to the code review editor
   is a follow-up.
4. **Unversioned files in commits.** JB excludes them by default; Warpium's
   commit dialog includes them (`git add -A`). Left unchanged here — say the
   word if JB parity (opt-in add) is wanted, but it changes commit semantics
   and deserves its own review.
5. **Rollback of unversioned files = delete** (JB semantics, implemented via
   `git clean -fd -- paths` inside the confirmation dialog). If the owner
   prefers unversioned rollback to be "exclude from list" instead, that's a
   one-line change in the batching split.

---

## Implementation status (2026-11-24, PR "Code Review panel: JetBrains Commit tool window parity")

All eight plan commits landed on `claude/code-review-jb`, in plan order:

1. **warpium_git foundations** — `stash.rs` (list parse, args builders,
   per-file patch split), `rollback_many` (batching split tested:
   committed→`checkout HEAD`, added→`rm --cached`, untracked→`clean -fd`),
   `cat_file_batch`.
2. **Diff-load performance (§4.9 F1, F2, F3, F5)** — one batched
   `git diff --patch-with-raw -z` + one `cat-file --batch` + untracked
   synthesized from disk; combined Head-mode open round shares one status walk
   between metadata and diffs; `detect_main_branch` memoized per process;
   editors/buttons lazy on first expand. Measured on the synthetic 100-file
   change set (this doc's method, local disk): **202 spawns / ~157-189 ms →
   4 spawns / ~13-16 ms**; the doc's dev-container numbers (per-spawn ~15 ms
   on the bind mount) put the old path at ≈1.44 s for the same set.
3. **Collapsed by default + click-to-expand + Expand/Collapse All (§5.2-5.3)**
   — single click toggles immediately; Expand All restores the old
   fully-expanded view on demand.
4. **Group By + Unversioned Files node (§5.1, §5.4)** — Flat / Directory /
   Repository dropdown persisted in `warpium.code_review_group_by`;
   unversioned node collapsed by default in every mode.
5. **Rollback of selected files (§5.5)** — cmd-click selection, header
   Rollback… button, "Rollback N files?" checklist dialog, batched git,
   renames per-file.
6. **Stash tab (§5.6)** — Changes | Stashes strip
   (`warpium.code_review_panel_tab`), list/create/apply/pop/drop with toasts,
   read-only per-file unified patch viewer (capped at 2,000 lines).
7. **Amend (§5.7)** — checkbox, message prefill, `git commit --amend`,
   pushed-commit warning, "Amend" button prefixes. Local-only: the wire
   protocol has no amend bit, so remote repos keep current behavior
   (documented deviation; the proto change deserves its own review).
8. **Fast reopen + watcher coalescing (§4.9 F4)** — dirty flag skips the diff
   reload on a clean reopen; commit-level watcher storms coalesce through the
   5 s throttle. Docs: WARPIUM.md's "Code Review panel" section.

Deviations from this doc: none beyond the two documented above (amend
local-only; unversioned stay committable via `git add -A`). VM checkpoint
screenshots (§8) are pending the mac-mcp VM's return.

Remaining follow-ups (out of scope here): editor-grade stash diff viewer,
multi-repo aggregation, remote-repo amend (proto), session restore of the
panel.
