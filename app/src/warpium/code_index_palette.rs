//! Command palette data sources and rows for Go to Class / Go to Symbol and for the results of
//! Go to Definition and Find Usages.

use std::path::PathBuf;

use code_index::{SearchScope, SymbolInfo, SymbolKind, Usage, flags};
use futures_lite::FutureExt as _;
use ordered_float::OrderedFloat;
use warp_util::path::LineAndColumnArg;
use warpui::elements::{Align, ConstrainedBox, Flex, Highlight, ParentElement, Shrinkable, Text};
use warpui::fonts::{Properties, Weight};
use warpui::{AppContext, Element, Entity, SingletonEntity};

use super::code_index::{CodeIndexModel, IndexState, NavigationItem};
use crate::appearance::Appearance;
use crate::search::action::search_item::styles;
use crate::search::command_palette::mixer::CommandPaletteItemAction;
use crate::search::command_palette::render_util;
use crate::search::data_source::{Query, QueryResult};
use crate::search::item::SearchItem;
use crate::search::mixer::{AsyncDataSource, BoxFuture, DataSourceRunErrorWrapper};
use crate::search::result_renderer::ItemHighlightState;
use crate::ui_components::icons::Icon as UiIcon;

/// Results shown per query.
const MAX_RESULTS: usize = 100;

/// Searches the current project's code index.
pub(crate) struct CodeSymbolDataSource {
    scope: SearchScope,
}

impl CodeSymbolDataSource {
    pub(crate) fn new(scope: SearchScope) -> Self {
        Self { scope }
    }
}

impl Entity for CodeSymbolDataSource {
    type Event = ();
}

impl AsyncDataSource for CodeSymbolDataSource {
    type Action = CommandPaletteItemAction;

    fn run_query(
        &self,
        query: &Query,
        app: &AppContext,
    ) -> BoxFuture<'static, Result<Vec<QueryResult<Self::Action>>, DataSourceRunErrorWrapper>> {
        let handle = app
            .has_singleton_model::<CodeIndexModel>()
            .then(|| CodeIndexModel::as_ref(app).handle_for_search())
            .flatten();
        let scope = self.scope;
        let text = query.text.clone();
        async move {
            let Some(handle) = handle else {
                return Ok(vec![QueryResult::from(StatusItem {
                    message: "Open a project folder to use Go to Class and Go to Symbol".to_owned(),
                })]);
            };
            // Runs on a background executor; the read lock is shared with other searches and
            // only waits for a batch being applied.
            let results = handle.index.read().search(&text, scope, MAX_RESULTS);
            let count = results.len();
            let mut items: Vec<QueryResult<CommandPaletteItemAction>> = results
                .into_iter()
                .enumerate()
                .map(|(rank, result)| {
                    QueryResult::from(SymbolSearchItem {
                        root: handle.root.clone(),
                        // The mixer sorts by score, so the score keeps the index's ranking.
                        score: (count - rank) as f64,
                        matched_indices: char_indices(&result.symbol.name, &result.matched_indices),
                        symbol: result.symbol,
                    })
                })
                .collect();
            if handle.state == IndexState::Indexing {
                let project = handle
                    .root
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default();
                items.push(QueryResult::from(StatusItem {
                    message: format!("Indexing {project}… results may be incomplete"),
                }));
            }
            Ok(items)
        }
        .boxed()
    }
}

/// Lists the results of the last Go to Definition or Find Usages, filtered by the query.
pub(crate) struct CodeNavigationDataSource;

impl Entity for CodeNavigationDataSource {
    type Event = ();
}

impl AsyncDataSource for CodeNavigationDataSource {
    type Action = CommandPaletteItemAction;

    fn run_query(
        &self,
        query: &Query,
        app: &AppContext,
    ) -> BoxFuture<'static, Result<Vec<QueryResult<Self::Action>>, DataSourceRunErrorWrapper>> {
        let results = app
            .has_singleton_model::<CodeIndexModel>()
            .then(|| CodeIndexModel::as_ref(app).navigation_results())
            .flatten();
        let filter = query.text.trim().to_lowercase();
        async move {
            let Some(results) = results else {
                return Ok(vec![QueryResult::from(StatusItem {
                    message: "Use Go to Definition or Find Usages in a Kotlin or Java file"
                        .to_owned(),
                })]);
            };
            let count = results.items.len();
            let mut items: Vec<QueryResult<CommandPaletteItemAction>> = results
                .items
                .iter()
                .enumerate()
                .filter(|(_, item)| filter.is_empty() || matches_filter(item, &filter))
                .map(|(rank, item)| {
                    // The mixer sorts by score, so the score keeps the index's order.
                    let score = (count - rank) as f64;
                    match item {
                        NavigationItem::Definition(definition) => {
                            QueryResult::from(SymbolSearchItem {
                                root: results.root.clone(),
                                symbol: definition.symbol.clone(),
                                score,
                                matched_indices: Vec::new(),
                            })
                        }
                        NavigationItem::Usage(usage) => QueryResult::from(UsageSearchItem {
                            root: results.root.clone(),
                            usage: usage.clone(),
                            score,
                        }),
                    }
                })
                .collect();
            items.push(QueryResult::from(StatusItem {
                message: results.summary.clone(),
            }));
            Ok(items)
        }
        .boxed()
    }
}

/// Whether the lowercase `filter` occurs in the item's path, line or name.
fn matches_filter(item: &NavigationItem, filter: &str) -> bool {
    match item {
        NavigationItem::Definition(definition) => {
            definition.symbol.rel_path.to_lowercase().contains(filter)
                || definition
                    .symbol
                    .qualified_name()
                    .to_lowercase()
                    .contains(filter)
        }
        NavigationItem::Usage(usage) => {
            usage.rel_path.to_lowercase().contains(filter)
                || usage.line_text.to_lowercase().contains(filter)
        }
    }
}

/// Converts byte offsets in `name` to the character indices text highlighting uses.
fn char_indices(name: &str, byte_offsets: &[usize]) -> Vec<usize> {
    name.char_indices()
        .enumerate()
        .filter(|(_, (byte, _))| byte_offsets.contains(byte))
        .map(|(index, _)| index)
        .collect()
}

#[derive(Debug)]
struct SymbolSearchItem {
    root: PathBuf,
    symbol: SymbolInfo,
    score: f64,
    matched_indices: Vec<usize>,
}

impl SymbolSearchItem {
    /// `com.example.feature.Outer · Foo.kt · commonMain · expect`
    fn details(&self) -> String {
        let file_name = self.symbol.rel_path.rsplit('/').next().unwrap_or_default();
        let mut parts = vec![];
        let location = self.symbol.location();
        if !location.is_empty() {
            parts.push(location);
        }
        parts.push(format!("{file_name}:{}", self.symbol.name_pos.line + 1));
        if !self.symbol.source_set.is_empty() {
            parts.push(self.symbol.source_set.clone());
        }
        if self.symbol.has_flag(flags::EXPECT) {
            parts.push("expect".to_owned());
        }
        if self.symbol.has_flag(flags::ACTUAL) {
            parts.push("actual".to_owned());
        }
        if self.symbol.origin == code_index::Origin::Generated {
            parts.push("generated".to_owned());
        }
        parts.join(" · ")
    }

    fn label(&self) -> String {
        match &self.symbol.receiver {
            Some(receiver) => format!("{receiver}.{}", self.symbol.name),
            None => self.symbol.name.clone(),
        }
    }
}

impl SearchItem for SymbolSearchItem {
    type Action = CommandPaletteItemAction;

    fn render_icon(
        &self,
        highlight_state: ItemHighlightState,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let icon = if self.symbol.kind.is_type() {
            UiIcon::Code2
        } else {
            UiIcon::Code1
        };
        render_util::render_search_item_icon(
            appearance,
            icon,
            highlight_state.icon_fill(appearance).into_solid(),
            highlight_state,
        )
    }

    fn render_item(
        &self,
        highlight_state: ItemHighlightState,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let receiver_len = self
            .symbol
            .receiver
            .as_ref()
            .map_or(0, |receiver| receiver.chars().count() + 1);
        render_row(
            self.label(),
            self.matched_indices
                .iter()
                .map(|index| index + receiver_len)
                .collect(),
            format!("  {}  {}", self.symbol.kind.label(), self.details()),
            highlight_state,
            app,
        )
    }

    fn score(&self) -> OrderedFloat<f64> {
        OrderedFloat(self.score)
    }

    fn accept_result(&self) -> CommandPaletteItemAction {
        CommandPaletteItemAction::OpenFile {
            path: self.symbol.rel_path.clone(),
            project_directory: self.root.to_string_lossy().into_owned(),
            line_and_column_arg: Some(LineAndColumnArg {
                line_num: self.symbol.name_pos.line as usize + 1,
                column_num: Some(self.symbol.name_pos.column as usize + 1),
            }),
        }
    }

    fn execute_result(&self) -> CommandPaletteItemAction {
        self.accept_result()
    }

    fn accessibility_label(&self) -> String {
        format!(
            "{} {} in {}",
            kind_name(self.symbol.kind),
            self.label(),
            self.symbol.location()
        )
    }

    fn accessibility_help_message(&self) -> Option<String> {
        Some("Press Enter to open the declaration".to_owned())
    }
}

/// A row: the label in the main text color (bold where `highlighted`), then dimmer details.
fn render_row(
    label: String,
    highlighted: Vec<usize>,
    details: String,
    highlight_state: ItemHighlightState,
    app: &AppContext,
) -> Box<dyn Element> {
    let appearance = Appearance::as_ref(app);
    let label = Text::new_inline(
        label,
        appearance.ui_font_family(),
        appearance.monospace_font_size(),
    )
    .with_color(highlight_state.main_text_fill(appearance).into_solid())
    .with_style(Properties::default().weight(Weight::Normal))
    .with_single_highlight(
        Highlight::new().with_properties(Properties::default().weight(Weight::Bold)),
        highlighted,
    )
    .finish();
    let details = Text::new_inline(
        details,
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

/// One usage: `NewsViewModel.kt:11  val items = repository.getNewsResources()`.
#[derive(Debug)]
struct UsageSearchItem {
    root: PathBuf,
    usage: Usage,
    score: f64,
}

impl UsageSearchItem {
    fn label(&self) -> String {
        let file_name = self.usage.rel_path.rsplit('/').next().unwrap_or_default();
        format!("{file_name}:{}", self.usage.position.line + 1)
    }

    /// The line, then `≈` for a usage matched by name only, then the directory.
    fn details(&self) -> String {
        let directory = self
            .usage
            .rel_path
            .rsplit_once('/')
            .map_or("", |(directory, _)| directory);
        let approximate = if self.usage.exact { "" } else { "≈ " };
        format!("  {approximate}{}  · {directory}", self.usage.line_text)
    }
}

impl SearchItem for UsageSearchItem {
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
        render_row(
            self.label(),
            Vec::new(),
            self.details(),
            highlight_state,
            app,
        )
    }

    fn score(&self) -> OrderedFloat<f64> {
        OrderedFloat(self.score)
    }

    fn accept_result(&self) -> CommandPaletteItemAction {
        CommandPaletteItemAction::OpenFile {
            path: self.usage.rel_path.clone(),
            project_directory: self.root.to_string_lossy().into_owned(),
            line_and_column_arg: Some(LineAndColumnArg {
                line_num: self.usage.position.line as usize + 1,
                column_num: Some(self.usage.position.column as usize + 1),
            }),
        }
    }

    fn execute_result(&self) -> CommandPaletteItemAction {
        self.accept_result()
    }

    fn accessibility_label(&self) -> String {
        format!("Usage in {}: {}", self.label(), self.usage.line_text)
    }

    fn accessibility_help_message(&self) -> Option<String> {
        Some("Press Enter to open the usage".to_owned())
    }
}

fn kind_name(kind: SymbolKind) -> &'static str {
    match kind {
        SymbolKind::Class => "Class",
        SymbolKind::Interface => "Interface",
        SymbolKind::Object => "Object",
        SymbolKind::CompanionObject => "Companion object",
        SymbolKind::Enum => "Enum",
        SymbolKind::Annotation => "Annotation",
        SymbolKind::Record => "Record",
        SymbolKind::TypeAlias => "Type alias",
        SymbolKind::Function => "Function",
        SymbolKind::Constructor => "Constructor",
        SymbolKind::Property => "Property",
        SymbolKind::EnumEntry => "Enum entry",
    }
}

/// A disabled row explaining why results are missing or incomplete.
#[derive(Debug)]
struct StatusItem {
    message: String,
}

impl SearchItem for StatusItem {
    type Action = CommandPaletteItemAction;

    fn render_icon(
        &self,
        _highlight_state: ItemHighlightState,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        render_util::render_search_item_icon_placeholder(appearance)
    }

    fn render_item(
        &self,
        highlight_state: ItemHighlightState,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        ConstrainedBox::new(
            Align::new(
                Text::new_inline(
                    self.message.clone(),
                    appearance.ui_font_family(),
                    appearance.monospace_font_size(),
                )
                .with_color(highlight_state.sub_text_fill(appearance).into_solid())
                .finish(),
            )
            .left()
            .finish(),
        )
        .with_height(styles::SEARCH_ITEM_HEIGHT)
        .finish()
    }

    fn score(&self) -> OrderedFloat<f64> {
        // Below every symbol, whose scores are positive.
        OrderedFloat(-1.)
    }

    fn accept_result(&self) -> CommandPaletteItemAction {
        CommandPaletteItemAction::NoOp
    }

    fn execute_result(&self) -> CommandPaletteItemAction {
        CommandPaletteItemAction::NoOp
    }

    fn accessibility_label(&self) -> String {
        self.message.clone()
    }

    fn is_disabled(&self) -> bool {
        true
    }
}

#[cfg(test)]
#[path = "code_index_palette_tests.rs"]
mod tests;
