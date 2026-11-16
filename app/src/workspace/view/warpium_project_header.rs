//! Warpium: the Project Explorer's project header. It shows whether the tab's project follows its
//! terminals or is locked to folders, toggles between the two, changes and adds folders, and
//! hints when the active terminal is outside a locked project.

use std::path::Path;

use warp_core::ui::appearance::Appearance;
use warp_core::ui::icons::Icon;
use warpium_project_root::{ProjectRoot, TerminalPlacement};
use warpui::elements::{
    ConstrainedBox, Container, CrossAxisAlignment, Element, Flex, MainAxisSize, MouseStateHandle,
    ParentElement, Shrinkable, Text,
};
use warpui::platform::Cursor;
use warpui::ui_components::components::UiComponent;

use crate::ui_components::buttons::icon_button_with_color;
use crate::warpium::project_root::ProjectRootAction;
use crate::workspace::WorkspaceAction;

const FONT_SIZE: f32 = 12.;
const HINT_FONT_SIZE: f32 = 11.;
const SMALL_ICON_SIZE: f32 = 12.;
/// Extra folders that get a remove button; more are still listed.
const MAX_REMOVABLE_ROOTS: usize = 8;
/// Lines the extra folders and the hint up with the root name, past the pin button.
const INDENT: f32 = 28.;

/// Mouse states of the header's buttons, kept by the left panel across renders.
pub(super) struct ProjectHeaderMouseStates {
    toggle: MouseStateHandle,
    change: MouseStateHandle,
    add: MouseStateHandle,
    remove: Vec<MouseStateHandle>,
}

impl Default for ProjectHeaderMouseStates {
    fn default() -> Self {
        Self {
            toggle: Default::default(),
            change: Default::default(),
            add: Default::default(),
            remove: (0..MAX_REMOVABLE_ROOTS)
                .map(|_| Default::default())
                .collect(),
        }
    }
}

/// Renders the header for a tab's `project_root`; `placement` is where its active terminal is
/// relative to a locked project.
pub(super) fn render_project_header(
    project_root: &ProjectRoot,
    placement: Option<TerminalPlacement>,
    mouse_states: &ProjectHeaderMouseStates,
    appearance: &Appearance,
) -> Box<dyn Element> {
    let theme = appearance.theme();
    let main_color = theme.main_text_color(theme.background());
    let sub_color = theme.sub_text_color(theme.background());

    let (label, label_color) = match project_root.primary() {
        Some(root) => (folder_name(root), main_color),
        None => ("Following terminals".to_owned(), sub_color),
    };
    let toggle_tooltip = match project_root.primary() {
        Some(root) => format!(
            "Locked to {}. Click to unlock (follow terminals)",
            root.display()
        ),
        None => "Following terminals. Click to lock the project to the current folder".to_owned(),
    };
    let toggle_icon = if project_root.is_locked() {
        Icon::PinFilled
    } else {
        Icon::Pin
    };

    let mut title_row = Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_child(header_button(
            toggle_icon,
            toggle_tooltip,
            ProjectRootAction::ToggleLock,
            mouse_states.toggle.clone(),
            appearance,
        ))
        .with_child(
            Shrinkable::new(
                1.,
                Container::new(
                    Text::new_inline(label, appearance.ui_font_family(), FONT_SIZE)
                        .with_color(label_color.into_solid())
                        .finish(),
                )
                .with_margin_left(4.)
                .finish(),
            )
            .finish(),
        );
    if project_root.is_locked() {
        title_row = title_row.with_child(header_button(
            Icon::Folder,
            "Change project folder…".to_owned(),
            ProjectRootAction::ChangeFolder,
            mouse_states.change.clone(),
            appearance,
        ));
    }
    title_row = title_row.with_child(header_button(
        Icon::Plus,
        "Add folder to project…".to_owned(),
        ProjectRootAction::AddFolder,
        mouse_states.add.clone(),
        appearance,
    ));

    let mut column = Flex::column()
        .with_main_axis_size(MainAxisSize::Min)
        .with_child(title_row.finish());

    // Every folder but the primary one, each removable.
    for (index, root) in project_root.roots().iter().enumerate().skip(1) {
        let mut row = Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(
                ConstrainedBox::new(Icon::Folder.to_warpui_icon(sub_color).finish())
                    .with_width(SMALL_ICON_SIZE)
                    .with_height(SMALL_ICON_SIZE)
                    .finish(),
            )
            .with_child(
                Shrinkable::new(
                    1.,
                    Container::new(
                        Text::new_inline(folder_name(root), appearance.ui_font_family(), FONT_SIZE)
                            .with_color(main_color.into_solid())
                            .finish(),
                    )
                    .with_margin_left(4.)
                    .finish(),
                )
                .finish(),
            );
        if let Some(mouse_state) = mouse_states.remove.get(index - 1) {
            row = row.with_child(header_button(
                Icon::X,
                format!("Remove {} from the project", root.display()),
                ProjectRootAction::RemoveFolder(root.clone()),
                mouse_state.clone(),
                appearance,
            ));
        }
        column = column.with_child(
            Container::new(row.finish())
                .with_padding_left(INDENT)
                .finish(),
        );
    }

    if let Some(hint) = placement.and_then(TerminalPlacement::indicator_text) {
        column = column.with_child(
            Container::new(
                Text::new_inline(hint, appearance.ui_font_family(), HINT_FONT_SIZE)
                    .with_color(sub_color.into_solid())
                    .finish(),
            )
            .with_padding_left(INDENT)
            .with_padding_bottom(4.)
            .finish(),
        );
    }

    Container::new(column.finish())
        .with_padding_left(6.)
        .with_padding_right(6.)
        .with_padding_bottom(2.)
        .finish()
}

fn header_button(
    icon: Icon,
    tooltip: String,
    action: ProjectRootAction,
    mouse_state: MouseStateHandle,
    appearance: &Appearance,
) -> Box<dyn Element> {
    let theme = appearance.theme();
    let ui_builder = appearance.ui_builder().clone();
    icon_button_with_color(
        appearance,
        icon,
        false,
        mouse_state,
        theme.sub_text_color(theme.background()),
    )
    .with_tooltip(move || ui_builder.tool_tip(tooltip.clone()).build().finish())
    .build()
    .on_click(move |ctx, _, _| {
        ctx.dispatch_typed_action(WorkspaceAction::WarpiumProjectRoot(action.clone()));
    })
    .with_cursor(Cursor::PointingHand)
    .finish()
}

/// The folder's name, or the whole path for a root such as `/`.
fn folder_name(root: &Path) -> String {
    root.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| root.display().to_string())
}
