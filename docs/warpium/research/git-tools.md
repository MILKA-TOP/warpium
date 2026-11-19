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

## Left out for now

- Docking the Git Log pane in the bottom panel (it opens as a split).
- Side-by-side diffs and opening a commit's file version in a read-only editor.
- `--follow` across renames for file history (path-limited history stops at the rename).
- Blame of an older revision ("Annotate previous revision") and blame with whitespace or move
  detection options.
- Branch operations from the log (checkout, cherry-pick, reset, …).
