//! Warpium: Find in Files (roadmap item 22) — an IDEA Find in Path–style popup on cmd-shift-F.
//!
//! The popup searches the active tab's project with Warp's global search engine
//! (`workspace/view/global_search`), grouped-by-file results in a virtualized list. The workspace
//! owns the view and computes its scope (`workspace/view/warpium_find_in_files.rs`); the pure
//! decision logic (pattern wrapping, filters, row flattening, the summary) is unit-tested in
//! `find_in_files_tests.rs`.

use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_channel::Sender;
use pathfinder_geometry::vector::vec2f;
use warp_core::features::FeatureFlag;
use warp_core::r#async::debounce;
use warp_core::ui::appearance::Appearance;
use warp_core::ui::theme::AnsiColorIdentifier;
use warp_core::ui::theme::Fill as ThemeFill;
use warp_editor::editor::NavigationKey;
use warp_ripgrep::filter::{PathFilter, SearchFilters, parse_glob_list};
use warp_ripgrep::search::Submatch;
use warp_util::local_or_remote_path::LocalOrRemotePath;
use warpui::elements::{
    Align, Border, ChildAnchor, ChildView, Clipped, ConstrainedBox, Container, CornerRadius,
    CrossAxisAlignment, DispatchEventResult, Element, Empty, EventHandler, Fill, Flex, Highlight,
    Hoverable, MainAxisAlignment, MainAxisSize, MouseStateHandle, OffsetPositioning, Padding,
    ParentAnchor, ParentElement, ParentOffsetBounds, Radius, ScrollStateHandle, Scrollable,
    ScrollableElement, ScrollbarWidth, Shrinkable, Stack, Text, UniformList, UniformListState,
};
use warpui::keymap::macros::*;
use warpui::keymap::{EditableBinding, FixedBinding};
use warpui::platform::Cursor;
use warpui::text_layout::TextStyle;
use warpui::{
    AppContext, Entity, ModelHandle, SingletonEntity, TypedActionView, View, ViewContext,
    ViewHandle, WeakViewHandle,
};

use crate::code::icon_from_file_path;
use crate::editor::{
    EditorView, Event as EditorEvent, PropagateAndNoOpNavigationKeys,
    PropagateHorizontalNavigationKeys, SingleLineEditorOptions, TextOptions,
};
use crate::search::ItemHighlightState as SearchHighlightState;
use crate::settings_view::flags;
use crate::ui_components::blended_colors;
use crate::ui_components::icons::Icon as UiIcon;
use crate::ui_components::item_highlight::{ImageOrIcon, ItemHighlightState};
use crate::ui_components::render_file_search_row::{FileSearchRowOptions, render_file_search_row};
use crate::util::bindings::BindingGroup;
use crate::view_components::action_button::{ActionButton, ButtonSize, NakedTheme};
use crate::workspace::WorkspaceAction;
use crate::workspace::view::global_search::model::GlobalSearch;
use crate::workspace::view::global_search::view::GlobalSearchEvent;
use crate::workspace::view::global_search::{GlobalSearchMatch, SearchConfig};

const QUERY_DEBOUNCE_PERIOD: Duration = Duration::from_millis(300);
/// Matches shown in the results list; the summary still reports the engine's full count.
const MAX_DISPLAYED_MATCHES: usize = 1000;
const POPUP_WIDTH: f32 = 760.;
const ROW_HEIGHT: f32 = 24.;
const MAX_VISIBLE_ROWS: usize = 14;
const PAGE_ROWS: isize = 10;

pub(crate) fn init_bindings(app: &mut AppContext) {
    // cmd-shift-F is Find in Files (the roadmap reserves it); global search keeps its palette
    // command, its ctrl-3 toggle and alt-shift-F on Linux/Windows.
    app.register_editable_bindings([
        EditableBinding::new(
            "workspace:open_find_in_files",
            "Find in Files",
            WorkspaceAction::OpenFindInFiles,
        )
        .with_group(BindingGroup::Navigation.as_str())
        .with_context_predicate(id!("Workspace") & id!(flags::SHOW_GLOBAL_SEARCH))
        .with_enabled(|| FeatureFlag::GlobalSearch.is_enabled())
        .with_mac_key_binding("cmd-shift-F"),
    ]);

    app.register_fixed_bindings([
        FixedBinding::new(
            "up",
            FindInFilesAction::MoveSelectionUp,
            id!(FindInFilesView::ui_name()),
        ),
        FixedBinding::new(
            "down",
            FindInFilesAction::MoveSelectionDown,
            id!(FindInFilesView::ui_name()),
        ),
        FixedBinding::new(
            "pageup",
            FindInFilesAction::MoveSelectionPageUp,
            id!(FindInFilesView::ui_name()),
        ),
        FixedBinding::new(
            "pagedown",
            FindInFilesAction::MoveSelectionPageDown,
            id!(FindInFilesView::ui_name()),
        ),
        FixedBinding::new(
            "enter",
            FindInFilesAction::OpenSelected,
            id!(FindInFilesView::ui_name()),
        ),
        FixedBinding::new(
            "escape",
            FindInFilesAction::Close,
            id!(FindInFilesView::ui_name()),
        ),
    ]);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FindInFilesAction {
    Close,
    MoveSelectionUp,
    MoveSelectionDown,
    MoveSelectionPageUp,
    MoveSelectionPageDown,
    OpenSelected,
    ToggleMatchCase,
    ToggleWholeWords,
    ToggleRegex,
    ToggleIgnoreFiles,
    SelectRow(usize),
    OpenRow(usize),
}

pub(crate) enum FindInFilesEvent {
    Close,
    OpenMatch {
        path: PathBuf,
        line_number: u32,
        column_num: Option<usize>,
    },
}

/// The pattern and regex flag handed to the search engine. The Words toggle wraps the query in
/// word boundaries and so always searches with a regex; the engine escapes non-regex queries
/// itself.
pub(crate) fn search_pattern(query: &str, whole_words: bool, regex: bool) -> (String, bool) {
    if !whole_words {
        return (query.to_owned(), regex);
    }
    if regex {
        (format!("\\b(?:{query})\\b"), true)
    } else {
        (format!("\\b{}\\b", regex::escape(query)), true)
    }
}

/// The engine's filters for the file mask input: comma-separated include globs (`!` negates).
pub(crate) fn search_filters(mask: &str, use_ignore_files: bool) -> SearchFilters {
    SearchFilters {
        use_ignore_files,
        include_globs: parse_glob_list(mask),
        exclude_globs: Vec::new(),
    }
}

/// Whether `path` may be searched: never `$HOME` or `/`.
pub(crate) fn is_searchable_root(path: &Path, home: Option<&Path>) -> bool {
    path.parent().is_some() && Some(path) != home
}

/// A row of the flattened results list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ResultRow {
    FileHeader { group: usize },
    Match { group: usize, index: usize },
}

/// The results list's rows: each file's header followed by its match rows.
pub(crate) fn flatten_rows(group_match_counts: &[usize]) -> Vec<ResultRow> {
    let mut rows = Vec::new();
    for (group, &match_count) in group_match_counts.iter().enumerate() {
        rows.push(ResultRow::FileHeader { group });
        rows.extend((0..match_count).map(|index| ResultRow::Match { group, index }));
    }
    rows
}

/// The footer's summary. `displayed_matches` is what the list shows (capped), `total_matches`
/// what the engine found (or has found so far while searching).
pub(crate) fn results_summary(
    total_matches: usize,
    file_count: usize,
    displayed_matches: usize,
    is_search_in_progress: bool,
) -> String {
    if total_matches == 0 {
        return if is_search_in_progress {
            "Searching…".to_owned()
        } else {
            "No matches found".to_owned()
        };
    }
    let match_word = if total_matches == 1 { "match" } else { "matches" };
    let file_word = if file_count == 1 { "file" } else { "files" };
    let mut summary = format!("{total_matches} {match_word} in {file_count} {file_word}");
    if displayed_matches < total_matches {
        summary.push_str(&format!(" — showing first {displayed_matches}"));
    }
    summary
}

/// One match within a file.
struct MatchRow {
    line_number: u32,
    column_num: Option<usize>,
    line_text: String,
    submatches: Vec<Submatch>,
    mouse_state: MouseStateHandle,
}

/// A file's matches, shown under a file-header row.
struct FileGroup {
    path: LocalOrRemotePath,
    /// The path relative to its search root, for the header row.
    display_path: PathBuf,
    mouse_state: MouseStateHandle,
    matches: Vec<MatchRow>,
}

pub(crate) struct FindInFilesView {
    open: bool,
    find_model: ModelHandle<GlobalSearch>,
    query_editor: ViewHandle<EditorView>,
    mask_editor: ViewHandle<EditorView>,
    query_change_tx: Sender<()>,
    match_case_button: ViewHandle<ActionButton>,
    whole_words_button: ViewHandle<ActionButton>,
    regex_button: ViewHandle<ActionButton>,
    ignore_files_button: ViewHandle<ActionButton>,
    match_case: bool,
    whole_words: bool,
    regex: bool,
    use_ignore_files: bool,
    /// The local folders searched, set by the workspace on open.
    roots: Vec<LocalOrRemotePath>,
    scope_label: String,
    groups: Vec<FileGroup>,
    group_index_by_path: HashMap<LocalOrRemotePath, usize>,
    rows: Vec<ResultRow>,
    selected_row: Option<usize>,
    /// Matches the engine reported for the finished search.
    total_match_count: usize,
    /// Matches in the list (at most [`MAX_DISPLAYED_MATCHES`]).
    displayed_match_count: usize,
    is_search_in_progress: bool,
    last_error: Option<String>,
    current_search_id: Option<u32>,
    last_searched_pattern: Option<String>,
    last_searched_filters: Option<SearchFilters>,
    /// Whether the results list (rather than an editor) has keyboard focus.
    focus_results: bool,
    uniform_list_state: UniformListState,
    scroll_state: ScrollStateHandle,
    handle: WeakViewHandle<FindInFilesView>,
}

impl Entity for FindInFilesView {
    type Event = FindInFilesEvent;
}

impl FindInFilesView {
    pub(crate) fn new(ctx: &mut ViewContext<Self>) -> Self {
        let find_model = ctx.add_model(|_| GlobalSearch::new());

        let query_editor = ctx.add_typed_action_view(|ctx| {
            let appearance = Appearance::as_ref(ctx);
            let options = SingleLineEditorOptions {
                text: TextOptions::ui_text(Some(14.), appearance),
                select_all_on_focus: true,
                clear_selections_on_blur: true,
                propagate_and_no_op_vertical_navigation_keys: PropagateAndNoOpNavigationKeys::Always,
                propagate_horizontal_navigation_keys: PropagateHorizontalNavigationKeys::Always,
                ..Default::default()
            };
            let mut editor = EditorView::single_line(options, ctx);
            editor.set_placeholder_text("Find in files", ctx);
            editor
        });

        let mask_editor = ctx.add_typed_action_view(|ctx| {
            let appearance = Appearance::as_ref(ctx);
            let options = SingleLineEditorOptions {
                text: TextOptions::ui_text(Some(13.), appearance),
                select_all_on_focus: true,
                clear_selections_on_blur: true,
                propagate_and_no_op_vertical_navigation_keys: PropagateAndNoOpNavigationKeys::Always,
                propagate_horizontal_navigation_keys: PropagateHorizontalNavigationKeys::Always,
                ..Default::default()
            };
            let mut editor = EditorView::single_line(options, ctx);
            editor.set_placeholder_text("File mask (e.g. *.{kt,kts}, !*Test*)", ctx);
            editor
        });

        let (query_change_tx, query_change_rx) = async_channel::unbounded();
        ctx.spawn_stream_local(
            debounce(QUERY_DEBOUNCE_PERIOD, query_change_rx),
            Self::handle_debounced_query_change,
            |_, _| {},
        );

        let toggle_button = |icon: Option<UiIcon>,
                             label: &str,
                             tooltip: &str,
                             action: FindInFilesAction,
                             ctx: &mut ViewContext<Self>| {
            let button = ActionButton::new_with_boxed_theme(label.to_owned(), Arc::new(NakedTheme))
                .with_tooltip(tooltip)
                .with_size(ButtonSize::Small)
                .on_click(move |ctx| {
                    ctx.dispatch_typed_action(action.clone());
                });
            let button = match icon {
                Some(icon) => button.with_icon(icon),
                None => button,
            };
            ctx.add_typed_action_view(|_| button)
        };

        let match_case_button = toggle_button(
            Some(UiIcon::CaseSensitivity),
            "",
            "Match Case",
            FindInFilesAction::ToggleMatchCase,
            ctx,
        );
        let whole_words_button =
            toggle_button(None, "ab", "Match Whole Words", FindInFilesAction::ToggleWholeWords, ctx);
        let regex_button = toggle_button(
            Some(UiIcon::Regex),
            "",
            "Regular Expression",
            FindInFilesAction::ToggleRegex,
            ctx,
        );
        let ignore_files_button = toggle_button(
            Some(UiIcon::FilterFunnel),
            "",
            "Use .gitignore and ignore files",
            FindInFilesAction::ToggleIgnoreFiles,
            ctx,
        );
        ignore_files_button.update(ctx, |button, ctx| {
            button.set_active(true, ctx);
        });

        ctx.subscribe_to_view(&query_editor, |me, _, event, ctx| {
            me.handle_query_editor_event(event, ctx);
        });
        ctx.subscribe_to_view(&mask_editor, |me, _, event, ctx| {
            me.handle_mask_editor_event(event, ctx);
        });
        ctx.subscribe_to_model(&find_model, |me, _, event, ctx| {
            me.handle_search_event(event, ctx);
        });

        Self {
            open: false,
            find_model,
            query_editor,
            mask_editor,
            query_change_tx,
            match_case_button,
            whole_words_button,
            regex_button,
            ignore_files_button,
            match_case: false,
            whole_words: false,
            regex: false,
            use_ignore_files: true,
            roots: Vec::new(),
            scope_label: String::new(),
            groups: Vec::new(),
            group_index_by_path: HashMap::new(),
            rows: Vec::new(),
            selected_row: None,
            total_match_count: 0,
            displayed_match_count: 0,
            is_search_in_progress: false,
            last_error: None,
            current_search_id: None,
            last_searched_pattern: None,
            last_searched_filters: None,
            focus_results: false,
            uniform_list_state: UniformListState::new(),
            scroll_state: ScrollStateHandle::default(),
            handle: ctx.handle(),
        }
    }

    pub(crate) fn is_open(&self) -> bool {
        self.open
    }

    /// Shows the popup over `roots`, pre-filling the query with the current selection.
    pub(crate) fn open(
        &mut self,
        roots: Vec<LocalOrRemotePath>,
        scope_label: String,
        selected_text: Option<String>,
        ctx: &mut ViewContext<Self>,
    ) {
        self.open = true;
        self.roots = roots;
        self.scope_label = scope_label;
        self.focus_results = false;
        if let Some(text) = selected_text {
            self.query_editor.update(ctx, |editor, ctx| {
                editor.set_buffer_text(&text, ctx);
                editor.select_all(ctx);
            });
        }
        ctx.focus(&self.query_editor);
        // Search right away: the roots may have changed since the popup was last open.
        self.rerun_search(true, ctx);
        ctx.notify();
    }

    fn close(&mut self, ctx: &mut ViewContext<Self>) {
        if !self.open {
            return;
        }
        self.dismiss(ctx);
        ctx.emit(FindInFilesEvent::Close);
    }

    /// Hides the popup and aborts the in-flight search, without emitting an event.
    fn dismiss(&mut self, ctx: &mut ViewContext<Self>) {
        self.open = false;
        self.focus_results = false;
        self.is_search_in_progress = false;
        self.current_search_id = None;
        self.find_model.update(ctx, |model, model_ctx| {
            model.abort_search(model_ctx);
        });
        ctx.notify();
    }

    fn handle_debounced_query_change(&mut self, _event: (), ctx: &mut ViewContext<Self>) {
        self.rerun_search(false, ctx);
    }

    fn handle_query_editor_event(&mut self, event: &EditorEvent, ctx: &mut ViewContext<Self>) {
        match event {
            EditorEvent::Edited(_)
            | EditorEvent::BufferReplaced
            | EditorEvent::BufferReinitialized => {
                let _ = self.query_change_tx.try_send(());
            }
            EditorEvent::Navigate(NavigationKey::Down | NavigationKey::PageDown) => {
                self.focus_results_list(ctx);
            }
            EditorEvent::Navigate(NavigationKey::Tab) => {
                ctx.focus(&self.mask_editor);
            }
            EditorEvent::Enter => self.accept(ctx),
            EditorEvent::Escape => self.close(ctx),
            _ => {}
        }
    }

    fn handle_mask_editor_event(&mut self, event: &EditorEvent, ctx: &mut ViewContext<Self>) {
        match event {
            EditorEvent::Edited(_)
            | EditorEvent::BufferReplaced
            | EditorEvent::BufferReinitialized => {
                let _ = self.query_change_tx.try_send(());
            }
            EditorEvent::Navigate(NavigationKey::Down | NavigationKey::PageDown) => {
                self.focus_results_list(ctx);
            }
            EditorEvent::Navigate(NavigationKey::Tab | NavigationKey::ShiftTab) => {
                ctx.focus(&self.query_editor);
            }
            EditorEvent::Enter => {
                self.rerun_search(true, ctx);
            }
            EditorEvent::Escape => self.close(ctx),
            _ => {}
        }
    }

    /// Enter in the query editor: open the selected match, or search now when there is nothing to
    /// open yet.
    fn accept(&mut self, ctx: &mut ViewContext<Self>) {
        if self.rows.is_empty() {
            self.rerun_search(true, ctx);
            return;
        }
        self.ensure_selection(ctx);
        if let Some(selected) = self.selected_row {
            self.activate_row(selected, ctx);
        }
    }

    fn rerun_search(&mut self, force: bool, ctx: &mut ViewContext<Self>) {
        let query = self.query_editor.as_ref(ctx).buffer_text(ctx);
        if query.is_empty() {
            self.cancel_search(ctx);
            self.reset_results();
            ctx.notify();
            return;
        }
        if self.roots.is_empty() {
            return;
        }

        let mask = self.mask_editor.as_ref(ctx).buffer_text(ctx);
        let filters = search_filters(&mask, self.use_ignore_files);
        let (pattern, use_regex) = search_pattern(&query, self.whole_words, self.regex);
        // Skip no-op reruns (e.g. a mask edit that parses to the same globs); toggles force.
        if !force
            && self.last_searched_pattern.as_deref() == Some(pattern.as_str())
            && self.last_searched_filters.as_ref() == Some(&filters)
        {
            return;
        }
        if let Err(err) = PathFilter::new(&filters) {
            self.cancel_search(ctx);
            self.reset_results();
            self.last_error = Some(err.to_string());
            ctx.notify();
            return;
        }

        self.last_searched_pattern = Some(pattern.clone());
        self.last_searched_filters = Some(filters.clone());
        let roots = self.roots.clone();
        self.find_model.update(ctx, |model, model_ctx| {
            model.run_search(
                pattern,
                roots,
                SearchConfig {
                    use_regex,
                    use_case_sensitivity: self.match_case,
                    filters,
                },
                model_ctx,
            );
        });
    }

    fn cancel_search(&mut self, ctx: &mut ViewContext<Self>) {
        self.is_search_in_progress = false;
        self.current_search_id = None;
        self.find_model.update(ctx, |model, model_ctx| {
            model.abort_search(model_ctx);
        });
    }

    fn reset_results(&mut self) {
        self.groups.clear();
        self.group_index_by_path.clear();
        self.rows.clear();
        self.selected_row = None;
        self.total_match_count = 0;
        self.displayed_match_count = 0;
        self.last_error = None;
        self.uniform_list_state = UniformListState::new();
    }

    fn handle_search_event(&mut self, event: &GlobalSearchEvent, ctx: &mut ViewContext<Self>) {
        match event {
            GlobalSearchEvent::Started { search_id, .. } => {
                self.current_search_id = Some(*search_id);
                self.is_search_in_progress = true;
                self.reset_results();
                ctx.notify();
            }
            GlobalSearchEvent::Progress { search_id, result } => {
                if Some(*search_id) != self.current_search_id {
                    return;
                }
                self.apply_match(result);
                self.rebuild_rows();
                ctx.notify();
            }
            GlobalSearchEvent::ProgressBatch { search_id, items } => {
                if Some(*search_id) != self.current_search_id {
                    return;
                }
                for item in items {
                    self.apply_match(item);
                }
                self.rebuild_rows();
                ctx.notify();
            }
            GlobalSearchEvent::Completed {
                search_id,
                total_match_count,
                ..
            } => {
                if Some(*search_id) != self.current_search_id {
                    return;
                }
                self.is_search_in_progress = false;
                self.total_match_count = *total_match_count;
                ctx.notify();
            }
            GlobalSearchEvent::Failed { search_id, error } => {
                if Some(*search_id) != self.current_search_id {
                    return;
                }
                self.is_search_in_progress = false;
                self.reset_results();
                self.last_error = Some(error.clone());
                ctx.notify();
            }
        }
    }

    fn apply_match(&mut self, result: &GlobalSearchMatch) {
        if self.displayed_match_count >= MAX_DISPLAYED_MATCHES {
            return;
        }
        // Only local roots are searched, so remote matches never arrive.
        if !matches!(result.location, LocalOrRemotePath::Local(_)) {
            return;
        }
        let group_index = match self.group_index_by_path.get(&result.location) {
            Some(&index) => index,
            None => {
                let display_path = self.display_path_for(&result.location);
                self.groups
                    .push(FileGroup::new(result.location.clone(), display_path));
                let index = self.groups.len() - 1;
                self.group_index_by_path.insert(result.location.clone(), index);
                index
            }
        };
        self.groups[group_index].matches.push(MatchRow::new(
            result.line_text.clone(),
            result.line_number,
            result.column_num,
            result.submatches.clone(),
        ));
        self.displayed_match_count += 1;
    }

    /// The matched file's path relative to the search root holding it.
    fn display_path_for(&self, location: &LocalOrRemotePath) -> PathBuf {
        self.roots
            .iter()
            .find_map(|root| root.strip_repo_prefix(location))
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(location.display_path()))
    }

    fn rebuild_rows(&mut self) {
        let counts: Vec<usize> = self.groups.iter().map(|group| group.matches.len()).collect();
        self.rows = flatten_rows(&counts);
        if self.selected_row.is_some_and(|selected| selected >= self.rows.len()) {
            self.selected_row = None;
        }
    }

    fn focus_results_list(&mut self, ctx: &mut ViewContext<Self>) {
        if self.rows.is_empty() {
            return;
        }
        self.focus_results = true;
        self.ensure_selection(ctx);
        ctx.focus_self();
        ctx.notify();
    }

    fn focus_query_editor(&mut self, ctx: &mut ViewContext<Self>) {
        self.focus_results = false;
        ctx.focus(&self.query_editor);
        ctx.notify();
    }

    fn ensure_selection(&mut self, ctx: &mut ViewContext<Self>) {
        if self.selected_row.is_none() && !self.rows.is_empty() {
            self.select_row(0, ctx);
        }
    }

    fn select_row(&mut self, index: usize, ctx: &mut ViewContext<Self>) {
        self.selected_row = Some(index);
        self.uniform_list_state.scroll_to(index);
        ctx.notify();
    }

    fn move_selection(&mut self, delta: isize, ctx: &mut ViewContext<Self>) {
        if self.rows.is_empty() {
            return;
        }
        self.ensure_selection(ctx);
        let Some(current) = self.selected_row else {
            return;
        };
        let max = self.rows.len() - 1;
        let next = current.saturating_add_signed(delta).min(max);
        self.select_row(next, ctx);
    }

    /// Opens the row's match (a file header opens its first match) and closes the popup.
    fn activate_row(&mut self, row: usize, ctx: &mut ViewContext<Self>) {
        let Some(row) = self.rows.get(row).copied() else {
            return;
        };
        let (group, index) = match row {
            ResultRow::FileHeader { group } => (group, 0),
            ResultRow::Match { group, index } => (group, index),
        };
        let Some(file) = self.groups.get(group) else {
            return;
        };
        let Some(matched) = file.matches.get(index) else {
            return;
        };
        let LocalOrRemotePath::Local(path) = &file.path else {
            return;
        };
        let path = path.clone();
        let line_number = matched.line_number;
        let column_num = matched.column_num;
        self.dismiss(ctx);
        ctx.emit(FindInFilesEvent::OpenMatch {
            path,
            line_number,
            column_num,
        });
    }

    fn render_row_at_index(&self, index: usize, app: &AppContext) -> Box<dyn Element> {
        let Some(row) = self.rows.get(index).copied() else {
            return Empty::new().finish();
        };
        let row = match row {
            ResultRow::FileHeader { group } => self.render_file_header(index, group, app),
            ResultRow::Match { group, index: match_index } => {
                self.render_match_row(index, group, match_index, app)
            }
        };
        // UniformList measures every row from the first, so rows must share one height.
        ConstrainedBox::new(row).with_height(ROW_HEIGHT).finish()
    }

    fn render_file_header(&self, index: usize, group: usize, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let Some(file) = self.groups.get(group) else {
            return Empty::new().finish();
        };
        let is_selected = self.selected_row == Some(index);
        let mouse_state = file.mouse_state.clone();
        let display_path = file.display_path.clone();
        let match_count = file.matches.len();

        Hoverable::new(mouse_state, move |mouse_state| {
            let list_highlight_state = ItemHighlightState::new(is_selected, mouse_state);
            let search_highlight_state = SearchHighlightState::new(is_selected, mouse_state);

            let icon_size = 16.0;
            let icon_color = list_highlight_state.text_and_icon_color(appearance);
            let file_icon = match icon_from_file_path(&display_path.to_string_lossy(), appearance) {
                Some(image) => ImageOrIcon::Image(image),
                None => ImageOrIcon::Icon(UiIcon::File),
            };
            let file_icon = match file_icon {
                ImageOrIcon::Icon(icon) => icon.to_warpui_icon(ThemeFill::from(icon_color)).finish(),
                ImageOrIcon::Image(image) => image,
            };
            let file_icon = Container::new(
                ConstrainedBox::new(file_icon)
                    .with_width(icon_size)
                    .with_height(icon_size)
                    .finish(),
            )
            .with_padding_right(8.)
            .finish();

            let header_text_fill = match list_highlight_state {
                ItemHighlightState::None => {
                    ThemeFill::Solid(blended_colors::text_sub(theme, theme.background()))
                }
                ItemHighlightState::Hovered => {
                    ThemeFill::Solid(blended_colors::text_main(theme, theme.background()))
                }
                ItemHighlightState::Selected => ThemeFill::Solid(theme.foreground().into()),
            };
            let path_row = render_file_search_row(
                &display_path,
                FileSearchRowOptions {
                    item_font_size: Some(13.),
                    path_font_size: Some(12.),
                    highlight_state: search_highlight_state,
                    text_color_override: Some(header_text_fill),
                    max_combined_length: None,
                    ..Default::default()
                },
                app,
            );

            let count_text =
                Text::new_inline(match_count.to_string(), appearance.ui_font_family(), 12.0)
                    .with_color(appearance.theme().background().into())
                    .finish();
            let count_badge = Container::new(count_text)
                .with_horizontal_padding(4.)
                .with_vertical_padding(1.)
                .with_background_color(theme.terminal_colors().normal.yellow.into())
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.)))
                .finish();

            let row = Flex::row()
                .with_main_axis_size(MainAxisSize::Max)
                .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(
                    Shrinkable::new(
                        1.,
                        Flex::row()
                            .with_cross_axis_alignment(CrossAxisAlignment::Center)
                            .with_child(file_icon)
                            .with_child(Shrinkable::new(1., path_row).finish())
                            .finish(),
                    )
                    .finish(),
                )
                .with_child(count_badge)
                .finish();

            let mut row = Container::new(row)
                .with_padding(Padding::uniform(2.).with_left(8.).with_right(8.));
            if let Some(background_fill) = list_highlight_state.background_color(appearance) {
                row = row
                    .with_background(background_fill)
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.)));
            }
            row.finish()
        })
        .with_cursor(Cursor::PointingHand)
        .on_click(move |ctx, _, _| {
            ctx.dispatch_typed_action(FindInFilesAction::SelectRow(index));
        })
        .finish()
    }

    fn render_match_row(
        &self,
        index: usize,
        group: usize,
        match_index: usize,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let Some(file) = self.groups.get(group) else {
            return Empty::new().finish();
        };
        let Some(matched) = file.matches.get(match_index) else {
            return Empty::new().finish();
        };
        let is_selected = self.selected_row == Some(index);
        let mouse_state = matched.mouse_state.clone();
        let line_number = matched.line_number;
        let line_text = matched.line_text.clone();
        let submatches = matched.submatches.clone();

        Hoverable::new(mouse_state, move |mouse_state| {
            let list_highlight_state = ItemHighlightState::new(is_selected, mouse_state);
            let text_color = match list_highlight_state {
                ItemHighlightState::None => blended_colors::text_sub(theme, theme.background()),
                ItemHighlightState::Hovered => blended_colors::text_main(theme, theme.background()),
                ItemHighlightState::Selected => theme.foreground().into(),
            };

            let line_number_text = Text::new_inline(
                line_number.to_string(),
                appearance.monospace_font_family(),
                12.,
            )
            .with_color(blended_colors::text_disabled(theme, theme.background()))
            .finish();
            let line_number_element = Container::new(
                ConstrainedBox::new(line_number_text).with_width(40.).finish(),
            )
            .with_padding_right(8.)
            .finish();

            let highlight_indices =
                crate::workspace::view::global_search::view::GlobalSearchView::highlight_indices_from_submatches(
                    &line_text,
                    &submatches,
                );
            let (display_text, display_highlight_indices) =
                crate::workspace::view::global_search::view::GlobalSearchView::elide_match_text(
                    &line_text,
                    highlight_indices,
                );
            let mut match_text = Text::new_inline(
                display_text,
                appearance.monospace_font_family(),
                13.,
            )
            .with_color(text_color);
            if !display_highlight_indices.is_empty() {
                let yellow_overlay_2 = theme.ansi_overlay_2(
                    AnsiColorIdentifier::Yellow.to_ansi_color(&theme.terminal_colors().normal),
                );
                let highlight_style = TextStyle::new().with_background_color(yellow_overlay_2);
                let highlight = Highlight::new().with_text_style(highlight_style);
                match_text =
                    match_text.with_single_highlight(highlight, display_highlight_indices);
            }

            let row = Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(line_number_element)
                .with_child(
                    Shrinkable::new(1., Clipped::new(match_text.finish()).finish()).finish(),
                )
                .finish();

            let mut row = Container::new(row)
                .with_padding(Padding::uniform(2.).with_left(8.).with_right(8.));
            if let Some(background_fill) = list_highlight_state.background_color(appearance) {
                row = row
                    .with_background(background_fill)
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.)));
            }
            row.finish()
        })
        .with_cursor(Cursor::PointingHand)
        .on_click(move |ctx, _, _| {
            ctx.dispatch_typed_action(FindInFilesAction::OpenRow(index));
        })
        .finish()
    }

    fn render_results(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let handle = self.handle.clone();
        let item_count = self.rows.len();

        let build_items = move |range: Range<usize>, app: &AppContext| {
            let view_handle = handle
                .upgrade(app)
                .expect("FindInFilesView handle should be upgradeable");
            let view = view_handle.as_ref(app);
            (range.start..range.end)
                .map(|idx| view.render_row_at_index(idx, app))
                .collect::<Vec<_>>()
                .into_iter()
        };

        let list = UniformList::new(self.uniform_list_state.clone(), item_count, build_items);
        let scrollable = Scrollable::vertical(
            self.scroll_state.clone(),
            list.finish_scrollable(),
            ScrollbarWidth::Auto,
            theme.nonactive_ui_detail().into(),
            theme.active_ui_detail().into(),
            Fill::None,
        )
        .with_overlayed_scrollbar()
        .finish();

        let height = (item_count.min(MAX_VISIBLE_ROWS)) as f32 * ROW_HEIGHT;
        ConstrainedBox::new(scrollable)
            .with_height(height.max(ROW_HEIGHT))
            .finish()
    }
}

impl TypedActionView for FindInFilesView {
    type Action = FindInFilesAction;

    fn handle_action(&mut self, action: &FindInFilesAction, ctx: &mut ViewContext<Self>) {
        match action {
            FindInFilesAction::Close => self.close(ctx),
            FindInFilesAction::MoveSelectionUp => {
                if !self.focus_results {
                    return;
                }
                if self.selected_row == Some(0) {
                    self.focus_query_editor(ctx);
                } else {
                    self.move_selection(-1, ctx);
                }
            }
            FindInFilesAction::MoveSelectionDown => {
                if self.focus_results {
                    self.move_selection(1, ctx);
                }
            }
            FindInFilesAction::MoveSelectionPageUp => {
                if self.focus_results {
                    self.move_selection(-PAGE_ROWS, ctx);
                }
            }
            FindInFilesAction::MoveSelectionPageDown => {
                if self.focus_results {
                    self.move_selection(PAGE_ROWS, ctx);
                }
            }
            FindInFilesAction::OpenSelected => {
                if !self.focus_results {
                    return;
                }
                self.ensure_selection(ctx);
                if let Some(selected) = self.selected_row {
                    self.activate_row(selected, ctx);
                }
            }
            FindInFilesAction::ToggleMatchCase => {
                self.match_case = !self.match_case;
                let match_case = self.match_case;
                self.match_case_button.update(ctx, |button, ctx| {
                    button.set_active(match_case, ctx);
                });
                self.rerun_search(true, ctx);
            }
            FindInFilesAction::ToggleWholeWords => {
                self.whole_words = !self.whole_words;
                let whole_words = self.whole_words;
                self.whole_words_button.update(ctx, |button, ctx| {
                    button.set_active(whole_words, ctx);
                });
                self.rerun_search(true, ctx);
            }
            FindInFilesAction::ToggleRegex => {
                self.regex = !self.regex;
                let regex = self.regex;
                self.regex_button.update(ctx, |button, ctx| {
                    button.set_active(regex, ctx);
                });
                self.rerun_search(true, ctx);
            }
            FindInFilesAction::ToggleIgnoreFiles => {
                self.use_ignore_files = !self.use_ignore_files;
                let use_ignore_files = self.use_ignore_files;
                self.ignore_files_button.update(ctx, |button, ctx| {
                    button.set_active(use_ignore_files, ctx);
                });
                self.rerun_search(true, ctx);
            }
            FindInFilesAction::SelectRow(index) => {
                self.select_row(*index, ctx);
            }
            FindInFilesAction::OpenRow(index) => {
                self.activate_row(*index, ctx);
            }
        }
    }
}

impl View for FindInFilesView {
    fn ui_name() -> &'static str {
        "FindInFilesView"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        if !self.open {
            return Empty::new().finish();
        }
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();

        let mut query_row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(
                Shrinkable::new(
                    1.,
                    ConstrainedBox::new(Clipped::new(ChildView::new(&self.query_editor).finish()).finish())
                        .finish(),
                )
                .finish(),
            );
        for button in [
            &self.match_case_button,
            &self.whole_words_button,
            &self.regex_button,
        ] {
            query_row.add_child(Container::new(ChildView::new(button).finish()).finish());
        }
        let query_box = Container::new(query_row.finish())
            .with_padding(Padding::uniform(6.))
            .with_border(Border::all(1.).with_border_fill(theme.surface_3()))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.)))
            .finish();

        let scope_text = Text::new_inline(
            format!("in {}", self.scope_label),
            appearance.ui_font_family(),
            12.,
        )
        .with_color(blended_colors::text_disabled(theme, theme.background()))
        .finish();
        let mask_box = Container::new(ChildView::new(&self.mask_editor).finish())
            .with_padding(Padding::uniform(6.))
            .with_border(Border::all(1.).with_border_fill(theme.surface_3()))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.)))
            .finish();
        let mask_row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(Shrinkable::new(1., mask_box).finish())
            .with_child(Container::new(ChildView::new(&self.ignore_files_button).finish()).finish())
            .with_child(Container::new(scope_text).with_padding_left(8.).finish())
            .finish();

        let total_matches = if self.is_search_in_progress {
            self.displayed_match_count
        } else {
            self.total_match_count
        };
        let query_is_empty = self.query_editor.as_ref(app).buffer_text(app).is_empty();
        let footer_text = if let Some(error) = &self.last_error {
            error.clone()
        } else if query_is_empty {
            format!("Search in {}", self.scope_label)
        } else {
            results_summary(
                total_matches,
                self.groups.len(),
                self.displayed_match_count,
                self.is_search_in_progress,
            )
        };
        let footer = Container::new(
            Text::new_inline(footer_text, appearance.ui_font_family(), 12.)
                .with_color(blended_colors::text_disabled(theme, theme.background()))
                .finish(),
        )
        .with_padding_top(8.)
        .finish();

        let mut column = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(query_box)
            .with_child(Container::new(mask_row).with_padding_top(8.).finish());
        if !self.rows.is_empty() {
            column.add_child(Container::new(self.render_results(app)).with_padding_top(8.).finish());
        }
        column.add_child(footer);

        let card = ConstrainedBox::new(
            Container::new(column.finish())
                .with_uniform_padding(12.)
                .with_background(theme.surface_1())
                .with_border(Border::all(1.).with_border_fill(theme.outline()))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.)))
                .finish(),
        )
        .with_width(POPUP_WIDTH)
        .finish();
        // Clicks inside the card must not reach the dismiss-on-click backdrop.
        let card = EventHandler::new(card)
            .on_left_mouse_down(|_, _, _| DispatchEventResult::StopPropagation)
            .finish();

        let mut stack = Stack::new();
        stack.add_positioned_child(
            card,
            OffsetPositioning::offset_from_parent(
                vec2f(0., 96.),
                ParentOffsetBounds::WindowByPosition,
                ParentAnchor::TopMiddle,
                ChildAnchor::TopMiddle,
            ),
        );
        EventHandler::new(
            Container::new(Align::new(stack.finish()).finish())
                .with_background_color(theme.blurred_background_overlay().into())
                .finish(),
        )
        .on_left_mouse_down(|ctx, _, _| {
            ctx.dispatch_typed_action(FindInFilesAction::Close);
            DispatchEventResult::StopPropagation
        })
        .finish()
    }
}

impl MatchRow {
    fn new(
        line_text: String,
        line_number: u32,
        column_num: Option<usize>,
        submatches: Vec<Submatch>,
    ) -> Self {
        Self {
            line_text,
            line_number,
            column_num,
            submatches,
            mouse_state: MouseStateHandle::default(),
        }
    }
}

impl FileGroup {
    fn new(path: LocalOrRemotePath, display_path: PathBuf) -> Self {
        Self {
            path,
            display_path,
            mouse_state: MouseStateHandle::default(),
            matches: Vec::new(),
        }
    }
}

#[cfg(test)]
#[path = "find_in_files_tests.rs"]
mod tests;
