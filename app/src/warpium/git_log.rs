//! Warpium's Git Log pane, laid out like the Log tab of JetBrains IDEs' Git tool window: branches
//! on the left, the commits with their graph in the middle (filter bar above), the selected
//! commit's changed files and details on the right, and a Before/After diff viewer.
//!
//! `git log` runs on a background thread (`warpium_git::spawn_log`) that parses its output and
//! lays out the graph as it reads, and only reads as far as the list has been scrolled (plus a
//! page), so opening the log of a repository with hundreds of thousands of commits costs about
//! as much as opening a small one. The list only lays out its visible rows.

use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use warp_core::r#async::debounce;
use warp_core::ui::appearance::Appearance;
use warp_core::ui::icons::Icon;
use warpium_git::actions::{CommitAction, ResetMode, ref_name_error};
use warpium_git::diff::{
    DiffLine, FileChange, SideBySideRow, changed_files_args, file_diff_args, parse_name_status,
    parse_unified_diff, side_by_side,
};
use warpium_git::format::{local_date_time, truncate_chars};
use warpium_git::log::{LogQuery, Revisions};
use warpium_git::oid::looks_like_hex_id;
use warpium_git::process::{relative_to_repo, repo_toplevel, run_git};
use warpium_git::refs::{RefKind, RefLabel, branch_list_args, parse_branch_list};
use warpium_git::show::{CommitDetails, details_args, parse_details};
use warpium_git::tree::{TreeRow, file_tree};
use warpium_git::{LogEvent, LogRow, LogStream, Oid, spawn_log};
use warpui::clipboard::ClipboardContent;
use warpui::elements::{
    Border, ChildAnchor, ChildView, Clipped, ClippedScrollStateHandle, ConstrainedBox, Container,
    CornerRadius, CrossAxisAlignment, DispatchEventResult, EventHandler, Expanded, Fill, Flex,
    Hoverable, MouseStateHandle, OffsetPositioning, Padding, ParentAnchor, ParentElement,
    ParentOffsetBounds, Radius, SavePosition, ScrollStateHandle, Scrollable, ScrollableElement,
    ScrollbarWidth, Shrinkable, Stack, Text, UniformList, UniformListState,
};
use warpui::fonts::{Properties, Weight};
use warpui::keymap::FixedBinding;
use warpui::text_layout::ClipConfig;
use warpui::ui_components::components::UiComponent as _;
use warpui::{
    AppContext, Element, Entity, ModelHandle, SingletonEntity, TypedActionView, View, ViewContext,
    ViewHandle, WeakViewHandle, WindowId,
};

use super::git_log_graph::GraphCell;
use super::git_tools::run_in_background;
use super::logcat::chip_button;
use crate::editor::{
    EditorOptions, EditorView, Event as EditorEvent, PropagateHorizontalNavigationKeys, TextOptions,
};
use crate::menu::{Event as MenuEvent, Menu, MenuItem, MenuItemFields};
use crate::pane_group::focus_state::PaneFocusHandle;
use crate::pane_group::pane::view::{self, HeaderContent, StandardHeader, StandardHeaderOptions};
use crate::pane_group::{BackingView, PaneConfiguration, PaneEvent};
use crate::ui_components::buttons::icon_button;
use crate::util::bindings::CustomAction;
use crate::view_components::DismissibleToast;
use crate::workspace::{PaneViewLocator, ToastStack, WorkspaceAction};

const HEADER_TEXT: &str = "Git Log";
/// Rows read before the list is scrolled.
const INITIAL_ROWS: usize = 2_000;
/// Rows read ahead of the last visible one.
const PREFETCH_ROWS: usize = 1_500;
const PAGE_ROWS: usize = 5_000;
/// Lines of a file diff shown, at most.
const MAX_DIFF_LINES: usize = 20_000;
const FILTER_DEBOUNCE: Duration = Duration::from_millis(300);
const FONT_SIZE: f32 = 12.;
const ROW_HEIGHT: f32 = 22.;
const LANE_WIDTH: f32 = 14.;
const CONTROL_HEIGHT: f32 = 24.;
const AUTHOR_WIDTH: f32 = 140.;
const DATE_WIDTH: f32 = 116.;
const MENU_WIDTH: f32 = 300.;
const DETAILS_PADDING: f32 = 10.;
const BRANCHES_WIDTH: f32 = 200.;

pub(crate) fn init(app: &mut AppContext) {
    use warpui::keymap::macros::*;

    let context = id!(GitLogView::ui_name());
    app.register_fixed_bindings([
        FixedBinding::custom(
            CustomAction::Copy,
            GitLogAction::CopyHash,
            "Copy commit hash",
            context.clone(),
        ),
        FixedBinding::new("up", GitLogAction::SelectBy(-1), context.clone()),
        FixedBinding::new("down", GitLogAction::SelectBy(1), context.clone()),
        FixedBinding::new("pageup", GitLogAction::SelectBy(-20), context.clone()),
        FixedBinding::new("pagedown", GitLogAction::SelectBy(20), context.clone()),
        FixedBinding::new("escape", GitLogAction::CloseDiff, context),
    ]);
}

/// Tracks the Git Log pane of each window, so showing the log reuses it.
#[derive(Default)]
pub(crate) struct GitLogPaneManager {
    panes: HashMap<WindowId, PaneViewLocator>,
}

impl GitLogPaneManager {
    pub(crate) fn find_pane(&self, window_id: WindowId) -> Option<PaneViewLocator> {
        self.panes.get(&window_id).copied()
    }

    pub(crate) fn register_pane(&mut self, window_id: WindowId, locator: PaneViewLocator) {
        self.panes.insert(window_id, locator);
    }

    pub(crate) fn deregister_pane(
        &mut self,
        window_id: &WindowId,
        pane_id: crate::pane_group::PaneId,
    ) {
        if self
            .panes
            .get(window_id)
            .is_some_and(|locator| locator.pane_id == pane_id)
        {
            self.panes.remove(window_id);
        }
    }
}

impl Entity for GitLogPaneManager {
    type Event = ();
}

impl SingletonEntity for GitLogPaneManager {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GitLogViewEvent {
    Pane(PaneEvent),
}

/// Which history the branch filter lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BranchChoice {
    Head,
    All,
    Branch(String),
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum GitLogAction {
    ToggleBranchMenu,
    SetBranch(BranchChoice),
    Refresh,
    ClearPath,
    SelectRow(usize),
    SelectBy(i32),
    CopyHash,
    OpenFileDiff(usize),
    CloseDiff,
    OpenWorkingFile(String),
    FocusEditor(FilterField),
    /// Selects the row and opens its context menu.
    OpenCommitMenu(usize),
    CommitMenu(CommitMenuItem),
    ConfirmPending,
    CancelPending,
    /// Resets the current branch to the pending reset's commit.
    ConfirmReset(ResetMode),
    ToggleDiffMode,
    /// Scrolls the diff to the next (`true`) or previous difference.
    JumpToDifference(bool),
}

/// The commit context menu's entries, as in JetBrains IDEs' Log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CommitMenuItem {
    CopyRevision,
    CherryPick,
    Checkout,
    Reset,
    Revert,
    NewBranch,
    NewTag,
    GoToChild,
    GoToParent,
}

/// An action waiting for confirmation or a name.
enum PendingAction {
    Confirm {
        action: CommitAction,
        oid: Oid,
        title: String,
        message: String,
    },
    Name {
        tag: bool,
        oid: Oid,
        error: Option<String>,
    },
    /// Reset Current Branch to Here, waiting for a mode.
    Reset { oid: Oid },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FilterField {
    Author,
    Text,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GitLogOverflowAction {
    Refresh,
    CopyHash,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum LoadState {
    /// No repository yet.
    Idle,
    /// Looking up the repository.
    Resolving,
    Loading,
    Finished,
    Failed(String),
}

enum DetailsState {
    None,
    Loading,
    Loaded(Box<LoadedDetails>),
    Failed(String),
}

struct LoadedDetails {
    details: CommitDetails,
    files: Result<Vec<FileChange>, String>,
    /// The changed files as a directory tree.
    tree: Vec<TreeRow>,
    file_mouse_states: Vec<MouseStateHandle>,
}

enum DiffContent {
    Loading,
    Loaded {
        lines: Rc<Vec<DiffLine>>,
        side_by_side: Rc<Vec<SideBySideRow>>,
        /// The diff was cut at `MAX_DIFF_LINES`; the viewer says so.
        truncated: bool,
    },
    Failed(String),
}

struct FileDiff {
    /// The difference the Previous/Next buttons last jumped to.
    current_difference: Option<usize>,
    change: FileChange,
    /// The commit (after) and its first parent (before).
    oid: Oid,
    parent: Option<Oid>,
    content: DiffContent,
    list_state: UniformListState,
    scroll_state: ScrollStateHandle,
}

#[derive(Default)]
struct MouseStates {
    branch_chip: MouseStateHandle,
    refresh: MouseStateHandle,
    clear_path: MouseStateHandle,
    copy_hash: MouseStateHandle,
    back: MouseStateHandle,
    open_file: MouseStateHandle,
    side_by_side: MouseStateHandle,
    unified: MouseStateHandle,
    head_branch: MouseStateHandle,
    all_branches: MouseStateHandle,
    confirm: MouseStateHandle,
    cancel: MouseStateHandle,
    reset_modes: [MouseStateHandle; 4],
    previous_difference: MouseStateHandle,
    next_difference: MouseStateHandle,
}

pub(crate) struct GitLogView {
    /// The work tree root.
    repo: Option<PathBuf>,
    query: LogQuery,
    /// Set while the text filter is a commit id: the log starts at that commit.
    hash_start: Option<String>,
    stream: Option<LogStream>,
    /// Bumped on every (re)start, so events from an older stream are dropped.
    generation: u64,
    rows: Vec<LogRow>,
    load_state: LoadState,
    /// The widest graph row loaded, in lanes.
    graph_lanes: u16,
    selected: Option<usize>,
    /// A commit to select once it is loaded.
    pending_select: Option<String>,
    /// The pre-reload selection, re-selected silently if it streams in again
    /// (kept across Refresh and filter changes, like IDEA).
    restore_selection: Option<String>,
    details: DetailsState,
    details_generation: u64,
    diff: Option<FileDiff>,
    branches: Vec<RefLabel>,
    menu: ViewHandle<Menu<GitLogAction>>,
    menu_open: bool,
    author_editor: ViewHandle<EditorView>,
    text_editor: ViewHandle<EditorView>,
    filter_change_tx: async_channel::Sender<()>,
    list_state: UniformListState,
    scroll_state: ScrollStateHandle,
    visible: Range<usize>,
    visible_tx: async_channel::Sender<Range<usize>>,
    details_scroll: ClippedScrollStateHandle,
    files_scroll: ClippedScrollStateHandle,
    branches_scroll: ClippedScrollStateHandle,
    branch_mouse_states: Vec<MouseStateHandle>,
    commit_menu: ViewHandle<Menu<GitLogAction>>,
    commit_menu_open: bool,
    pending: Option<PendingAction>,
    name_editor: ViewHandle<EditorView>,
    /// Before/After columns rather than one unified column.
    diff_side_by_side: bool,
    /// Saved position of the selected row, where its context menu opens.
    row_position_id: String,
    row_mouse_states: Vec<MouseStateHandle>,
    lane_colors: Rc<[warpui::color::ColorU]>,
    handle: WeakViewHandle<Self>,
    pane_configuration: ModelHandle<PaneConfiguration>,
    focus_handle: Option<PaneFocusHandle>,
    mouse_states: MouseStates,
}

impl GitLogView {
    pub(crate) fn new(ctx: &mut ViewContext<Self>) -> Self {
        let pane_configuration = ctx.add_model(|_| PaneConfiguration::new(HEADER_TEXT));
        let author_editor = Self::create_filter_editor("User", ctx);
        let text_editor = Self::create_filter_editor("Text or hash", ctx);
        let (filter_change_tx, filter_change_rx) = async_channel::unbounded();
        for editor in [&author_editor, &text_editor] {
            ctx.subscribe_to_view(editor, |me, _, event, _| match event {
                EditorEvent::Edited(_)
                | EditorEvent::BufferReplaced
                | EditorEvent::BufferReinitialized => {
                    let _ = me.filter_change_tx.try_send(());
                }
                _ => {}
            });
        }
        ctx.spawn_stream_local(
            debounce(FILTER_DEBOUNCE, filter_change_rx),
            |me, _, ctx| me.apply_filters(ctx),
            |_, _| {},
        );

        let menu = ctx.add_typed_action_view(|_| {
            Menu::new()
                .with_width(MENU_WIDTH)
                .with_drop_shadow()
                .prevent_interaction_with_other_elements()
        });
        ctx.subscribe_to_view(&menu, |me, _, event, ctx| match event {
            MenuEvent::Close { .. } => {
                me.menu_open = false;
                ctx.notify();
            }
            MenuEvent::ItemSelected | MenuEvent::ItemHovered => {}
        });

        let commit_menu = ctx.add_typed_action_view(|_| {
            Menu::new()
                .with_width(MENU_WIDTH)
                .with_drop_shadow()
                .prevent_interaction_with_other_elements()
        });
        ctx.subscribe_to_view(&commit_menu, |me, _, event, ctx| match event {
            MenuEvent::Close { .. } => {
                me.commit_menu_open = false;
                ctx.notify();
            }
            MenuEvent::ItemSelected | MenuEvent::ItemHovered => {}
        });
        let name_editor = Self::create_filter_editor("Name", ctx);
        ctx.subscribe_to_view(&name_editor, |me, _, event, ctx| match event {
            EditorEvent::Enter => me.handle_action(&GitLogAction::ConfirmPending, ctx),
            EditorEvent::Escape => me.handle_action(&GitLogAction::CancelPending, ctx),
            _ => {}
        });

        let (visible_tx, visible_rx) = async_channel::unbounded();
        ctx.spawn_stream_local(
            visible_rx,
            |me, visible: Range<usize>, _| {
                me.visible = visible;
                me.request_more_if_needed();
            },
            |_, _| {},
        );

        let colors = lane_colors(Appearance::as_ref(ctx));
        ctx.subscribe_to_model(&Appearance::handle(ctx), |me, _, _, ctx| {
            me.lane_colors = lane_colors(Appearance::as_ref(ctx));
            ctx.notify();
        });

        Self {
            repo: None,
            query: LogQuery::default(),
            hash_start: None,
            stream: None,
            generation: 0,
            rows: Vec::new(),
            load_state: LoadState::Idle,
            graph_lanes: 1,
            selected: None,
            pending_select: None,
            restore_selection: None,
            details: DetailsState::None,
            details_generation: 0,
            diff: None,
            branches: Vec::new(),
            menu,
            menu_open: false,
            author_editor,
            text_editor,
            filter_change_tx,
            list_state: UniformListState::new(),
            scroll_state: ScrollStateHandle::default(),
            visible: 0..0,
            visible_tx,
            details_scroll: ClippedScrollStateHandle::default(),
            files_scroll: ClippedScrollStateHandle::default(),
            branches_scroll: ClippedScrollStateHandle::default(),
            branch_mouse_states: Vec::new(),
            commit_menu,
            commit_menu_open: false,
            pending: None,
            name_editor,
            diff_side_by_side: true,
            row_position_id: format!("git_log_selected_row_{}", ctx.view_id()),
            row_mouse_states: Vec::new(),
            lane_colors: colors,
            handle: ctx.handle(),
            pane_configuration,
            focus_handle: None,
            mouse_states: MouseStates::default(),
        }
    }

    fn create_filter_editor(
        placeholder: &'static str,
        ctx: &mut ViewContext<Self>,
    ) -> ViewHandle<EditorView> {
        ctx.add_typed_action_view(|ctx| {
            let appearance = Appearance::as_ref(ctx);
            let options = EditorOptions {
                text: TextOptions::ui_text(Some(FONT_SIZE), appearance),
                select_all_on_focus: true,
                clear_selections_on_blur: true,
                propagate_horizontal_navigation_keys: PropagateHorizontalNavigationKeys::Always,
                single_line: true,
                autogrow: false,
                soft_wrap: false,
                ..Default::default()
            };
            let mut editor = EditorView::new(options, ctx);
            editor.set_placeholder_text(placeholder, ctx);
            editor
        })
    }

    pub(crate) fn pane_configuration(&self) -> ModelHandle<PaneConfiguration> {
        self.pane_configuration.clone()
    }

    pub(crate) fn focus(&mut self, ctx: &mut ViewContext<Self>) {
        ctx.focus_self();
    }

    /// Shows the log of the repository holding `dir`: the history of `file` when given (all of
    /// it otherwise), selecting the commit `select` once it is loaded.
    pub(crate) fn open(
        &mut self,
        dir: PathBuf,
        file: Option<PathBuf>,
        select: Option<String>,
        ctx: &mut ViewContext<Self>,
    ) {
        self.load_state = LoadState::Resolving;
        ctx.notify();
        let receiver = run_in_background("git-toplevel", move || {
            repo_toplevel(&dir).map(|toplevel| {
                let relative = file.and_then(|file| relative_to_repo(&toplevel, &file));
                (toplevel, relative)
            })
        });
        ctx.spawn(
            async move { receiver.await.ok() },
            move |me, result, ctx| match result {
                Some(Ok((toplevel, relative))) => {
                    let same_repo = me.repo.as_deref() == Some(toplevel.as_path());
                    if !same_repo {
                        me.query = LogQuery::default();
                        me.hash_start = None;
                        me.repo = Some(toplevel);
                        me.load_branches(ctx);
                    }
                    let paths: Vec<String> = relative.into_iter().collect();
                    let paths_changed = me.query.paths != paths;
                    me.query.paths = paths;
                    // Only the current repo's rows are a valid match: two
                    // repos can share a commit (forks), and a stale row would
                    // consume the selection before the new repo's log loads.
                    let selected_loaded = same_repo
                        && select
                            .as_deref()
                            .is_some_and(|oid| me.select_commit_if_loaded(oid, ctx));
                    if select.is_some() && !selected_loaded {
                        // The commit may be outside the filtered history: list everything.
                        me.query = LogQuery {
                            revisions: Revisions::All,
                            ..Default::default()
                        };
                        me.hash_start = None;
                        me.clear_filter_editors(ctx);
                    }
                    me.pending_select = if selected_loaded {
                        None
                    } else {
                        select.clone()
                    };
                    let running = me.stream.is_some()
                        && matches!(me.load_state, LoadState::Loading | LoadState::Finished);
                    if !same_repo || paths_changed || !running || me.pending_select.is_some() {
                        me.restart(ctx);
                    }
                    me.update_title(ctx);
                }
                Some(Err(err)) => {
                    me.stop();
                    me.repo = None;
                    me.load_state = LoadState::Failed(format!("Not a git repository: {err}"));
                    me.update_title(ctx);
                    ctx.notify();
                }
                None => {}
            },
        );
    }

    fn clear_filter_editors(&mut self, ctx: &mut ViewContext<Self>) {
        for editor in [&self.author_editor, &self.text_editor] {
            editor.update(ctx, |editor, ctx| editor.set_buffer_text("", ctx));
        }
    }

    fn stop(&mut self) {
        self.stream = None;
        self.generation += 1;
    }

    fn effective_query(&self) -> LogQuery {
        let mut query = self.query.clone();
        if let Some(start) = &self.hash_start {
            query.revisions = Revisions::Rev(start.clone());
        }
        query
    }

    /// Restarts `git log` for the current repository and filters.
    fn restart(&mut self, ctx: &mut ViewContext<Self>) {
        self.stop();
        // Keep the selected commit across the reload (IDEA does): it's
        // re-selected silently if it shows up in the freshly streamed rows.
        // An explicit navigation target (`pending_select`) wins over it.
        self.restore_selection = if self.pending_select.is_some() {
            None
        } else {
            self.selected
                .and_then(|index| self.rows.get(index))
                .map(|row| row.commit.oid.to_hex())
        };
        self.rows.clear();
        self.row_mouse_states.clear();
        self.graph_lanes = 1;
        self.selected = None;
        self.details = DetailsState::None;
        self.diff = None;
        self.list_state = UniformListState::new();
        self.scroll_state = ScrollStateHandle::default();
        // The old visible range would demand that many rows up front from the
        // new stream, breaking the small-first-screen design.
        self.visible = 0..0;
        let Some(repo) = self.repo.clone() else {
            self.load_state = LoadState::Idle;
            ctx.notify();
            return;
        };
        let generation = self.generation;
        let (sender, receiver) = async_channel::unbounded();
        match spawn_log(&repo, &self.effective_query(), INITIAL_ROWS, move |event| {
            let _ = sender.try_send(event);
        }) {
            Ok(stream) => {
                self.stream = Some(stream);
                self.load_state = LoadState::Loading;
            }
            Err(err) => {
                log::warn!("[Git log] Couldn't start git log: {err}");
                self.load_state = LoadState::Failed(err.to_string());
                ctx.notify();
                return;
            }
        }
        ctx.spawn_stream_local(
            receiver,
            move |me, event, ctx| {
                if me.generation == generation {
                    me.on_log_event(event, ctx);
                }
            },
            |_, _| {},
        );
        ctx.notify();
    }

    fn on_log_event(&mut self, event: LogEvent, ctx: &mut ViewContext<Self>) {
        match event {
            LogEvent::Rows(rows) => {
                let first_batch = self.rows.is_empty();
                for row in &rows {
                    self.graph_lanes = self.graph_lanes.max(row.graph.width);
                }
                self.row_mouse_states
                    .extend((0..rows.len()).map(|_| MouseStateHandle::default()));
                self.rows.extend(rows);
                if let Some(pending) = self.pending_select.clone() {
                    if !self.select_commit_if_loaded(&pending, ctx)
                        && let Some(stream) = &self.stream
                    {
                        // Keep reading until the commit shows up.
                        stream.request(self.rows.len() + PAGE_ROWS);
                    }
                } else if let Some(restore) = self.restore_selection.clone() {
                    // Re-select the pre-reload commit if it streamed in; no
                    // extra reads for it (a dropped commit just isn't
                    // restored).
                    if self.select_commit_if_loaded(&restore, ctx) {
                        self.restore_selection = None;
                    }
                } else if first_batch && self.selected.is_none() && !self.rows.is_empty() {
                    self.select_row(0, ctx);
                }
                self.request_more_if_needed();
                ctx.notify();
            }
            LogEvent::Finished { .. } => {
                self.load_state = LoadState::Finished;
                // A selection that didn't stream back in just isn't restored.
                self.restore_selection = None;
                if let Some(pending) = self.pending_select.take() {
                    self.show_toast(
                        format!(
                            "Commit {} isn't in this history",
                            truncate_chars(&pending, 12)
                        ),
                        ctx,
                    );
                }
                ctx.notify();
            }
            LogEvent::Failed(message) => {
                log::info!("[Git log] git log failed: {message}");
                self.load_state = LoadState::Failed(message);
                self.pending_select = None;
                self.restore_selection = None;
                ctx.notify();
            }
        }
    }

    fn request_more_if_needed(&self) {
        if let Some(stream) = &self.stream
            && self.visible.end + PREFETCH_ROWS > self.rows.len()
        {
            stream.request(self.visible.end + PREFETCH_ROWS + PAGE_ROWS);
        }
    }

    /// Selects the loaded commit whose id starts with `oid`, if there is one.
    fn select_commit_if_loaded(&mut self, oid: &str, ctx: &mut ViewContext<Self>) -> bool {
        let Some(index) = self
            .rows
            .iter()
            .position(|row| row.commit.oid.starts_with_hex(oid))
        else {
            return false;
        };
        self.pending_select = None;
        self.select_row(index, ctx);
        self.list_state.scroll_to(index);
        true
    }

    fn load_branches(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        let receiver = run_in_background("git-branches", move || {
            run_git(&repo, &branch_list_args())
                .map(|output| parse_branch_list(&String::from_utf8_lossy(&output)))
        });
        ctx.spawn(
            async move { receiver.await.ok() },
            |me, result, ctx| match result {
                Some(Ok(branches)) => {
                    me.branch_mouse_states = branches
                        .iter()
                        .map(|_| MouseStateHandle::default())
                        .collect();
                    me.branches = branches;
                    ctx.notify();
                    if me.menu_open {
                        me.refresh_menu(ctx);
                    }
                }
                Some(Err(err)) => log::info!("[Git log] Couldn't list branches: {err}"),
                None => {}
            },
        );
    }

    fn apply_filters(&mut self, ctx: &mut ViewContext<Self>) {
        let author = self
            .author_editor
            .as_ref(ctx)
            .buffer_text(ctx)
            .trim()
            .to_owned();
        let text = self
            .text_editor
            .as_ref(ctx)
            .buffer_text(ctx)
            .trim()
            .to_owned();
        let author = (!author.is_empty()).then_some(author);
        if !looks_like_hex_id(&text) {
            let text = (!text.is_empty()).then_some(text);
            if author == self.query.author && text == self.query.text && self.hash_start.is_none() {
                return;
            }
            self.query.author = author;
            self.query.text = text;
            self.hash_start = None;
            self.restart(ctx);
            return;
        }
        let Some(repo) = self.repo.clone() else {
            return;
        };
        // A commit id: start the log at that commit, or search messages when it names none.
        let candidate = text.clone();
        let receiver = run_in_background("git-rev-parse", move || {
            run_git(
                &repo,
                &[
                    "rev-parse".to_owned(),
                    "--verify".to_owned(),
                    "--quiet".to_owned(),
                    format!("{candidate}^{{commit}}"),
                ],
            )
            .ok()
            .map(|output| String::from_utf8_lossy(&output).trim().to_owned())
            .filter(|oid| Oid::from_hex(oid).is_some())
        });
        ctx.spawn(
            async move { receiver.await.ok().flatten() },
            move |me, resolved, ctx| {
                me.query.author = author.clone();
                match resolved {
                    Some(oid) => {
                        me.query.text = None;
                        me.hash_start = Some(oid.clone());
                        me.pending_select = Some(oid);
                    }
                    None => {
                        me.query.text = Some(text.clone());
                        me.hash_start = None;
                    }
                }
                me.restart(ctx);
            },
        );
    }

    fn update_title(&self, ctx: &mut ViewContext<Self>) {
        let title = match self.title_secondary() {
            Some(secondary) => format!("{HEADER_TEXT} · {secondary}"),
            None => HEADER_TEXT.to_owned(),
        };
        self.pane_configuration.update(ctx, |configuration, ctx| {
            configuration.set_title(title, ctx)
        });
    }

    fn title_secondary(&self) -> Option<String> {
        if let Some(path) = self.query.paths.first() {
            return Some(path.clone());
        }
        self.repo
            .as_deref()
            .and_then(Path::file_name)
            .map(|name| name.to_string_lossy().into_owned())
    }

    fn select_row(&mut self, index: usize, ctx: &mut ViewContext<Self>) {
        if index >= self.rows.len() {
            return;
        }
        if self.selected == Some(index) {
            return;
        }
        self.selected = Some(index);
        // A manual selection ends any pending restore.
        self.restore_selection = None;
        self.diff = None;
        self.details_scroll = ClippedScrollStateHandle::default();
        self.load_details(ctx);
        ctx.notify();
    }

    fn load_details(&mut self, ctx: &mut ViewContext<Self>) {
        let (Some(repo), Some(row)) = (
            self.repo.clone(),
            self.selected.and_then(|index| self.rows.get(index)),
        ) else {
            self.details = DetailsState::None;
            return;
        };
        let oid = row.commit.oid;
        let first_parent = row.commit.parents.first().copied();
        self.details_generation += 1;
        let generation = self.details_generation;
        self.details = DetailsState::Loading;
        let receiver = run_in_background("git-show", move || {
            let details = run_git(&repo, &details_args(&oid))
                .map_err(|err| err.to_string())
                .and_then(|output| {
                    parse_details(&String::from_utf8_lossy(&output))
                        .ok_or_else(|| "Couldn't read the commit".to_owned())
                });
            let files = run_git(&repo, &changed_files_args(&oid, first_parent.as_ref()))
                .map(|output| parse_name_status(&output))
                .map_err(|err| err.to_string());
            (details, files)
        });
        ctx.spawn(
            async move { receiver.await.ok() },
            move |me, result, ctx| {
                if me.details_generation != generation {
                    return;
                }
                me.details = match result {
                    Some((Ok(details), files)) => {
                        let count = files.as_ref().map(Vec::len).unwrap_or(0);
                        let tree = files.as_deref().map(file_tree).unwrap_or_default();
                        DetailsState::Loaded(Box::new(LoadedDetails {
                            details,
                            tree,
                            files,
                            file_mouse_states: (0..count)
                                .map(|_| MouseStateHandle::default())
                                .collect(),
                        }))
                    }
                    Some((Err(err), _)) => DetailsState::Failed(err),
                    None => DetailsState::None,
                };
                ctx.notify();
            },
        );
    }

    fn open_file_diff(&mut self, index: usize, ctx: &mut ViewContext<Self>) {
        let DetailsState::Loaded(loaded) = &self.details else {
            return;
        };
        let Some(change) = loaded
            .files
            .as_ref()
            .ok()
            .and_then(|files| files.get(index))
            .cloned()
        else {
            return;
        };
        let Some(repo) = self.repo.clone() else {
            return;
        };
        let oid = loaded.details.oid;
        let first_parent = loaded.details.parents.first().copied();
        self.diff = Some(FileDiff {
            current_difference: None,
            change: change.clone(),
            oid,
            parent: first_parent,
            content: DiffContent::Loading,
            list_state: UniformListState::new(),
            scroll_state: ScrollStateHandle::default(),
        });
        let path = change.path.clone();
        let receiver = run_in_background("git-diff", move || {
            run_git(&repo, &file_diff_args(&oid, first_parent.as_ref(), &change))
                .map(|output| {
                    let mut lines = parse_unified_diff(&String::from_utf8_lossy(&output));
                    let truncated = lines.len() > MAX_DIFF_LINES;
                    lines.truncate(MAX_DIFF_LINES);
                    let rows = side_by_side(&lines);
                    (lines, rows, truncated)
                })
                .map_err(|err| err.to_string())
        });
        ctx.spawn(
            async move { receiver.await.ok() },
            move |me, result, ctx| {
                // Guard on the commit too, not just the path: the same file
                // opened from another commit must not get this stale diff.
                let Some(diff) = me
                    .diff
                    .as_mut()
                    .filter(|diff| diff.change.path == path && diff.oid == oid)
                else {
                    return;
                };
                diff.content = match result {
                    Some(Ok((lines, rows, truncated))) => DiffContent::Loaded {
                        lines: Rc::new(lines),
                        side_by_side: Rc::new(rows),
                        truncated,
                    },
                    Some(Err(err)) => DiffContent::Failed(err),
                    None => DiffContent::Failed("git didn't answer".to_owned()),
                };
                ctx.notify();
            },
        );
        ctx.notify();
    }

    fn refresh_menu(&mut self, ctx: &mut ViewContext<Self>) {
        let mut items = vec![
            MenuItem::Item(
                MenuItemFields::new("Current branch (HEAD)")
                    .with_on_select_action(GitLogAction::SetBranch(BranchChoice::Head)),
            ),
            MenuItem::Item(
                MenuItemFields::new("All branches")
                    .with_on_select_action(GitLogAction::SetBranch(BranchChoice::All)),
            ),
        ];
        items.extend(self.branches.iter().map(|branch| {
            MenuItem::Item(
                MenuItemFields::new(branch.name.clone()).with_on_select_action(
                    GitLogAction::SetBranch(BranchChoice::Branch(branch.name.clone())),
                ),
            )
        }));
        let selected = GitLogAction::SetBranch(self.branch_choice());
        self.menu.update(ctx, |menu, ctx| {
            menu.set_items(items, ctx);
            menu.set_selected_by_action(&selected, ctx);
        });
    }

    fn branch_choice(&self) -> BranchChoice {
        match &self.query.revisions {
            Revisions::Head => BranchChoice::Head,
            Revisions::All => BranchChoice::All,
            Revisions::Rev(rev) => BranchChoice::Branch(rev.clone()),
        }
    }

    fn branch_label(&self) -> String {
        match self.branch_choice() {
            BranchChoice::Head => "Branch: HEAD".to_owned(),
            BranchChoice::All => "Branch: All".to_owned(),
            BranchChoice::Branch(branch) => format!("Branch: {branch}"),
        }
    }

    fn selected_oid(&self) -> Option<Oid> {
        self.selected
            .and_then(|index| self.rows.get(index))
            .map(|row| row.commit.oid)
    }

    fn show_toast(&self, message: String, ctx: &mut ViewContext<Self>) {
        self.show_toast_with(DismissibleToast::error(message), ctx);
    }

    fn show_toast_with(
        &self,
        toast: DismissibleToast<WorkspaceAction>,
        ctx: &mut ViewContext<Self>,
    ) {
        let window_id = ctx.window_id();
        ToastStack::handle(ctx).update(ctx, |stack, ctx| {
            stack.add_ephemeral_toast(toast, window_id, ctx);
        });
    }

    fn open_commit_menu(&mut self, index: usize, ctx: &mut ViewContext<Self>) {
        self.select_row(index, ctx);
        let Some(row) = self.rows.get(index) else {
            return;
        };
        let item = |label: &str, item: CommitMenuItem| {
            MenuItem::Item(
                MenuItemFields::new(label.to_owned())
                    .with_on_select_action(GitLogAction::CommitMenu(item)),
            )
        };
        let mut items = vec![
            item("Copy Revision Number", CommitMenuItem::CopyRevision),
            MenuItem::Separator,
            item("Cherry-Pick", CommitMenuItem::CherryPick),
            item("Checkout Revision", CommitMenuItem::Checkout),
            item("Reset Current Branch to Here…", CommitMenuItem::Reset),
            item("Revert Commit", CommitMenuItem::Revert),
            MenuItem::Separator,
            item("New Branch…", CommitMenuItem::NewBranch),
            item("New Tag…", CommitMenuItem::NewTag),
        ];
        let has_child = self.child_of(index).is_some();
        let has_parent = !row.commit.parents.is_empty();
        if has_child || has_parent {
            items.push(MenuItem::Separator);
        }
        if has_child {
            items.push(item("Go to Child Commit", CommitMenuItem::GoToChild));
        }
        if has_parent {
            items.push(item("Go to Parent Commit", CommitMenuItem::GoToParent));
        }
        self.commit_menu
            .update(ctx, |menu, ctx| menu.set_items(items, ctx));
        self.commit_menu_open = true;
        ctx.focus(&self.commit_menu);
        ctx.notify();
    }

    /// The nearest loaded row above `index` whose parents include that row's commit.
    fn child_of(&self, index: usize) -> Option<usize> {
        let oid = self.rows.get(index)?.commit.oid;
        self.rows[..index]
            .iter()
            .rposition(|row| row.commit.parents.contains(&oid))
    }

    fn handle_commit_menu_item(&mut self, item: &CommitMenuItem, ctx: &mut ViewContext<Self>) {
        self.commit_menu_open = false;
        let Some(row) = self.selected.and_then(|index| self.rows.get(index)) else {
            return;
        };
        let oid = row.commit.oid;
        let short = oid.short();
        let parent = row.commit.parents.first().copied();
        match item {
            CommitMenuItem::CopyRevision => self.handle_action(&GitLogAction::CopyHash, ctx),
            CommitMenuItem::CherryPick => {
                self.run_commit_action(CommitAction::CherryPick, oid, ctx)
            }
            CommitMenuItem::Revert => self.run_commit_action(CommitAction::Revert, oid, ctx),
            CommitMenuItem::Checkout => {
                self.pending = Some(PendingAction::Confirm {
                    action: CommitAction::Checkout,
                    oid,
                    title: format!("Check out {short}?"),
                    message: "HEAD will be detached at this commit. Local changes are kept \
                              unless they conflict."
                        .to_owned(),
                });
            }
            CommitMenuItem::Reset => {
                self.pending = Some(PendingAction::Reset { oid });
            }
            CommitMenuItem::NewBranch | CommitMenuItem::NewTag => {
                self.pending = Some(PendingAction::Name {
                    tag: matches!(item, CommitMenuItem::NewTag),
                    oid,
                    error: None,
                });
                self.name_editor
                    .update(ctx, |editor, ctx| editor.set_buffer_text("", ctx));
                ctx.focus(&self.name_editor);
            }
            CommitMenuItem::GoToChild => {
                if let Some(child) = self.selected.and_then(|index| self.child_of(index)) {
                    self.select_row(child, ctx);
                    self.list_state.scroll_to(child);
                }
            }
            CommitMenuItem::GoToParent => {
                if let Some(parent) = parent {
                    let hex = parent.to_hex();
                    if !self.select_commit_if_loaded(&hex, ctx) {
                        self.pending_select = Some(hex);
                        // `request_more_if_needed` only reads ahead of the
                        // visible rows, so from a scrolled-up position it
                        // would never load the parent: read on explicitly
                        // (the same demand `on_log_event` makes).
                        if let Some(stream) = &self.stream {
                            stream.request(self.rows.len() + PAGE_ROWS);
                        }
                    }
                }
            }
        }
        ctx.notify();
    }

    /// Scrolls the diff to the next or previous change (a hunk header).
    fn jump_to_difference(&mut self, next: bool, ctx: &mut ViewContext<Self>) {
        let side_by_side = self.diff_side_by_side;
        let Some(diff) = &mut self.diff else {
            return;
        };
        let DiffContent::Loaded {
            lines,
            side_by_side: rows,
            ..
        } = &diff.content
        else {
            return;
        };
        let starts: Vec<usize> = if side_by_side {
            rows.iter()
                .enumerate()
                .filter(|(_, row)| {
                    matches!(
                        row,
                        SideBySideRow::Banner {
                            kind: warpium_git::diff::DiffLineKind::Hunk,
                            ..
                        }
                    )
                })
                .map(|(index, _)| index)
                .collect()
        } else {
            lines
                .iter()
                .enumerate()
                .filter(|(_, line)| line.kind == warpium_git::diff::DiffLineKind::Hunk)
                .map(|(index, _)| index)
                .collect()
        };
        if starts.is_empty() {
            return;
        }
        let target = match (diff.current_difference, next) {
            (None, true) => 0,
            (None, false) => starts.len() - 1,
            (Some(current), true) => (current + 1).min(starts.len() - 1),
            (Some(current), false) => current.saturating_sub(1),
        };
        diff.current_difference = Some(target);
        diff.list_state.scroll_to(starts[target]);
        ctx.notify();
    }

    fn confirm_pending(&mut self, ctx: &mut ViewContext<Self>) {
        match self.pending.take() {
            Some(PendingAction::Confirm { action, oid, .. }) => {
                self.run_commit_action(action, oid, ctx)
            }
            Some(PendingAction::Reset { oid }) => {
                // Enter picks Mixed, IDEA's default mode.
                self.run_commit_action(CommitAction::Reset(ResetMode::Mixed), oid, ctx)
            }
            Some(PendingAction::Name { tag, oid, .. }) => {
                let name = self
                    .name_editor
                    .as_ref(ctx)
                    .buffer_text(ctx)
                    .trim()
                    .to_owned();
                if let Some(error) = ref_name_error(&name) {
                    self.pending = Some(PendingAction::Name {
                        tag,
                        oid,
                        error: Some(error.to_owned()),
                    });
                    ctx.notify();
                    return;
                }
                let action = if tag {
                    CommitAction::NewTag(name)
                } else {
                    CommitAction::NewBranch(name)
                };
                self.run_commit_action(action, oid, ctx);
            }
            None => {}
        }
        ctx.focus_self();
        ctx.notify();
    }

    /// Runs a commit action in the background, reports the result and reloads the log.
    fn run_commit_action(&mut self, action: CommitAction, oid: Oid, ctx: &mut ViewContext<Self>) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        let args = action.args(&oid);
        let receiver = run_in_background("git-commit-action", move || run_git(&repo, &args));
        ctx.spawn(
            async move { receiver.await.ok() },
            move |me, result, ctx| match result {
                Some(Ok(_)) => {
                    me.show_toast_with(
                        DismissibleToast::success(action.done_message(&oid.short())),
                        ctx,
                    );
                    me.load_branches(ctx);
                    me.pending_select = Some(oid.to_hex());
                    me.restart(ctx);
                }
                Some(Err(err)) => {
                    log::info!("[Git log] A commit action failed: {err}");
                    me.show_toast(err.to_string(), ctx);
                    me.load_branches(ctx);
                    me.restart(ctx);
                }
                None => {}
            },
        );
    }

    fn status_message(&self) -> Option<(String, bool)> {
        match &self.load_state {
            LoadState::Idle => Some((
                "Show Git Log from the command palette in a tab inside a repository.".to_owned(),
                false,
            )),
            LoadState::Resolving => Some(("Finding the repository…".to_owned(), false)),
            LoadState::Failed(message) => Some((message.clone(), true)),
            LoadState::Loading if self.rows.is_empty() => {
                Some(("Loading commits…".to_owned(), false))
            }
            LoadState::Finished if self.rows.is_empty() => {
                Some(("No commits match the filters.".to_owned(), false))
            }
            LoadState::Loading | LoadState::Finished => None,
        }
    }

    fn render_toolbar(&self, appearance: &Appearance) -> Box<dyn Element> {
        let theme = appearance.theme();
        let branch_chip = chip_button(
            self.mouse_states.branch_chip.clone(),
            self.branch_label(),
            Some(Icon::GitBranch),
            self.menu_open,
            appearance,
        )
        .build()
        .on_click(|ctx, _, _| ctx.dispatch_typed_action(GitLogAction::ToggleBranchMenu))
        .finish();
        // JetBrains' order: the text or hash search, then the Branch, User and Paths filters.
        let mut row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(
                Shrinkable::new(
                    2.,
                    self.render_filter_input(&self.text_editor, FilterField::Text, appearance),
                )
                .finish(),
            )
            .with_child(Container::new(branch_chip).with_margin_left(6.).finish())
            .with_child(
                Shrinkable::new(
                    1.,
                    self.render_filter_input(&self.author_editor, FilterField::Author, appearance),
                )
                .finish(),
            );
        if let Some(path) = self.query.paths.first() {
            row.add_child(self.render_path_chip(path, appearance));
        }
        row.add_child(
            Container::new(self.tool_button(
                Icon::Refresh,
                "Refresh",
                self.mouse_states.refresh.clone(),
                GitLogAction::Refresh,
                appearance,
            ))
            .with_margin_left(4.)
            .finish(),
        );
        Container::new(row.finish())
            .with_horizontal_padding(8.)
            .with_vertical_padding(4.)
            .with_border(Border::bottom(1.).with_border_fill(theme.outline()))
            .finish()
    }

    fn render_path_chip(&self, path: &str, appearance: &Appearance) -> Box<dyn Element> {
        let theme = appearance.theme();
        let label = Text::new_inline(
            format!("Paths: {path}"),
            appearance.ui_font_family(),
            FONT_SIZE,
        )
        .with_color(theme.main_text_color(theme.background()).into_solid())
        .with_clip(ClipConfig::start())
        .finish();
        let clear = self.tool_button(
            Icon::X,
            "Show the whole history",
            self.mouse_states.clear_path.clone(),
            GitLogAction::ClearPath,
            appearance,
        );
        Container::new(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(
                    Shrinkable::new(1., ConstrainedBox::new(label).with_max_width(260.).finish())
                        .finish(),
                )
                .with_child(clear)
                .finish(),
        )
        .with_padding_left(8.)
        .with_margin_left(6.)
        .with_background(theme.surface_2())
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.)))
        .finish()
    }

    fn tool_button(
        &self,
        icon: Icon,
        tooltip: &'static str,
        mouse_state: MouseStateHandle,
        action: GitLogAction,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let ui_builder = appearance.ui_builder().clone();
        icon_button(appearance, icon, false, mouse_state)
            .with_tooltip(move || ui_builder.tool_tip(tooltip.to_owned()).build().finish())
            .build()
            .on_click(move |ctx, _, _| ctx.dispatch_typed_action(action.clone()))
            .finish()
    }

    fn render_filter_input(
        &self,
        editor: &ViewHandle<EditorView>,
        field: FilterField,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let theme = appearance.theme();
        let input = Container::new(
            ConstrainedBox::new(Clipped::new(ChildView::new(editor).finish()).finish())
                .with_height(CONTROL_HEIGHT - 8.)
                // Keep the fields usable in a narrow pane instead of letting
                // them collapse to empty boxes.
                .with_min_width(100.)
                .finish(),
        )
        .with_padding(Padding::uniform(4.))
        .with_border(Border::all(1.).with_border_fill(theme.surface_3()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.)))
        .with_margin_left(6.)
        .finish();
        EventHandler::new(input)
            .on_left_mouse_down(move |ctx, _, _| {
                ctx.dispatch_typed_action(GitLogAction::FocusEditor(field));
                DispatchEventResult::StopPropagation
            })
            .finish()
    }

    fn render_status(
        &self,
        message: &str,
        is_error: bool,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let theme = appearance.theme();
        let color = if is_error {
            theme.ui_error_color()
        } else {
            theme.sub_text_color(theme.background()).into_solid()
        };
        Container::new(
            Text::new(message.to_owned(), appearance.ui_font_family(), FONT_SIZE)
                .with_color(color)
                .finish(),
        )
        .with_horizontal_padding(10.)
        .with_vertical_padding(6.)
        .finish()
    }

    fn render_list(&self, appearance: &Appearance) -> Box<dyn Element> {
        let theme = appearance.theme();
        let handle = self.handle.clone();
        let build_items = move |range: Range<usize>, app: &AppContext| {
            match handle.upgrade(app) {
                Some(view) => view.as_ref(app).render_rows(range, app),
                None => Vec::new(),
            }
            .into_iter()
        };
        let list = UniformList::new(self.list_state.clone(), self.rows.len(), build_items)
            .notify_visible_items(self.visible_tx.clone());
        // An empty `UniformList` reports zero visible and total size, which makes `Scrollable`
        // compute a NaN scrollbar thumb and trip the scene's rect validation.
        if self.rows.is_empty() {
            return list.finish();
        }
        Scrollable::vertical(
            self.scroll_state.clone(),
            list.finish_scrollable(),
            ScrollbarWidth::Auto,
            theme.nonactive_ui_detail().into(),
            theme.active_ui_detail().into(),
            Fill::None,
        )
        .with_overlayed_scrollbar()
        .finish()
    }

    fn render_rows(&self, range: Range<usize>, ctx: &AppContext) -> Vec<Box<dyn Element>> {
        let appearance = Appearance::as_ref(ctx);
        range
            .filter_map(|index| {
                let row = self.rows.get(index)?;
                Some(self.render_row(index, row, appearance))
            })
            .collect()
    }

    fn render_row(&self, index: usize, row: &LogRow, appearance: &Appearance) -> Box<dyn Element> {
        let theme = appearance.theme();
        let font = appearance.ui_font_family();
        let main_color = theme.main_text_color(theme.background()).into_solid();
        let sub_color = theme.sub_text_color(theme.background()).into_solid();
        let commit = &row.commit;
        let is_head = commit.refs.iter().any(|label| label.is_head);
        let graph_width = self.graph_lanes.max(1) as f32 * LANE_WIDTH + 4.;
        let graph = ConstrainedBox::new(
            GraphCell::new(
                row.graph.clone(),
                self.lane_colors.clone(),
                LANE_WIDTH,
                is_head,
                theme.background().into_solid(),
            )
            .finish(),
        )
        .with_width(graph_width)
        .with_height(ROW_HEIGHT)
        .finish();

        let mut subject = Flex::row().with_cross_axis_alignment(CrossAxisAlignment::Center);
        for label in commit.refs.iter().take(4) {
            subject.add_child(render_ref_label(label, appearance));
        }
        let subject_weight = if is_head {
            Weight::Bold
        } else {
            Weight::Normal
        };
        subject.add_child(
            Shrinkable::new(
                1.,
                Text::new_inline(commit.subject.clone(), font, FONT_SIZE)
                    .with_color(main_color)
                    .with_style(Properties::default().weight(subject_weight))
                    .with_clip(ClipConfig::end())
                    .finish(),
            )
            .finish(),
        );
        // Author and date cap at their preferred widths but shrink (clipped)
        // when the pane is narrow, so the subject keeps its space — IDEA
        // shrinks these columns first, not the subject.
        let shrinkable_column = |text: String, max_width: f32, color| {
            Shrinkable::new(
                1.,
                ConstrainedBox::new(
                    Container::new(
                        Text::new_inline(text, font, FONT_SIZE)
                            .with_color(color)
                            .with_clip(ClipConfig::end())
                            .finish(),
                    )
                    .with_padding_left(8.)
                    .finish(),
                )
                .with_max_width(max_width)
                .finish(),
            )
            .finish()
        };
        let content = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(graph)
            .with_child(Expanded::new(6., Clipped::new(subject.finish()).finish()).finish())
            .with_child(shrinkable_column(
                commit.author_name.clone(),
                AUTHOR_WIDTH,
                sub_color,
            ))
            .with_child(shrinkable_column(
                local_date_time(commit.author_time),
                DATE_WIDTH,
                sub_color,
            ))
            .finish();
        let selected = self.selected == Some(index);
        let selected_fill = theme.surface_2();
        let hover_fill = theme.surface_1();
        let mouse_state = self
            .row_mouse_states
            .get(index)
            .cloned()
            .unwrap_or_default();
        let row = Hoverable::new(mouse_state, move |state| {
            let mut container = Container::new(
                ConstrainedBox::new(content)
                    .with_height(ROW_HEIGHT)
                    .finish(),
            )
            .with_padding_left(4.)
            .with_padding_right(8.);
            if selected {
                container = container.with_background(selected_fill);
            } else if state.is_hovered() {
                container = container.with_background(hover_fill);
            }
            container.finish()
        })
        .on_click(move |ctx, _, _| ctx.dispatch_typed_action(GitLogAction::SelectRow(index)))
        .on_right_click(move |ctx, _, _| {
            ctx.dispatch_typed_action(GitLogAction::OpenCommitMenu(index))
        })
        .finish();
        if selected {
            SavePosition::new(row, &self.row_position_id).finish()
        } else {
            row
        }
    }
}

fn diff_title(change: &FileChange) -> String {
    match change.kind.old_path() {
        Some(old) => format!("{old} → {}", change.path),
        None => change.path.clone(),
    }
}

fn render_ref_label(label: &RefLabel, appearance: &Appearance) -> Box<dyn Element> {
    let theme = appearance.theme();
    let color = match label.kind {
        _ if label.is_head => theme.accent().into_solid(),
        RefKind::Head => theme.accent().into_solid(),
        RefKind::LocalBranch => theme.ansi_fg_green(),
        RefKind::RemoteBranch => theme.ansi_fg_magenta(),
        RefKind::Tag => theme.ansi_fg_yellow(),
        RefKind::Other => theme.hint_text_color(theme.background()).into_solid(),
    };
    let name = truncate_chars(&label.name, 28);
    Container::new(
        Text::new_inline(name, appearance.ui_font_family(), FONT_SIZE - 1.)
            .with_color(color)
            .finish(),
    )
    .with_horizontal_padding(4.)
    .with_margin_right(4.)
    .with_border(Border::all(1.).with_border_color(color))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(3.)))
    .finish()
}

/// Lane colors, cycled by the graph's color index.
fn lane_colors(appearance: &Appearance) -> Rc<[warpui::color::ColorU]> {
    let theme = appearance.theme();
    Rc::from(vec![
        theme.ansi_fg_blue(),
        theme.ansi_fg_green(),
        theme.ansi_fg_magenta(),
        theme.ansi_fg_yellow(),
        theme.ansi_fg_cyan(),
        theme.ansi_fg_red(),
    ])
}

impl Entity for GitLogView {
    type Event = GitLogViewEvent;
}

impl View for GitLogView {
    fn ui_name() -> &'static str {
        "GitLogView"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let body: Box<dyn Element> = match &self.diff {
            Some(diff) => self.render_diff_viewer(diff, appearance),
            None => {
                let mut log = Flex::column().with_child(self.render_toolbar(appearance));
                if let Some((message, is_error)) = self.status_message() {
                    log.add_child(self.render_status(&message, is_error, appearance));
                }
                log.add_child(Expanded::new(1., self.render_list(appearance)).finish());
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                    .with_child(
                        ConstrainedBox::new(
                            Container::new(self.render_branches(appearance))
                                .with_border(Border::right(1.).with_border_fill(theme.outline()))
                                .finish(),
                        )
                        .with_width(BRANCHES_WIDTH)
                        .finish(),
                    )
                    .with_child(Expanded::new(3., log.finish()).finish())
                    .with_child(
                        Expanded::new(
                            2.,
                            Container::new(self.render_commit_panel(appearance))
                                .with_border(Border::left(1.).with_border_fill(theme.outline()))
                                .finish(),
                        )
                        .finish(),
                    )
                    .finish()
            }
        };
        let content = Container::new(body)
            .with_background(theme.background())
            .finish();

        let mut stack = Stack::new();
        stack.add_child(content);
        if self.menu_open {
            stack.add_positioned_overlay_child(
                ChildView::new(&self.menu).finish(),
                OffsetPositioning::offset_from_parent(
                    warpui::geometry::vector::vec2f(BRANCHES_WIDTH + 8., 34.),
                    ParentOffsetBounds::ParentByPosition,
                    ParentAnchor::TopLeft,
                    ChildAnchor::TopLeft,
                ),
            );
        }
        if self.commit_menu_open {
            stack.add_positioned_overlay_child(
                ChildView::new(&self.commit_menu).finish(),
                self.commit_menu_positioning(),
            );
        }
        if let Some(pending) = &self.pending {
            stack.add_positioned_overlay_child(
                self.render_pending(pending, appearance),
                OffsetPositioning::offset_from_parent(
                    warpui::geometry::vector::vec2f(0., 40.),
                    ParentOffsetBounds::ParentByPosition,
                    ParentAnchor::TopMiddle,
                    ChildAnchor::TopMiddle,
                ),
            );
        }
        stack.finish()
    }
}

impl TypedActionView for GitLogView {
    type Action = GitLogAction;

    fn handle_action(&mut self, action: &GitLogAction, ctx: &mut ViewContext<Self>) {
        match action {
            GitLogAction::ToggleBranchMenu => {
                self.menu_open = !self.menu_open;
                if self.menu_open {
                    self.refresh_menu(ctx);
                    ctx.focus(&self.menu);
                }
                ctx.notify();
            }
            GitLogAction::SetBranch(choice) => {
                self.menu_open = false;
                self.query.revisions = match choice {
                    BranchChoice::Head => Revisions::Head,
                    BranchChoice::All => Revisions::All,
                    BranchChoice::Branch(branch) => Revisions::Rev(branch.clone()),
                };
                self.hash_start = None;
                self.restart(ctx);
            }
            GitLogAction::Refresh => {
                self.load_branches(ctx);
                self.restart(ctx);
            }
            GitLogAction::ClearPath => {
                self.query.paths.clear();
                self.update_title(ctx);
                self.restart(ctx);
            }
            GitLogAction::SelectRow(index) => {
                self.select_row(*index, ctx);
                ctx.focus_self();
            }
            GitLogAction::SelectBy(delta) => {
                if self.rows.is_empty() {
                    return;
                }
                let current = self.selected.unwrap_or(0) as i64;
                let target = (current + *delta as i64).clamp(0, self.rows.len() as i64 - 1);
                self.select_row(target as usize, ctx);
                self.list_state.scroll_to(target as usize);
                self.request_more_if_needed();
            }
            GitLogAction::CopyHash => {
                if let Some(oid) = self.selected_oid() {
                    ctx.clipboard()
                        .write(ClipboardContent::plain_text(oid.to_hex()));
                }
            }
            GitLogAction::OpenFileDiff(index) => self.open_file_diff(*index, ctx),
            GitLogAction::CloseDiff => {
                if self.diff.take().is_some() {
                    ctx.notify();
                }
            }
            GitLogAction::OpenWorkingFile(path) => {
                if let Some(repo) = &self.repo {
                    ctx.dispatch_typed_action_deferred(WorkspaceAction::OpenFileInNewTab {
                        full_path: repo.join(path),
                        line_and_column: None,
                    });
                }
            }
            GitLogAction::OpenCommitMenu(index) => self.open_commit_menu(*index, ctx),
            GitLogAction::CommitMenu(item) => self.handle_commit_menu_item(item, ctx),
            GitLogAction::ConfirmPending => self.confirm_pending(ctx),
            GitLogAction::ConfirmReset(mode) => {
                if let Some(PendingAction::Reset { oid }) = self.pending.take() {
                    self.run_commit_action(CommitAction::Reset(*mode), oid, ctx);
                }
                ctx.focus_self();
                ctx.notify();
            }
            GitLogAction::JumpToDifference(next) => self.jump_to_difference(*next, ctx),
            GitLogAction::CancelPending => {
                self.pending = None;
                ctx.focus_self();
                ctx.notify();
            }
            GitLogAction::ToggleDiffMode => {
                self.diff_side_by_side = !self.diff_side_by_side;
                if let Some(diff) = &mut self.diff {
                    diff.list_state = UniformListState::new();
                    diff.scroll_state = ScrollStateHandle::default();
                    diff.current_difference = None;
                }
                ctx.notify();
            }
            GitLogAction::FocusEditor(field) => {
                let editor = match field {
                    FilterField::Author => &self.author_editor,
                    FilterField::Text => &self.text_editor,
                };
                ctx.focus(editor);
            }
        }
    }
}

impl BackingView for GitLogView {
    type PaneHeaderOverflowMenuAction = GitLogOverflowAction;
    type CustomAction = ();
    type AssociatedData = ();

    fn handle_pane_header_overflow_menu_action(
        &mut self,
        action: &Self::PaneHeaderOverflowMenuAction,
        ctx: &mut ViewContext<Self>,
    ) {
        match action {
            GitLogOverflowAction::Refresh => self.handle_action(&GitLogAction::Refresh, ctx),
            GitLogOverflowAction::CopyHash => self.handle_action(&GitLogAction::CopyHash, ctx),
        }
    }

    fn pane_header_overflow_menu_items(
        &self,
        _ctx: &AppContext,
    ) -> Vec<MenuItem<Self::PaneHeaderOverflowMenuAction>> {
        vec![
            MenuItem::Item(
                MenuItemFields::new("Refresh").with_on_select_action(GitLogOverflowAction::Refresh),
            ),
            MenuItem::Item(
                MenuItemFields::new("Copy commit hash")
                    .with_on_select_action(GitLogOverflowAction::CopyHash),
            ),
        ]
    }

    fn close(&mut self, ctx: &mut ViewContext<Self>) {
        ctx.emit(GitLogViewEvent::Pane(PaneEvent::Close));
    }

    fn focus_contents(&mut self, ctx: &mut ViewContext<Self>) {
        self.focus(ctx);
    }

    fn render_header_content(
        &self,
        _ctx: &view::HeaderRenderContext<'_>,
        _app: &AppContext,
    ) -> HeaderContent {
        HeaderContent::Standard(StandardHeader {
            title: HEADER_TEXT.to_owned(),
            title_secondary: self.title_secondary(),
            title_style: None,
            title_clip_config: ClipConfig::start(),
            title_max_width: None,
            left_of_title: None,
            right_of_title: None,
            left_of_overflow: None,
            options: StandardHeaderOptions::default(),
        })
    }

    fn set_focus_handle(&mut self, focus_handle: PaneFocusHandle, _ctx: &mut ViewContext<Self>) {
        self.focus_handle = Some(focus_handle);
    }
}

#[path = "git_log_panels.rs"]
mod panels;

#[cfg(test)]
#[path = "git_log_tests.rs"]
mod tests;
