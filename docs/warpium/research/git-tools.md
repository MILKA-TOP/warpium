# Git tools: log and graph, change markers and Rollback, blame

Roadmap items 3 (Git log / graph), 20 (gutter change markers and Rollback) and 19 (Git Blame),
with JetBrains IDEA and Android Studio as the reference.

## What Warp already has

- **git plumbing.** No git library in the UI path: Warp shells out to `git` through
  `warp_util::git::run_git_command` (async, `GIT_OPTIONAL_LOCKS=0`, WSL aware). `git2` is a
  dependency of the app but only used in a few places. `repo_metadata` watches repositories and
  routes file events; `code_review::git_repo_model` tracks branch and status.
- **Code review pane** (`app/src/code_review/`). Diffs of the working tree against `HEAD`, the
  main branch or another branch, one editor per file, with a per-hunk revert button
  (`FeatureFlag::RevertDiffHunk`, on by default) and "Discard changes" for a file or all files
  with a confirmation dialog (`discard_per_file_and_all_changes`, on by default). This *is*
  Rollback for whole files from the changes list; nothing new is needed there.
- **Code editor diff machinery** (`app/src/code/editor/diff.rs`, `model.rs`, `element.rs`,
  `nav_bar.rs`). Any `CodeEditorView` with a base text (`set_base`) computes line hunks off the
  main thread on every edit (`similar`), draws added/changed/deleted slivers in the gutter, can
  expand a hunk inline with the removed lines (diff navigation), and has a bar with
  "Hunk i/n", Previous, Next and a revert button that applies the reverse edit to the buffer.
  Code review uses all of it; the file editor panes never set a base, so none of it showed there.
- **No log, graph or blame anywhere.**

## Architecture

All parsing and layout is in the protoc-free crate `crates/warpium_git` (unit-tested against
fixtures and real temporary repositories); the app only renders.

| Piece | Crate | App |
| --- | --- | --- |
| `git log` arguments, streaming parser | `log.rs` | |
| Graph lane layout | `graph.rs` | `warpium/git_log_graph.rs` draws a row |
| Background reader with demand-driven paging | `process.rs` (`spawn_log`, `LogStream`) | `warpium/git_log.rs` |
| Commit details, changed files, unified diff | `show.rs`, `diff.rs` | details panel and diff view in the pane |
| Branch labels | `refs.rs` | ref chips, branch filter |
| Blame (`--porcelain --contents -`) | `blame.rs` | `code/editor/warpium_blame_column.rs` |
| Base text for markers, Rollback | `rollback.rs` | `code/warpium_git.rs` |

### Git Log pane (item 3)

- A pane like Logcat (`IPaneType::WarpiumGitLog`, never persisted), one per window, opened below
  the active pane from "Show Git Log" (the tab's locked project root, else the project toolbar's
  project, else the active session's directory, resolved to its work tree root).
- **Streaming.** `git log --date-order --decorate=full --format=<fields separated by 0x1f,
  records by 0x1e>` runs on a thread that parses records incrementally (records may span reads)
  and pushes each commit through the graph builder, so layout never touches the main thread.
  Rows go to the UI in batches (at most 2,000 rows or 50 ms). The thread stops reading once it
  has sent as many rows as the UI asked for; git then blocks on its full pipe. The list asks for
  the visible end plus 1,500 rows of read-ahead and pages of 5,000, so the first screen of a
  repository with 500k commits costs the same as a small one, and memory grows only as far as
  the user scrolls. git's commit-graph file makes `--date-order` output start immediately.
- **Graph layout** (`GraphBuilder`): lanes waiting for a commit; a commit takes the leftmost lane
  waiting for it (other waiting lanes join it), or opens a lane on the right (a branch tip). Its
  first parent continues its lane, further parents join a lane already waiting for them or open
  one in the first free slot to the right. Lanes are compacted left after every row. A row is
  stored as segments in two halves (top edge → node row middle, middle → bottom edge), 8 bytes
  each, so consecutive rows join exactly; lanes past 48 are laid out but not drawn. O(lanes) per
  commit: 360k synthetic commits lay out in milliseconds in release builds (a test bounds it).
- **Filters.** Branch (HEAD, all branches, any local or remote branch), author (`--author`),
  text (`--grep`, case-insensitive, fixed string) and path (file history, with `--parents` so
  parents are rewritten and the graph stays connected). Author and text filters drop commits
  without rewriting parents, so their results are listed without a graph, as IDEA does. A text
  that resolves to a commit (`rev-parse --verify <text>^{commit}`) starts the log there and
  selects it.
- **Details.** Selecting a commit loads `git show -s` (full message, author, committer, refs)
  and `git diff-tree --name-status -M` against the first parent (merges included). Clicking a
  file shows its patch (`git diff-tree -p -M`) in the details panel, with line numbers and
  colored lines; Esc goes back. Code review compares the working tree, not a commit with its
  parent, so this diff view is a light unified-diff list inside the pane rather than a reuse of
  the code review editor.
- **Selecting from elsewhere.** "Show commit" (from blame) reuses the pane, keeps reading until
  the commit is loaded (switching to all branches without filters first) and selects it.

### Change markers and Rollback (item 20)

File editor panes now call `CodeEditorView::set_base` with the file's `HEAD` blob (`git cat-file
blob HEAD:<path>`; empty for a staged new file, nothing for an untracked one), refreshed when the
file loads, when it changes on disk (save, checkout, rollback) and when the editor regains focus
(at most every 2 s, to pick up commits made in a terminal). Everything else is Warp's existing
diff machinery, switched on for these editors (`enable_warpium_change_markers`): clicking a gutter
marker expands that change inline, the bar offers Previous, Next and **Rollback** (the existing
revert, relabelled), and hovering an expanded change shows the revert gutter button. The diff is
recomputed off the main thread after every edit, as before. "Rollback…" for the whole file asks
for confirmation, then runs `git checkout HEAD -- <path>` (or `git rm --cached` for a staged new
file, which stays on disk) and reloads the buffer. The code review pane keeps its own per-file
Discard. Setting `warpium.git_gutter_markers` (default on).

### Blame (item 19)

"Annotate with Git Blame" runs `git blame --porcelain --contents -` with the buffer on stdin, so
unsaved edits show as not committed; it reruns after every save. The annotation column wraps the
editor's scrollable and positions rows from the render model (`y_offset_at_line`), so it follows
scrolling, soft wrap and expanded diffs; only visible lines get elements. Each run of lines from
one commit shows the date and author once, tinted by age; hovering shows hash, author, date and
summary; clicking opens the commit in the Git Log pane. Setting `warpium.inline_blame` (default
off) shows the cursor line's author, age and summary at the right of that line.

## Comparison with IntelliJ IDEA / Android Studio

The target is the Log tab of the Git tool window, the editor's change markers, Annotate with Git
Blame and the diff viewer of IntelliJ IDEA 2024–2025 (the JetBrains help pages "Log tab",
"Investigate changes in Git repository", "Compare file versions" and "Git tool window"). The
pages couldn't be fetched from the dev container, so the table is written from those pages as
remembered; check it against IDEA in the VM (see the PR's check list).

Status: **identical** (same element, place and wording), **differs** (present, but looks or
behaves differently), **missing**.

### Log tab layout

| IDEA | Warpium | Status |
| --- | --- | --- |
| Git tool window docked at the bottom, Log tab | Git Log pane, opened as a split below the active pane | differs (not docked in the bottom panel yet) |
| Branches pane on the left (HEAD, Local, Remote groups; double-click filters) | Branches pane on the left: HEAD (Current Branch), All branches, Local, Remote; a click filters | differs (flat list, no tree folding, no favorites/toolbar, single click) |
| Commit list in the middle, filter bar above it | same | identical |
| Changed files (top) and commit details (bottom) on the right | same: "Changes · N files" tree above "Commit Details" | identical |
| Pane proportions roughly 1 : 3 : 2 | branches 200 px, list 3, right side 2 | identical |

### Filter bar

| IDEA | Warpium | Status |
| --- | --- | --- |
| Search field "Text or hash" (regex and match-case toggles) | "Text or hash" field first; a hash starts the log at that commit | differs (no regex/match-case toggles; case-insensitive substring) |
| Branch filter | "Branch: HEAD / All / name" chip with a menu | identical |
| User filter (me, list of authors, free text) | "User" text field (substring of name or email) | differs (free text only) |
| Date filter | — | missing |
| Paths filter | "Paths: file" chip when showing a file's history, × clears it | differs (set from "Show history for file" only; no path picker) |
| Refresh, Cherry-pick, Highlight, View Options (show columns, IntelliJ sort) | Refresh button | missing: cherry-pick button, highlight and view options (cherry-pick is in the context menu) |

### Commit list

| IDEA | Warpium | Status |
| --- | --- | --- |
| Columns Subject (graph + ref labels + subject), Author, Date; hash hidden by default | graph, ref labels + subject, Author, Date | identical |
| Graph: colored lanes, merge edges, HEAD marked | lanes colored per branch, diagonal merge/fork edges, ring node for HEAD | differs (no long-edge collapsing with arrows, no lane highlight on hover) |
| Ref labels at the right of the subject | ref chips before the subject (HEAD's branch first) | differs (side) |
| Bold subject for unpushed / current commit | HEAD commit's subject bold | differs |
| Keyboard navigation, copy hash | Up/Down/PageUp/PageDown, cmd-c copies the hash | identical |

### Commit context menu (in IDEA's order)

| IDEA | Warpium | Status |
| --- | --- | --- |
| Copy Revision Number | Copy Revision Number | identical |
| Create Patch… | — | missing |
| Cherry-Pick | Cherry-Pick | identical |
| Checkout Revision | Checkout Revision (with confirmation) | differs (asks first) |
| Show Repository at Revision | — | missing |
| Compare with Local | — | missing |
| Reset Current Branch to Here… (Soft, Mixed, Hard, Keep) | Reset Current Branch to Here… with Soft, Mixed, Hard, Keep | identical |
| Revert Commit | Revert Commit (`--no-edit`) | differs (no commit message dialog) |
| Undo Commit…, Edit Commit Message…, Fixup…, Squash Into…, Drop Commits, Interactively Rebase from Here…, Push All up to Here… | — | missing |
| New Branch…, New Tag… | New Branch…, New Tag… (name prompt) | identical |
| Go to Child Commit, Go to Parent Commit | Go to Child Commit, Go to Parent Commit | identical |
| Open in GitHub | — | missing |

### Changed files and details

| IDEA | Warpium | Status |
| --- | --- | --- |
| Changed files as a tree grouped by directory, single-child folders merged | same, with the file count per folder | identical |
| File names colored by change (added green, modified blue, deleted grey, renamed) plus status | colored names and the status letter | differs (letter shown) |
| Toolbar: Show Diff, expand/collapse, group by | — (click a file to show its diff) | missing |
| Details: subject and message, hash, author and date, committer, branches containing the commit | subject (bold) and body, short hash with copy button, author with email and date, committer when different, ref labels | differs (no "In N branches" list) |

### Diff viewer

| IDEA | Warpium | Status |
| --- | --- | --- |
| Opens in its own editor tab or window | replaces the pane's content, back arrow or Esc returns | differs |
| Previous Difference / Next Difference (up/down arrows) | ↑ Previous Difference, ↓ Next Difference | identical |
| Viewer switch: Side-by-side viewer / Unified viewer | two buttons "Side-by-side viewer", "Unified viewer" | differs (buttons instead of a dropdown) |
| Before / After columns with revision and path headers | "Before: <parent> · path" and "After: <commit> · path" captions | identical |
| Line numbers, changed lines tinted, changed words highlighted, connectors between sides | line numbers, tinted lines | differs (no word-level highlight, no connector gutter, no syntax highlighting) |
| Whitespace and highlighting options, collapse unchanged fragments, synchronized scrolling | — (only changed hunks with three lines of context, as git prints them) | missing |
| Previous/Next file in the commit | — | missing |

### Editor

| IDEA | Warpium | Status |
| --- | --- | --- |
| Gutter change markers (added, modified, deleted) | same, from Warp's diff machinery | identical |
| Click a marker: popup with Rollback, Show Diff, Previous/Next Change, Copy | the change expands inline with a bar: Hunk i/n, Previous, Next, Rollback | differs (inline bar instead of a popup; no Copy) |
| Rollback… for a file, with a confirmation | Rollback… with a confirmation | identical |
| Annotate with Git Blame: column of date and author, colored by age; hover details; click shows the commit | same; click opens the commit in the Git Log | identical |
| Annotation options (show/hide date, author, hash; color by author), Annotate Previous Revision | — | missing |
| Inline blame of the current line (GitToolBox-style; IDEA 2024.3 adds it) | `warpium.inline_blame` | identical |

## Next steps (follow-up PR)

In order: dock the Git Log in the bottom panel with a Log tab strip; Date filter and a User menu
(me, recent authors); regex/match-case toggles; branches tree folding, favorites and its toolbar
(checkout, new branch, delete, compare, merge, rebase from the branches pane); diff viewer with
word-level highlighting, syntax colors, synchronized side-by-side scrolling over full files with
unchanged fragments collapsed, whitespace options, a viewer dropdown, next/previous file; Compare
with Local and Show Repository at Revision; Create Patch, Undo Commit, Edit Commit Message,
Fixup/Squash/Drop and interactive rebase; the changes toolbar (Show Diff, expand, group by);
Annotate options and Annotate Previous Revision; the marker popup (Rollback, Show Diff, Copy) in
place of the inline bar; long-edge collapsing in the graph.

## Left out for now

- Docking the Git Log pane in the bottom panel (it opens as a split).
- Side-by-side diffs and opening a commit's file version in a read-only editor.
- `--follow` across renames for file history (path-limited history stops at the rename).
- Blame of an older revision ("Annotate previous revision") and blame with whitespace or move
  detection options.
- Branch operations from the log (checkout, cherry-pick, reset, …).
