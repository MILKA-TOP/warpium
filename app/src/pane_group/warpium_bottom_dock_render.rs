//! Warpium: drawing the bottom panel, expanded (sub-tab chips over the selected pane, resizable
//! from its top edge, or maximized over the layout) or collapsed to its strip of chips.

use pathfinder_color::ColorU;
use pathfinder_geometry::rect::RectF;
use pathfinder_geometry::vector::Vector2F;
use warpium_bottom_panel::{DockedPane, SubTabStatus};
use warpui::elements::{
    AcceptedByDropTarget, Border, ChildAnchor, Clipped, ConstrainedBox, Container,
    CrossAxisAlignment, DragBarSide, Draggable, DropTarget, DropTargetData, Element, Flex,
    Hoverable, MainAxisSize, MouseStateHandle, OffsetPositioning, ParentElement,
    PositionedElementAnchor, PositionedElementOffsetBounds, Resizable, SavePosition, Shrinkable,
    Stack, Text,
};
use warpui::platform::Cursor;
use warpui::presenter::ChildView;
use warpui::text_layout::ClipConfig;
use warpui::ui_components::components::UiComponent;
use warpui::{AppContext, SingletonEntity as _};

use super::{
    BottomPanelAction, BottomPanelChipDrop, BottomPanelTabTarget, ChipStates, PaneGroup,
    PaneGroupAction, PaneId,
};
use crate::appearance::Appearance;
use crate::pane_group::pane::view::PaneDropTargetData;
use crate::ui_components::buttons::icon_button;
use crate::ui_components::icons::Icon;
use crate::workspace::{TabBarDropTargetData, TabBarLocation, VerticalTabsPaneDropTargetData};

const MIN_HEIGHT: f32 = 100.;
/// Height the layout keeps above the panel when it is dragged up.
const MIN_LAYOUT_HEIGHT: f32 = 160.;
const STRIP_HEIGHT: f32 = 28.;
const CHIP_MAX_TITLE_WIDTH: f32 = 180.;
const CHIP_FONT_SIZE: f32 = 12.;
const CHIP_ICON_SIZE: f32 = 12.;

/// Drop target data of a sub-tab chip, for reordering chips by dragging.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DockChipDropTargetData {
    pane_id: PaneId,
}

impl DropTargetData for DockChipDropTargetData {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

fn chip_position_id(pane_id: PaneId) -> String {
    format!("warpium_bottom_panel_chip:{}", pane_id.position_id())
}

fn dispatch(action: BottomPanelAction) -> PaneGroupAction {
    PaneGroupAction::WarpiumBottomPanel(action)
}

impl PaneGroup {
    /// Hook for `render`: adds the layout and, below it, the bottom panel to the pane group's
    /// column. A maximized panel takes the layout's place.
    pub(in crate::pane_group) fn add_warpium_layout_and_bottom_dock(
        &self,
        column: &mut Flex,
        layout: Box<dyn Element>,
        app: &AppContext,
    ) {
        match self.render_warpium_bottom_dock(app) {
            Some((dock, true)) => column.add_child(Shrinkable::new(1., dock).finish()),
            Some((dock, false)) => {
                column.add_child(Shrinkable::new(1., layout).finish());
                column.add_child(dock);
            }
            None => column.add_child(Shrinkable::new(1., layout).finish()),
        }
    }

    /// The panel, and whether it takes the layout's place (maximized).
    fn render_warpium_bottom_dock(&self, app: &AppContext) -> Option<(Box<dyn Element>, bool)> {
        let model = &self.warpium_bottom_dock.model;
        if model.is_empty() {
            return None;
        }
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let visible = model
            .visible_pane()
            .filter(|pane_id| self.pane_contents.contains_key(pane_id))
            // A maximized docked pane (pane maximize, not the panel's) replaces the layout.
            .filter(|pane_id| {
                !(self.is_focused_pane_maximized(app) && self.focused_pane_id(app) == *pane_id)
            });
        let strip = self.render_warpium_dock_strip(visible.is_some(), appearance, app);

        let panel = match visible {
            Some(pane_id) => {
                let content = Flex::column()
                    .with_main_axis_size(MainAxisSize::Max)
                    .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                    .with_child(strip)
                    .with_child(Shrinkable::new(1., pane_id.render(app)).finish())
                    .finish();
                let content = Container::new(content)
                    .with_border(Border::top(1.).with_border_fill(theme.outline()))
                    .finish();
                if model.is_maximized() {
                    content
                } else {
                    Resizable::new(self.warpium_bottom_dock.height.clone(), content)
                        .with_dragbar_side(DragBarSide::Top)
                        .on_resize(|ctx, _| ctx.notify())
                        .with_bounds_callback(Box::new(|window_size| {
                            let max = (window_size.y() - MIN_LAYOUT_HEIGHT).max(MIN_HEIGHT);
                            (MIN_HEIGHT, max)
                        }))
                        .finish()
                }
            }
            None => Container::new(strip)
                .with_border(Border::top(1.).with_border_fill(theme.outline()))
                .finish(),
        };

        let mut stack = Stack::new().with_child(panel);
        if let (Some(menu), Some(pane_id)) = (
            self.warpium_bottom_dock.menu.as_ref(),
            self.warpium_bottom_dock.menu_pane,
        ) {
            stack.add_positioned_overlay_child(
                ChildView::new(menu).finish(),
                OffsetPositioning::offset_from_save_position_element(
                    chip_position_id(pane_id),
                    Vector2F::new(0., -4.),
                    PositionedElementOffsetBounds::WindowByPosition,
                    PositionedElementAnchor::TopLeft,
                    ChildAnchor::BottomLeft,
                ),
            );
        }
        let maximized = visible.is_some() && model.is_maximized();
        Some((stack.finish(), maximized))
    }

    /// The row of sub-tab chips with the panel's buttons.
    fn render_warpium_dock_strip(
        &self,
        expanded: bool,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let dock = &self.warpium_bottom_dock;
        let active = dock.model.active();
        let mut chips = Flex::row().with_cross_axis_alignment(CrossAxisAlignment::Center);
        for entry in dock.model.entries() {
            let Some(states) = dock.chip_states.get(&entry.id) else {
                continue;
            };
            chips.add_child(self.render_warpium_dock_chip(
                entry,
                expanded && Some(entry.id) == active,
                expanded,
                states,
                appearance,
                app,
            ));
        }

        let buttons = &dock.buttons;
        let mut row = Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(Shrinkable::new(1., Clipped::new(chips.finish()).finish()).finish())
            .with_child(dock_button(
                Icon::Plus,
                "New terminal in bottom panel",
                buttons.new_terminal.clone(),
                BottomPanelAction::NewTerminal,
                appearance,
            ));
        if expanded {
            let (maximize_icon, maximize_tooltip) = if dock.model.is_maximized() {
                (Icon::Minimize, "Restore panel height")
            } else {
                (Icon::Maximize, "Maximize panel")
            };
            row.add_child(dock_button(
                maximize_icon,
                maximize_tooltip,
                buttons.maximize.clone(),
                BottomPanelAction::ToggleMaximized,
                appearance,
            ));
            if let Some(active) = active {
                row.add_child(dock_button(
                    Icon::ArrowUp,
                    "Restore pane to layout",
                    buttons.restore.clone(),
                    BottomPanelAction::Restore(active),
                    appearance,
                ));
            }
            row.add_child(dock_button(
                Icon::ChevronDown,
                "Collapse bottom panel",
                buttons.collapse.clone(),
                BottomPanelAction::Collapse,
                appearance,
            ));
        } else {
            row.add_child(dock_button(
                Icon::ChevronUp,
                "Expand bottom panel",
                buttons.expand.clone(),
                BottomPanelAction::Expand,
                appearance,
            ));
        }

        let mut strip = Container::new(
            ConstrainedBox::new(row.finish())
                .with_height(STRIP_HEIGHT)
                .finish(),
        )
        .with_horizontal_padding(4.);
        if expanded {
            strip = strip
                .with_border(Border::bottom(1.).with_border_fill(appearance.theme().outline()));
        }
        strip.finish()
    }

    /// The sub-tab's title: its own (a toolbar button's label), else its pane's, else its kind.
    fn warpium_sub_tab_title(&self, entry: &DockedPane<PaneId>, app: &AppContext) -> String {
        entry
            .title
            .clone()
            .or_else(|| {
                self.pane_contents
                    .get(&entry.id)
                    .map(|pane| {
                        pane.as_pane()
                            .pane_configuration()
                            .as_ref(app)
                            .title()
                            .to_owned()
                    })
                    .filter(|title| !title.is_empty())
            })
            .unwrap_or_else(|| {
                if self.is_logcat_pane(entry.id) {
                    "Logcat".to_owned()
                } else {
                    "Terminal".to_owned()
                }
            })
    }

    /// One chip: kind icon, title, status (running dot, or ✓ / ✗ with the exit code), unread
    /// output dot and, in the expanded panel, a close button. Click selects the sub-tab (expanding
    /// the panel), right click opens its menu, dragging reorders it, restores it into the layout
    /// or moves it to another tab.
    fn render_warpium_dock_chip(
        &self,
        entry: &DockedPane<PaneId>,
        is_active: bool,
        expanded: bool,
        states: &ChipStates,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let theme = appearance.theme();
        let pane_id = entry.id;
        let (status, unread) = self.chip_status(pane_id, app);
        let text_color = if is_active {
            theme.active_ui_text_color()
        } else {
            theme.nonactive_ui_text_color()
        };
        let font = appearance.ui_font_family();
        let mut row = Flex::row().with_cross_axis_alignment(CrossAxisAlignment::Center);

        let icon = if self.is_logcat_pane(pane_id) {
            Icon::Bug
        } else {
            Icon::Terminal
        };
        row.add_child(
            Container::new(
                ConstrainedBox::new(icon.to_warpui_icon(text_color).finish())
                    .with_width(CHIP_ICON_SIZE)
                    .with_height(CHIP_ICON_SIZE)
                    .finish(),
            )
            .with_margin_right(4.)
            .finish(),
        );
        if status == SubTabStatus::Running {
            row.add_child(
                Container::new(
                    Text::new_inline("●", font, CHIP_FONT_SIZE - 3.)
                        .with_color(theme.accent().into_solid())
                        .finish(),
                )
                .with_margin_right(4.)
                .finish(),
            );
        }
        row.add_child(
            ConstrainedBox::new(
                Text::new_inline(self.warpium_sub_tab_title(entry, app), font, CHIP_FONT_SIZE)
                    .with_color(text_color.into_solid())
                    .with_clip(ClipConfig::end())
                    .finish(),
            )
            .with_max_width(CHIP_MAX_TITLE_WIDTH)
            .finish(),
        );
        if let Some(label) = status.label() {
            let color: ColorU = match status {
                SubTabStatus::Failed(_) => theme.terminal_colors().normal.red,
                SubTabStatus::Idle | SubTabStatus::Running | SubTabStatus::Succeeded => {
                    theme.terminal_colors().normal.green
                }
            }
            .into();
            row.add_child(
                Container::new(
                    Text::new_inline(label, font, CHIP_FONT_SIZE)
                        .with_color(color)
                        .finish(),
                )
                .with_margin_left(4.)
                .finish(),
            );
        }
        if unread {
            row.add_child(
                Container::new(
                    Text::new_inline("•", font, CHIP_FONT_SIZE + 2.)
                        .with_color(theme.accent().into_solid())
                        .finish(),
                )
                .with_margin_left(4.)
                .finish(),
            );
        }

        let chip = Hoverable::new(states.chip.clone(), |_| {
            Container::new(row.finish())
                .with_horizontal_padding(6.)
                .with_vertical_padding(3.)
                .finish()
        })
        .on_click(move |ctx, _, _| {
            ctx.dispatch_typed_action(dispatch(BottomPanelAction::Select(pane_id)))
        })
        .on_right_click(move |ctx, _, _| {
            ctx.dispatch_typed_action(dispatch(BottomPanelAction::OpenSubTabMenu(pane_id)))
        })
        .with_cursor(Cursor::PointingHand)
        .finish();

        let mut chip_row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(chip);
        if expanded {
            chip_row.add_child(
                icon_button(appearance, Icon::X, false, states.close.clone())
                    .build()
                    .on_click(move |ctx, _, _| {
                        ctx.dispatch_typed_action(dispatch(BottomPanelAction::Close(pane_id)))
                    })
                    .with_cursor(Cursor::PointingHand)
                    .finish(),
            );
        }
        let mut container = Container::new(chip_row.finish()).with_margin_right(2.);
        if is_active {
            container = container
                .with_background(theme.surface_2())
                .with_border(Border::bottom(2.).with_border_fill(theme.accent()));
        }

        let draggable = Draggable::new(states.drag.clone(), container.finish())
            .with_drag_bounds_callback(|_, window_size| {
                Some(RectF::new(Vector2F::zero(), window_size))
            })
            .with_accepted_by_drop_target_fn(|data, _| {
                let data = data.as_any();
                if data.is::<DockChipDropTargetData>()
                    || data.is::<PaneDropTargetData>()
                    || data.is::<TabBarDropTargetData>()
                    || data.is::<VerticalTabsPaneDropTargetData>()
                {
                    AcceptedByDropTarget::Yes
                } else {
                    AcceptedByDropTarget::No
                }
            })
            .on_drop(move |ctx, _, rect, data| {
                let Some(drop) = data.and_then(|data| chip_drop(data, rect)) else {
                    return;
                };
                if drop == BottomPanelChipDrop::Chip(pane_id) {
                    return;
                }
                ctx.dispatch_typed_action(dispatch(BottomPanelAction::ChipDropped {
                    pane_id,
                    drop,
                }));
            })
            .finish();
        SavePosition::new(
            DropTarget::new(draggable, DockChipDropTargetData { pane_id }).finish(),
            &chip_position_id(pane_id),
        )
        .finish()
    }
}

/// What a chip dropped onto `data` (with the dragged chip at `rect`) asks for.
fn chip_drop(data: &dyn DropTargetData, rect: RectF) -> Option<BottomPanelChipDrop> {
    let data = data.as_any();
    if let Some(chip) = data.downcast_ref::<DockChipDropTargetData>() {
        return Some(BottomPanelChipDrop::Chip(chip.pane_id));
    }
    if let Some(pane) = data.downcast_ref::<PaneDropTargetData>() {
        let center = rect.center();
        return Some(BottomPanelChipDrop::LayoutPane {
            target: pane.pane_id(),
            x: center.x(),
            y: center.y(),
        });
    }
    let location = data
        .downcast_ref::<TabBarDropTargetData>()
        .map(|tab_bar| tab_bar.tab_bar_location)
        .or_else(|| {
            data.downcast_ref::<VerticalTabsPaneDropTargetData>()
                .map(|tab_bar| tab_bar.tab_bar_location)
        })?;
    Some(BottomPanelChipDrop::TabBar(match location {
        TabBarLocation::TabIndex(index) => BottomPanelTabTarget::Tab(index),
        TabBarLocation::AfterTabIndex(_) => BottomPanelTabTarget::NewTab,
    }))
}

fn dock_button(
    icon: Icon,
    tooltip: &'static str,
    mouse_state: MouseStateHandle,
    action: BottomPanelAction,
    appearance: &Appearance,
) -> Box<dyn Element> {
    let ui_builder = appearance.ui_builder().clone();
    icon_button(appearance, icon, false, mouse_state)
        .with_tooltip(move || ui_builder.tool_tip(tooltip.to_owned()).build().finish())
        .build()
        .on_click(move |ctx, _, _| ctx.dispatch_typed_action(dispatch(action.clone())))
        .with_cursor(Cursor::PointingHand)
        .finish()
}
