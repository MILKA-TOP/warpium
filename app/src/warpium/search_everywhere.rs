//! Android Studio–style project search on cmd-O: in a Kotlin/Java project the Files palette gets
//! All, Classes, Symbols and Files tabs. Classes and Symbols are Go to Class and Go to Symbol over
//! the code index, Files is Warp's file search, and All merges the three into capped sections. The
//! tab order, the merging and when a project gets tabs are in `crates/warpium_search_everywhere`.

use pathfinder_color::ColorU;
pub(crate) use warpium_search_everywhere::SearchTab;
use warpui::elements::{
    Align, ChildView, Container, CornerRadius, CrossAxisAlignment, Fill, Flex, ParentElement,
    Radius, Shrinkable, Text,
};
use warpui::ui_components::components::{Coords, UiComponentStyles};
use warpui::ui_components::segmented_control::{
    LabelConfig, RenderableOptionConfig, SegmentedControl, SegmentedControlEvent,
};
use warpui::{
    AppContext, Element, Entity, SingletonEntity, TypedActionView, View, ViewContext, ViewHandle,
};

use crate::appearance::Appearance;
use crate::search::QueryFilter;

/// The palette filter that lists `tab`'s results.
pub(crate) fn tab_filter(tab: SearchTab) -> QueryFilter {
    match tab {
        SearchTab::All => QueryFilter::CodeAll,
        SearchTab::Classes => QueryFilter::CodeClasses,
        SearchTab::Symbols => QueryFilter::CodeSymbols,
        SearchTab::Files => QueryFilter::Files,
    }
}

/// The tab whose results `filter` lists, if it is one of the project search's.
pub(crate) fn filter_tab(filter: QueryFilter) -> Option<SearchTab> {
    SearchTab::ALL
        .into_iter()
        .find(|tab| tab_filter(*tab) == filter)
}

pub(crate) enum SearchTabsEvent {
    Selected(SearchTab),
}

/// The tab strip under the palette's search bar. Shown only while the palette was opened as the
/// project search and one of its tabs' filters is active.
pub(crate) struct SearchTabs {
    control: ViewHandle<SegmentedControl<SearchTab>>,
    enabled: bool,
}

impl SearchTabs {
    pub(crate) fn new(ctx: &mut ViewContext<Self>) -> Self {
        let control = ctx.add_typed_action_view(|ctx| {
            SegmentedControl::new(
                SearchTab::ALL.to_vec(),
                |tab, is_selected, app| {
                    let theme = Appearance::as_ref(app).theme();
                    let color: ColorU = if is_selected {
                        theme.accent().into()
                    } else {
                        theme.main_text_color(theme.surface_2()).into()
                    };
                    Some(RenderableOptionConfig {
                        icon_path: "",
                        icon_color: color,
                        label: Some(LabelConfig {
                            label: tab.label().into(),
                            width_override: Some(50.),
                            color,
                        }),
                        tooltip: None,
                        background: if is_selected {
                            Fill::Solid(theme.surface_3().into())
                        } else {
                            Fill::None
                        },
                    })
                },
                SearchTab::All,
                control_styles(ctx),
            )
        });
        ctx.subscribe_to_view(&control, |_, _, event, ctx| {
            let SegmentedControlEvent::OptionSelected(tab) = event;
            ctx.emit(SearchTabsEvent::Selected(*tab));
        });
        ctx.subscribe_to_model(&Appearance::handle(ctx), |me, _, _, ctx| {
            me.control.update(ctx, |control, ctx| {
                control.set_styles(control_styles(ctx), ctx);
            });
            ctx.notify();
        });
        Self {
            control,
            enabled: false,
        }
    }

    pub(crate) fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub(crate) fn set_enabled(&mut self, enabled: bool, ctx: &mut ViewContext<Self>) {
        self.enabled = enabled;
        ctx.notify();
    }

    /// Highlights `tab`.
    pub(crate) fn select(&mut self, tab: SearchTab, ctx: &mut ViewContext<Self>) {
        self.control.update(ctx, |control, ctx| {
            control.set_selected_option(tab, ctx);
        });
        ctx.notify();
    }

    /// The tab the palette should show instead of `active` on tab (shift-tab when `backward`), if
    /// the tabs are shown.
    pub(crate) fn cycled_tab(
        &self,
        active: Option<QueryFilter>,
        backward: bool,
    ) -> Option<SearchTab> {
        let tab = self.shown_tab(active)?;
        Some(if backward { tab.previous() } else { tab.next() })
    }

    /// The active tab, when the strip is shown.
    fn shown_tab(&self, active: Option<QueryFilter>) -> Option<SearchTab> {
        self.enabled.then(|| active.and_then(filter_tab)).flatten()
    }

    /// The strip, unless it is hidden (see [`SearchTabs`]).
    pub(crate) fn render_strip(
        handle: &ViewHandle<SearchTabs>,
        active: Option<QueryFilter>,
        horizontal_padding: f32,
        app: &AppContext,
    ) -> Option<Box<dyn Element>> {
        handle.as_ref(app).shown_tab(active)?;
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let hint = Text::new_inline(
            "tab / shift-tab to switch".to_owned(),
            appearance.ui_font_family(),
            appearance.ui_font_size() - 1.,
        )
        .with_color(theme.sub_text_color(theme.surface_2()).into_solid())
        .finish();
        Some(
            Container::new(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_child(ChildView::new(handle).finish())
                    .with_child(Shrinkable::new(1., Align::new(hint).right().finish()).finish())
                    .finish(),
            )
            .with_horizontal_padding(horizontal_padding)
            .with_padding_bottom(8.)
            .finish(),
        )
    }
}

fn control_styles(app: &AppContext) -> UiComponentStyles {
    let appearance = Appearance::as_ref(app);
    let theme = appearance.theme();
    UiComponentStyles {
        font_family_id: Some(appearance.ui_font_family()),
        font_size: Some(appearance.ui_font_size()),
        border_radius: Some(CornerRadius::with_all(Radius::Pixels(4.))),
        border_width: Some(1.),
        border_color: Some(Fill::Solid(theme.surface_3().into())),
        background: Some(Fill::Solid(theme.surface_2().into())),
        height: Some(24.),
        padding: Some(Coords::uniform(0.)),
        margin: Some(Coords::uniform(0.)),
        ..Default::default()
    }
}

impl Entity for SearchTabs {
    type Event = SearchTabsEvent;
}

impl View for SearchTabs {
    fn ui_name() -> &'static str {
        "WarpiumSearchTabs"
    }

    fn render(&self, _app: &AppContext) -> Box<dyn Element> {
        ChildView::new(&self.control).finish()
    }
}

impl TypedActionView for SearchTabs {
    type Action = ();
}

#[cfg(not(target_family = "wasm"))]
pub(crate) use all_tab::AllTabDataSource;

#[cfg(not(target_family = "wasm"))]
mod all_tab {
    use std::collections::HashSet;

    use code_index::SearchScope;
    use futures_lite::FutureExt as _;
    use ordered_float::OrderedFloat;
    use warpium_search_everywhere::{Entry, SECTION_LIMIT, Section, SectionResults};

    use super::*;
    use crate::search::command_palette::mixer::CommandPaletteItemAction;
    use crate::search::command_palette::separator_search_item::SeparatorSearchItem;
    use crate::search::data_source::{Query, QueryResult};
    use crate::search::item::{IconLocation, SearchItem, SearchItemDetail};
    use crate::search::mixer::{AsyncDataSource, BoxFuture, DataSourceRunErrorWrapper};
    use crate::search::result_renderer::ItemHighlightState;
    use crate::themes::theme::Fill as ThemeFill;
    use crate::warpium::code_index_palette::{
        MAX_RESULTS, indexing_footer, search_handle, search_symbols,
    };

    /// The All tab: the Classes, Symbols and Files tabs' best results under section headings.
    pub(crate) struct AllTabDataSource {
        files: Option<Box<dyn AsyncDataSource<Action = CommandPaletteItemAction>>>,
    }

    impl AllTabDataSource {
        /// `files` is the Files tab's data source; without one the All tab lists no files.
        pub(crate) fn new(
            files: Option<Box<dyn AsyncDataSource<Action = CommandPaletteItemAction>>>,
        ) -> Self {
            Self { files }
        }
    }

    impl Entity for AllTabDataSource {
        type Event = ();
    }

    impl AsyncDataSource for AllTabDataSource {
        type Action = CommandPaletteItemAction;

        fn run_query(
            &self,
            query: &Query,
            app: &AppContext,
        ) -> BoxFuture<'static, Result<Vec<QueryResult<Self::Action>>, DataSourceRunErrorWrapper>>
        {
            let handle = search_handle(app);
            // Not the Files filter, so the file search offers no "create file" row.
            let files_query = Query {
                filters: HashSet::from([QueryFilter::CodeAll]),
                text: query.text.clone(),
            };
            let files = self
                .files
                .as_ref()
                .map(|files| files.run_query(&files_query, app));
            let text = query.text.clone();
            async move {
                let mut sections = Vec::new();
                let mut footer = None;
                if let Some(handle) = &handle {
                    let classes = search_symbols(handle, &text, SearchScope::Types);
                    let symbols = search_symbols(handle, &text, SearchScope::All);
                    let symbols_truncated = symbols.len() == MAX_RESULTS;
                    sections.push(SectionResults {
                        section: Section::Classes,
                        truncated: classes.len() == MAX_RESULTS,
                        items: classes.into_iter().map(QueryResult::from).collect(),
                    });
                    sections.push(SectionResults {
                        section: Section::Symbols,
                        // Types are already in the Classes section.
                        items: symbols
                            .into_iter()
                            .filter(|item| !item.symbol.kind.is_type())
                            .map(QueryResult::from)
                            .collect(),
                        truncated: symbols_truncated,
                    });
                    footer = indexing_footer(handle);
                }
                if let Some(files) = files {
                    let mut files = files.await.unwrap_or_else(|_| {
                        log::warn!("[Search everywhere] File search failed");
                        Vec::new()
                    });
                    // Best first, with ties in the order the palette's mixer would show them.
                    files.sort_by_key(|file| (file.priority_tier(), file.score()));
                    files.reverse();
                    sections.push(SectionResults {
                        section: Section::Files,
                        items: files,
                        truncated: false,
                    });
                }

                let entries = warpium_search_everywhere::merge_sections(sections, SECTION_LIMIT);
                let count = entries.len();
                let mut results: Vec<QueryResult<CommandPaletteItemAction>> = entries
                    .into_iter()
                    .enumerate()
                    .map(|(rank, entry)| {
                        let item = match entry {
                            Entry::Header { label, .. } => {
                                QueryResult::from(SeparatorSearchItem::new(label))
                            }
                            Entry::Item(item) => item,
                        };
                        // The mixer sorts by score, so the score keeps the sections' order.
                        QueryResult::from(RankedItem {
                            item,
                            score: (count - rank) as f64,
                        })
                    })
                    .collect();
                results.extend(footer.map(QueryResult::from));
                Ok(results)
            }
            .boxed()
        }
    }

    /// Another tab's row placed on the All tab by `score`.
    struct RankedItem {
        item: QueryResult<CommandPaletteItemAction>,
        score: f64,
    }

    impl SearchItem for RankedItem {
        type Action = CommandPaletteItemAction;

        fn is_multiline(&self) -> bool {
            self.item.is_multiline()
        }

        fn render_icon(
            &self,
            highlight_state: ItemHighlightState,
            appearance: &Appearance,
        ) -> Box<dyn Element> {
            self.item.render_icon(highlight_state, appearance)
        }

        fn icon_location(&self, appearance: &Appearance) -> IconLocation {
            self.item.icon_location(appearance)
        }

        fn render_item(
            &self,
            highlight_state: ItemHighlightState,
            app: &AppContext,
        ) -> Box<dyn Element> {
            self.item.render_item(highlight_state, app)
        }

        fn item_background(
            &self,
            highlight_state: ItemHighlightState,
            appearance: &Appearance,
        ) -> Option<ThemeFill> {
            self.item.item_background(highlight_state, appearance)
        }

        fn render_details(&self, app: &AppContext) -> Option<Box<dyn Element>> {
            self.item.render_details(app)
        }

        fn score(&self) -> OrderedFloat<f64> {
            OrderedFloat(self.score)
        }

        fn accept_result(&self) -> CommandPaletteItemAction {
            self.item.accept_result()
        }

        fn execute_result(&self) -> CommandPaletteItemAction {
            self.item.execute_result()
        }

        fn accessibility_label(&self) -> String {
            self.item.accessibility_label()
        }

        fn accessibility_help_message(&self) -> Option<String> {
            self.item.accessibility_help_message()
        }

        fn is_static_separator(&self) -> bool {
            self.item.is_static_separator()
        }

        fn is_disabled(&self) -> bool {
            self.item.is_disabled()
        }

        fn tooltip(&self) -> Option<String> {
            self.item.tooltip()
        }

        fn detail_data(&self) -> Option<SearchItemDetail> {
            self.item.detail_data()
        }
    }
}

#[cfg(test)]
#[path = "search_everywhere_tests.rs"]
mod tests;
