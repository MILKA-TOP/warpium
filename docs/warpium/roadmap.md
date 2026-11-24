# Warpium roadmap

The owner's plan for the fork and where each item stands. It's kept here, not in an agent's
private memory, so every agent can work from it. Update it when an item lands, its scope changes or
the owner decides something; the "Decisions" lines record what the owner chose, so don't reopen them
without asking. Developer details of finished features are in [WARPIUM.md](../../WARPIUM.md); how
agents work on the fork is in [agent-workflow.md](agent-workflow.md).

## Product direction

Warpium is a lightweight IDE for AI-driven mobile development, built on Warp (OSS channel, macOS on
Apple silicon first). The goal is to replace Android Studio for day-to-day work on KMP and Android
projects, while CLI agents run in its terminals.

Requirements that apply to every item:

- **Performance comes first.** Typing, scrolling and switching must never stutter. Prefer Rust-native,
  in-process code (parsing, indexing, log handling). Heavy JVM pieces (Gradle Tooling API,
  kotlin-lsp, layoutlib) only on demand or as warm sidecars with idle shutdown; nothing should hold a
  lot of memory constantly (the in-process code index stays within about 150 MB).
- **Copy JetBrains IDEA / Android Studio behaviour exactly** where a feature exists there (keymap,
  layouts, menus, wording), unless Warp already binds the key; then pick the nearest free one and
  document it.
- **One big, comprehensive PR per feature** rather than a chain of small ones (a separate research
  PR is fine).
- **Never offer or install official Warp updates**; Warpium updates itself from this repository's
  releases only.
- **Per-project configuration** lives in `<project>/.warpium/` (only committable files such as
  `toolbar.toml`); caches and indexes go to the user cache directory, never into the project.
- The README is for end users only (install steps, short feature list, planned features at the end);
  everything for developers and agents goes to WARPIUM.md and this folder.

## Items

Status: **done** (merged), **review** (PR open), **wip** (in progress), **planned**, **later**.

| # | Item | Status |
| --- | --- | --- |
| 1 | "Open in new tab" in the Project Explorer opened the repo root, not the chosen folder | done (#8) |
| 2 | Global search: `.gitignore` toggle and include/exclude globs | planned, together with 22 |
| 3 | Git log / graph view, identical to IDEA | done (#42); follow-ups below |
| 4 | Notification when upstream Warp has new commits | done (#9, plus the Upstream Watch workflow) |
| 5 | Paste images into CLI agents inside Docker containers and SSH sessions | done (#41) |
| 6 | Rebrand to Warpium (name, logo, icon, onboarding, README) | done |
| 7 | Kotlin/Java navigation: code index, Go to Class/Symbol/Definition/Implementation, Find Usages, MCP server | done (#14, #31, #37 and others) |
| 8 | Run configurations from Android Studio / IDEA in the toolbar | done (#40); Compose Preview still open |
| 9 | Import and keep syncing the official Warp app's configuration | done (#7) |
| 10 | JetBrains-style project toolbar with a run-configurations dropdown and a GUI editor | done |
| 11 | Projects (JetBrains Recent Projects) | later, near the end, polished |
| 12 | Per-tab project root (Follow / Locked), single source of truth | done (#25) |
| 13 | Android: devices, AVDs, run, Logcat (phase A); emulator embedded in a pane (phase B) | A done; B partly done (#34, #36), rest later |
| 14 | Bottom panel (JetBrains tool window) for long-running panes | done (#30, #38) |
| 15 | Logcat MCP server for agents | done (#22) |
| 16 | Android Studio colors, starting with syntax highlighting | later |
| 17 | JetBrains "Islands" UI redesign | later |
| 18 | Code completion and hints in the editor | research done (#43); phase 1 wip |
| 19 | Git blame / Annotate | done (#42) |
| 20 | Change markers in the gutter and Rollback, like IDEA | done (#42); follow-ups below |
| 21 | Search Everywhere on `cmd-O` (All / Classes / Symbols / Files) | done (#39) |
| 22 | Find in Files on `cmd-shift-F`, plus a Text tab in `cmd-O` | planned |
| 23 | Code Review panel → JetBrains Commit tool window: group by directory/repository, expand/collapse all, collapsed-by-default file list, unversioned files, rollback selected files, stash tab, amend commit, fast open | research |

Also done along the way: Warpium self-update from GitHub Releases (#11), the fork's CI and release
workflows, the test VM MCP server (`tools/mac-mcp`).

## Queue

1. Item 23 (research first, then one big implementation PR; owner list: group by directory/repository, expand/collapse all, collapsed default with click-to-open diff, unversioned files, rollback of selected files, stash tab, amend commit, open-performance).
2. Git tools follow-ups (below) and the rest of IDEA parity from
   [research/git-tools.md](research/git-tools.md) "Next steps".
3. Item 18 phase 1: completion from the code index (branch `claude/code-completion`).
4. Item 18 next phase: library symbols in the index (Kotlin stdlib, AndroidX, Android SDK sources).
5. Item 22 (with item 2).
6. Item 11.
7. Item 13 phase B, remaining PRs (input, launching, stable texture, physical devices).
8. Items 16 and 17.

Git tools follow-ups found in the VM check of #42: in a narrow pane the commit subject column
collapses before author and date (IDEA shrinks author and date first); the filter fields collapse to
empty boxes when narrow; clicking the 2 px gutter marker doesn't open the change (the palette's "Show
change at cursor" does; IDEA's marker is wider and opens a popup); after Rollback the editor still
shows the unsaved dot although the text equals HEAD; the hunk bar stays after Esc; the Git Log opens
as a small split instead of a tool window (dock it in the bottom panel).

Small follow-ups found in VM checks: the Classes tab of `cmd-O` says "No Results Found" on an empty
query; the Devices pane header shows `DevicesFake AVD 5900` and the Git Log header `Git Logkt` without a
separator.

## Item details and decisions

### 3, 19, 20: Git tools

Must be identical to IDEA's Git tool window ("точь в точь"): the Log tab layout (branches tree on the
left, commit list with graph in the middle, changed files and details on the right), graph
visualization, sorting and the filter bar, commit actions in the context menu (checkout,
branch/tag, cherry-pick, revert, reset, compare, history), commit details, and before/after
(side-by-side and unified) diff comparison. Compare the visuals directly with IDEA element by
element (button placement and icons, toolbar order, column order, filters, menus, diff viewer) and
keep the comparison table in [research/git-tools.md](research/git-tools.md). Blame (19) and change
markers with Rollback (20) follow IDEA too.

### 8: IDE run configurations and Compose Preview

Done for Gradle, shell script, Android App, JUnit/Android tests and compound configurations. Still
open: Compose Preview. Routes to research: on a device through
`androidx.compose.ui.tooling.PreviewActivity` plus a screenshot; headless layoutlib (as Paparazzi and
Roborazzi do); Compose Desktop for KMP. `@Preview` discovery should use tree-sitter Kotlin.

### 11: Projects

A projects picker / welcome list like JetBrains Recent Projects: recent and pinned projects, search,
opened by a shortcut or the palette. Choosing one opens a new tab locked to the project (item 12),
possibly with per-project tab layouts or launch configurations. Do it near the end, polished.

### 12: Tab project root

Decisions: tab-level (not window-level); lock when a project is opened explicitly; container ↔ host
path mappings (for example in `.warpium/project.toml`) later.

### 13: Android

Decisions: the emulator runs in its own window first, the embedded pane is phase B; physical devices
are supported from phase A. Phase B was moved to the end of the queue by the owner; the plan is in
[research/embedded-emulator.md](research/embedded-emulator.md). Earlier decisions for phase B:
own-window launch stays the default (embedded is opt-in), split-right placement, no audio, confirm
before stopping an embedded emulator, performance work before physical-device mirroring.

### 14: Bottom panel

Decisions: per project/tab, not per window; closing the layout's last visible pane while panes are
docked opens a fresh terminal and keeps the tab; `exit` in a docked terminal removes only its
sub-tab; restore goes to the previous position, else split right; `cmd-j` toggles.

### 18: Code completion

Research and plan: [research/code-completion.md](research/code-completion.md). Decisions:

- the popup opens as you type, like JetBrains;
- `ctrl-space` invokes it inside the code editor only (overriding Warp's global binding there);
- after phase 1 (completion, auto-import, parameter info and quick docs from the index), index
  library symbols next, before LSP completion;
- AI ghost text later, opt-in, through fast models run with the Docker CLI (Docker Model Runner).

### 16, 17: Look and feel

16: match Android Studio's default dark and light schemes token by token, starting with code syntax
colors (inspect Android Studio's color scheme XML). 17: JetBrains "Islands" UI (floating rounded
panels on a darker background). Far future.

### 22: Find in Files

Like IDEA's Find in Path popup on `cmd-shift-F`: query with match case / words / regex, file mask,
scope (project, module, directory), a results list with preview, opening at the match; plus a Text
tab in the `cmd-O` palette that reuses it. The Symbols tab only searches declaration names, so
string literals such as "добрый день" are this item's job. Reuse Warp's existing global search
(`crates/warp_ripgrep`) and fold in item 2's `.gitignore` toggle and globs.

### 7: Kotlin navigation

Decisions: KMP and plain Kotlin/JVM projects both matter; Java is supported by the index but no
jdtls; kotlin-lsp only off by default / on demand with idle shutdown. Benchmarks on the owner's
large private project report aggregate numbers only: never commit its code, names or paths. Research
report: [research/kotlin-navigation-and-toolchain.md](research/kotlin-navigation-and-toolchain.md).
