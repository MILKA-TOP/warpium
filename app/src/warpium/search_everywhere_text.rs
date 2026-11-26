//! Warpium: the Text tab of the cmd-O project search — file contents over the project, the
//! palette counterpart of Find in Files. Rows open at the match like the Files tab's rows.

use std::path::PathBuf;

use futures::StreamExt as _;
use futures_lite::FutureExt as _;
use ordered_float::OrderedFloat;
use warp_ripgrep::filter::SearchFilters;
use warp_ripgrep::search::{Match as RipgrepMatch, Submatch};
use warp_util::path::LineAndColumnArg;
use warpui::elements::{Align, ConstrainedBox, Flex, Highlight, ParentElement, Shrinkable, Text};
use warpui::fonts::{Properties, Weight};
use warpui::{AppContext, Element, Entity, SingletonEntity};

use super::code_index_palette::StatusItem;
use crate::appearance::Appearance;
use crate::search::action::search_item::styles;
use crate::search::command_palette::mixer::CommandPaletteItemAction;
use crate::search::command_palette::render_util;
use crate::search::data_source::{Query, QueryResult};
use crate::search::item::SearchItem;
use crate::search::mixer::{AsyncDataSource, BoxFuture, DataSourceRunErrorWrapper};
use crate::search::result_renderer::ItemHighlightState;
use crate::ui_components::icons::Icon as UiIcon;
use crate::workspace::view::global_search::view::GlobalSearchView;

/// Matches listed per query; Find in Files (cmd-shift-F) lists more.
const MAX_RESULTS: usize = 100;

/// Searches the project folders the workspace scoped the project search to.
pub(crate) struct TextTabDataSource {
    roots: Vec<PathBuf>,
}

impl TextTabDataSource {
    pub(crate) fn new(roots: Vec<PathBuf>) -> Self {
        Self { roots }
    }
}

impl Entity for TextTabDataSource {
    type Event = ();
}

impl AsyncDataSource for TextTabDataSource {
    type Action = CommandPaletteItemAction;

    fn run_query(
        &self,
        query: &Query,
        _app: &AppContext,
    ) -> BoxFuture<'static, Result<Vec<QueryResult<Self::Action>>, DataSourceRunErrorWrapper>> {
        let roots = self.roots.clone();
        let text = query.text.clone();
        async move {
            if text.is_empty() {
                return Ok(Vec::new());
            }
            if roots.is_empty() {
                return Ok(vec![QueryResult::from(StatusItem::footer(
                    "Open a project folder to search file contents".to_owned(),
                ))]);
            }
            let patterns = [text];
            let stream = warp_ripgrep::search::search_streaming_with_filters(
                &patterns,
                &roots,
                true,
                false,
                &SearchFilters::default(),
            );
            let Ok(stream) = stream else {
                return Ok(vec![QueryResult::from(StatusItem::footer(
                    "Invalid search pattern".to_owned(),
                ))]);
            };
            futures::pin_mut!(stream);

            let mut matches = Vec::new();
            while let Some(matched) = stream.next().await {
                if matches.len() >= MAX_RESULTS {
                    break;
                }
                matches.push(matched);
            }
            let truncated = matches.len() == MAX_RESULTS;
            let count = matches.len();
            let mut items: Vec<QueryResult<CommandPaletteItemAction>> = matches
                .into_iter()
                .enumerate()
                .filter_map(|(rank, matched)| {
                    TextMatchSearchItem::new(&roots, matched, (count - rank) as f64)
                        .map(QueryResult::from)
                })
                .collect();
            if truncated {
                items.push(QueryResult::from(StatusItem::footer(format!(
                    "Showing the first {MAX_RESULTS} matches — use Find in Files (cmd-shift-F) for more"
                ))));
            }
            Ok(items)
        }
        .boxed()
    }
}

/// One matched line: the line's text with the match emphasized, and `path:line` as the detail.
struct TextMatchSearchItem {
    root: PathBuf,
    rel_path: String,
    line_number: u32,
    line_text: String,
    submatches: Vec<Submatch>,
    score: f64,
}

impl TextMatchSearchItem {
    /// The match under the search root that holds it; `None` when no root does.
    fn new(roots: &[PathBuf], matched: RipgrepMatch, score: f64) -> Option<Self> {
        let (root, rel_path) = roots
            .iter()
            .filter_map(|root| matched.file_path.strip_prefix(root).ok().map(|rel| (root, rel)))
            .min_by_key(|(_, rel)| rel.components().count())
            .map(|(root, rel)| (root.clone(), rel.to_string_lossy().into_owned()))?;
        Some(Self {
            root,
            rel_path,
            line_number: matched.line_number,
            line_text: matched.line_text,
            submatches: matched.submatches,
            score,
        })
    }

    /// The 1-based character column of the first submatch, for opening at the match.
    fn first_match_column(&self) -> Option<usize> {
        let start = self.submatches.first()?.byte_start.as_usize();
        (start <= self.line_text.len() && self.line_text.is_char_boundary(start))
            .then(|| self.line_text[..start].chars().count() + 1)
    }
}

impl SearchItem for TextMatchSearchItem {
    type Action = CommandPaletteItemAction;

    fn render_icon(
        &self,
        highlight_state: ItemHighlightState,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        render_util::render_search_item_icon(
            appearance,
            UiIcon::Code1,
            highlight_state.icon_fill(appearance).into_solid(),
            highlight_state,
        )
    }

    fn render_item(
        &self,
        highlight_state: ItemHighlightState,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let highlight_indices =
            GlobalSearchView::highlight_indices_from_submatches(&self.line_text, &self.submatches);
        let (display_text, display_indices) =
            GlobalSearchView::elide_match_text(&self.line_text, highlight_indices);
        let label = Text::new_inline(
            display_text,
            appearance.monospace_font_family(),
            appearance.monospace_font_size(),
        )
        .with_color(highlight_state.main_text_fill(appearance).into_solid())
        .with_style(Properties::default().weight(Weight::Normal))
        .with_single_highlight(
            Highlight::new().with_properties(Properties::default().weight(Weight::Bold)),
            display_indices,
        )
        .finish();
        let details = Text::new_inline(
            format!("{}:{}", self.rel_path, self.line_number),
            appearance.ui_font_family(),
            appearance.monospace_font_size() - 1.,
        )
        .with_color(highlight_state.sub_text_fill(appearance).into_solid())
        .finish();

        ConstrainedBox::new(
            Align::new(
                Flex::row()
                    .with_child(label)
                    .with_child(Shrinkable::new(1., details).finish())
                    .finish(),
            )
            .left()
            .finish(),
        )
        .with_height(styles::SEARCH_ITEM_HEIGHT)
        .finish()
    }

    fn score(&self) -> OrderedFloat<f64> {
        OrderedFloat(self.score)
    }

    fn accept_result(&self) -> CommandPaletteItemAction {
        CommandPaletteItemAction::OpenFile {
            path: self.rel_path.clone(),
            project_directory: self.root.to_string_lossy().into_owned(),
            line_and_column_arg: Some(LineAndColumnArg {
                line_num: self.line_number as usize,
                column_num: self.first_match_column(),
            }),
        }
    }

    fn execute_result(&self) -> CommandPaletteItemAction {
        self.accept_result()
    }

    fn accessibility_label(&self) -> String {
        format!("{}:{}", self.rel_path, self.line_number)
    }

    fn accessibility_help_message(&self) -> Option<String> {
        Some("Press Enter to open the match".to_owned())
    }
}

#[cfg(test)]
#[path = "search_everywhere_text_tests.rs"]
mod tests;
