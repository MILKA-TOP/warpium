//! The Git Log pane's side panels, laid out like JetBrains IDEs' Log tab: the branches on the
//! left, the selected commit's changed files (a tree) above its details on the right, the
//! Before/After diff viewer, the commit context menu's position and the confirmation cards.

use std::ops::Range;
use std::rc::Rc;

use warp_core::ui::appearance::Appearance;
use warp_core::ui::icons::Icon;
use warpium_git::diff::{
    DiffLine, DiffLineKind, FileChange, FileChangeKind, SideBySideRow, SideCell,
};
use warpium_git::format::local_date_time;
use warpium_git::refs::RefKind;
use warpium_git::tree::{TreeRow, TreeRowKind};
use warpui::color::ColorU;
use warpui::elements::{
    AnchorPair, Border, ChildView, Clipped, ClippedScrollable, ConstrainedBox, Container,
    CornerRadius, CrossAxisAlignment, Expanded, Fill, Flex, Hoverable, MainAxisAlignment,
    MainAxisSize, MouseStateHandle, OffsetPositioning, OffsetType, ParentElement,
    ParentOffsetBounds, PositionedElementOffsetBounds, PositioningAxis, Radius, Scrollable,
    ScrollableElement, ScrollbarWidth, Shrinkable, Text, UniformList, XAxisAnchor, YAxisAnchor,
};
use warpui::fonts::{Properties, Weight};
use warpui::platform::Cursor;
use warpui::text_layout::ClipConfig;
use warpui::ui_components::button::ButtonVariant;
use warpui::ui_components::components::UiComponent as _;
use warpui::{AppContext, Element, SingletonEntity as _};

use super::{
    BranchChoice, DETAILS_PADDING, DetailsState, DiffContent, FONT_SIZE, FileDiff, GitLogAction,
    GitLogView, LoadedDetails, PendingAction, ROW_HEIGHT, diff_title, render_ref_label,
};

const SECTION_HEADER_HEIGHT: f32 = 24.;
const TREE_INDENT: f32 = 14.;
const CARD_WIDTH: f32 = 420.;

impl GitLogView {
    /// Branches: HEAD, all branches, then local and remote branches; clicking one shows its log.
    pub(super) fn render_branches(&self, appearance: &Appearance) -> Box<dyn Element> {
        let theme = appearance.theme();
        let current = self.branch_choice();
        let mut column = Flex::column().with_child(section_header("Branches", appearance));
        column.add_child(branch_row(
            "HEAD (Current Branch)".to_owned(),
            None,
            current == BranchChoice::Head,
            false,
            self.mouse_states.head_branch.clone(),
            BranchChoice::Head,
            appearance,
        ));
        column.add_child(branch_row(
            "All branches".to_owned(),
            None,
            current == BranchChoice::All,
            false,
            self.mouse_states.all_branches.clone(),
            BranchChoice::All,
            appearance,
        ));
        let mut last_kind = None;
        for (index, branch) in self.branches.iter().enumerate() {
            if last_kind != Some(branch.kind) {
                let title = match branch.kind {
                    RefKind::RemoteBranch => "Remote",
                    RefKind::LocalBranch | RefKind::Head | RefKind::Tag | RefKind::Other => "Local",
                };
                column.add_child(section_header(title, appearance));
                last_kind = Some(branch.kind);
            }
            let choice = BranchChoice::Branch(branch.name.clone());
            let icon = match branch.kind {
                RefKind::RemoteBranch => Icon::Github,
                RefKind::LocalBranch | RefKind::Head | RefKind::Tag | RefKind::Other => {
                    Icon::GitBranch
                }
            };
            column.add_child(branch_row(
                branch.name.clone(),
                Some(icon),
                current == choice,
                branch.is_head,
                self.branch_mouse_states
                    .get(index)
                    .cloned()
                    .unwrap_or_default(),
                choice,
                appearance,
            ));
        }
        ClippedScrollable::vertical(
            self.branches_scroll.clone(),
            column.finish(),
            ScrollbarWidth::Auto,
            theme.nonactive_ui_detail().into(),
            theme.active_ui_detail().into(),
            Fill::None,
        )
        .with_overlayed_scrollbar()
        .finish()
    }

    /// The selected commit's changed files (top) and details (bottom).
    pub(super) fn render_commit_panel(&self, appearance: &Appearance) -> Box<dyn Element> {
        let theme = appearance.theme();
        let sub_color = theme.sub_text_color(theme.background()).into_solid();
        let placeholder = |text: String, color| {
            Container::new(
                Text::new(text, appearance.ui_font_family(), FONT_SIZE)
                    .with_color(color)
                    .finish(),
            )
            .with_uniform_padding(DETAILS_PADDING)
            .finish()
        };
        let loaded = match &self.details {
            DetailsState::None => {
                return placeholder("Select a commit to see its changes.".to_owned(), sub_color);
            }
            DetailsState::Loading => return placeholder("Loading…".to_owned(), sub_color),
            DetailsState::Failed(err) => return placeholder(err.clone(), theme.ui_error_color()),
            DetailsState::Loaded(loaded) => loaded,
        };
        let files_title = match &loaded.files {
            Ok(files) => format!(
                "Changes · {} file{}",
                files.len(),
                if files.len() == 1 { "" } else { "s" }
            ),
            Err(_) => "Changes".to_owned(),
        };
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(section_header(&files_title, appearance))
            .with_child(Expanded::new(1., self.render_changes(loaded, appearance)).finish())
            .with_child(
                Container::new(section_header("Commit Details", appearance))
                    .with_border(Border::top(1.).with_border_fill(theme.outline()))
                    .finish(),
            )
            .with_child(Expanded::new(1., self.render_commit_details(loaded, appearance)).finish())
            .finish()
    }

    fn render_changes(&self, loaded: &LoadedDetails, appearance: &Appearance) -> Box<dyn Element> {
        let theme = appearance.theme();
        let files = match &loaded.files {
            Ok(files) => files,
            Err(err) => {
                return Container::new(
                    Text::new(
                        format!("Couldn't list the files: {err}"),
                        appearance.ui_font_family(),
                        FONT_SIZE,
                    )
                    .with_color(theme.ui_error_color())
                    .finish(),
                )
                .with_uniform_padding(DETAILS_PADDING)
                .finish();
            }
        };
        let mut column = Flex::column();
        for row in &loaded.tree {
            column.add_child(match row.kind {
                TreeRowKind::Directory { files } => directory_row(row, files, appearance),
                TreeRowKind::File(index) => match files.get(index) {
                    Some(change) => file_row(
                        row,
                        index,
                        change,
                        loaded
                            .file_mouse_states
                            .get(index)
                            .cloned()
                            .unwrap_or_default(),
                        appearance,
                    ),
                    None => continue,
                },
            });
        }
        ClippedScrollable::vertical(
            self.files_scroll.clone(),
            Container::new(column.finish())
                .with_vertical_padding(4.)
                .finish(),
            ScrollbarWidth::Auto,
            theme.nonactive_ui_detail().into(),
            theme.active_ui_detail().into(),
            Fill::None,
        )
        .with_overlayed_scrollbar()
        .finish()
    }

    fn render_commit_details(
        &self,
        loaded: &LoadedDetails,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let theme = appearance.theme();
        let font = appearance.ui_font_family();
        let sub_color = theme.sub_text_color(theme.background()).into_solid();
        let main_color = theme.main_text_color(theme.background()).into_solid();
        let details = &loaded.details;
        let (subject, body) = match details.message.split_once('\n') {
            Some((subject, body)) => (subject.to_owned(), body.trim().to_owned()),
            None => (details.message.clone(), String::new()),
        };
        let mut column = Flex::column().with_child(
            Text::new(subject, font, FONT_SIZE + 1.)
                .with_color(main_color)
                .with_style(Properties::default().weight(Weight::Bold))
                .finish(),
        );
        if !body.is_empty() {
            column.add_child(
                Container::new(
                    Text::new(body, font, FONT_SIZE)
                        .with_color(main_color)
                        .finish(),
                )
                .with_margin_top(8.)
                .finish(),
            );
        }
        let copy = self.tool_button(
            Icon::Copy,
            "Copy Revision Number",
            self.mouse_states.copy_hash.clone(),
            GitLogAction::CopyHash,
            appearance,
        );
        column.add_child(
            Container::new(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_child(
                        Shrinkable::new(
                            1.,
                            Text::new_inline(
                                details.oid.short(),
                                appearance.monospace_font_family(),
                                FONT_SIZE,
                            )
                            .with_color(sub_color)
                            .finish(),
                        )
                        .finish(),
                    )
                    .with_child(copy)
                    .finish(),
            )
            .with_margin_top(12.)
            .finish(),
        );
        let mut people = format!(
            "{} <{}> on {}",
            details.author_name,
            details.author_email,
            local_date_time(details.author_time)
        );
        if details.committer_name != details.author_name
            || details.committer_email != details.author_email
        {
            people.push_str(&format!(
                "\ncommitted by {} on {}",
                details.committer_name,
                local_date_time(details.committer_time)
            ));
        }
        column.add_child(
            Container::new(
                Text::new(people, font, FONT_SIZE)
                    .with_color(sub_color)
                    .finish(),
            )
            .with_margin_top(4.)
            .finish(),
        );
        if !details.refs.is_empty() {
            let mut refs = Flex::row();
            for label in &details.refs {
                refs.add_child(render_ref_label(label, appearance));
            }
            column.add_child(Container::new(refs.finish()).with_margin_top(8.).finish());
        }
        ClippedScrollable::vertical(
            self.details_scroll.clone(),
            Container::new(column.finish())
                .with_uniform_padding(DETAILS_PADDING)
                .finish(),
            ScrollbarWidth::Auto,
            theme.nonactive_ui_detail().into(),
            theme.active_ui_detail().into(),
            Fill::None,
        )
        .with_overlayed_scrollbar()
        .finish()
    }

    /// The diff of a file in the selected commit, side by side (Before/After) or unified.
    pub(super) fn render_diff_viewer(
        &self,
        diff: &FileDiff,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let theme = appearance.theme();
        let font = appearance.ui_font_family();
        let mut toolbar = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(self.tool_button(
                Icon::ArrowLeft,
                "Back to the log (Esc)",
                self.mouse_states.back.clone(),
                GitLogAction::CloseDiff,
                appearance,
            ))
            .with_child(self.tool_button(
                Icon::ArrowUp,
                "Previous Difference",
                self.mouse_states.previous_difference.clone(),
                GitLogAction::JumpToDifference(false),
                appearance,
            ))
            .with_child(self.tool_button(
                Icon::ArrowDown,
                "Next Difference",
                self.mouse_states.next_difference.clone(),
                GitLogAction::JumpToDifference(true),
                appearance,
            ))
            .with_child(
                Shrinkable::new(
                    1.,
                    Container::new(
                        Text::new_inline(diff_title(&diff.change), font, FONT_SIZE)
                            .with_color(theme.main_text_color(theme.background()).into_solid())
                            .with_clip(ClipConfig::start())
                            .finish(),
                    )
                    .with_padding_left(6.)
                    .finish(),
                )
                .finish(),
            )
            .with_child(mode_button(
                "Side-by-side viewer",
                self.diff_side_by_side,
                self.mouse_states.side_by_side.clone(),
                appearance,
            ))
            .with_child(mode_button(
                "Unified viewer",
                !self.diff_side_by_side,
                self.mouse_states.unified.clone(),
                appearance,
            ));
        let working_file_exists = self
            .repo
            .as_ref()
            .is_some_and(|repo| repo.join(&diff.change.path).is_file());
        if working_file_exists {
            toolbar.add_child(self.tool_button(
                Icon::LinkExternal,
                "Open the file in the editor",
                self.mouse_states.open_file.clone(),
                GitLogAction::OpenWorkingFile(diff.change.path.clone()),
                appearance,
            ));
        }
        let toolbar = Container::new(toolbar.finish())
            .with_horizontal_padding(6.)
            .with_vertical_padding(2.)
            .with_border(Border::bottom(1.).with_border_fill(theme.outline()))
            .finish();

        let before = match (&diff.parent, &diff.change.kind) {
            (_, FileChangeKind::Added) | (None, _) => "Before: (none)".to_owned(),
            (Some(parent), kind) => format!(
                "Before: {} · {}",
                parent.short(),
                kind.old_path().unwrap_or(&diff.change.path)
            ),
        };
        let after = match diff.change.kind {
            FileChangeKind::Deleted => "After: (deleted)".to_owned(),
            _ => format!("After: {} · {}", diff.oid.short(), diff.change.path),
        };
        let caption = |text: String| {
            Container::new(
                Text::new_inline(text, font, FONT_SIZE - 1.)
                    .with_color(theme.sub_text_color(theme.background()).into_solid())
                    .with_clip(ClipConfig::start())
                    .finish(),
            )
            .with_horizontal_padding(8.)
            .with_vertical_padding(3.)
            .finish()
        };
        let captions: Box<dyn Element> = if self.diff_side_by_side {
            Flex::row()
                .with_child(Expanded::new(1., caption(before)).finish())
                .with_child(Expanded::new(1., caption(after)).finish())
                .finish()
        } else {
            caption(format!("{before}   →   {after}"))
        };

        let body: Box<dyn Element> = match &diff.content {
            DiffContent::Loading => self.render_status("Loading…", false, appearance),
            DiffContent::Failed(err) => self.render_status(err, true, appearance),
            DiffContent::Loaded { lines, .. } if lines.is_empty() => {
                self.render_status("No textual changes.", false, appearance)
            }
            DiffContent::Loaded {
                lines,
                side_by_side,
            } => {
                let list: Box<dyn ScrollableElement> = if self.diff_side_by_side {
                    let rows = side_by_side.clone();
                    let count = rows.len();
                    UniformList::new(
                        diff.list_state.clone(),
                        count,
                        move |range: Range<usize>, app: &AppContext| {
                            render_items(&rows, range, app, render_side_row)
                        },
                    )
                    .finish_scrollable()
                } else {
                    let lines = lines.clone();
                    let count = lines.len();
                    UniformList::new(
                        diff.list_state.clone(),
                        count,
                        move |range: Range<usize>, app: &AppContext| {
                            render_items(&lines, range, app, render_unified_line)
                        },
                    )
                    .finish_scrollable()
                };
                Scrollable::vertical(
                    diff.scroll_state.clone(),
                    list,
                    ScrollbarWidth::Auto,
                    theme.nonactive_ui_detail().into(),
                    theme.active_ui_detail().into(),
                    Fill::None,
                )
                .with_overlayed_scrollbar()
                .finish()
            }
        };
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(toolbar)
            .with_child(
                Container::new(captions)
                    .with_border(Border::bottom(1.).with_border_fill(theme.outline()))
                    .finish(),
            )
            .with_child(Expanded::new(1., body).finish())
            .finish()
    }

    /// Below the selected row (whose position is saved while it renders).
    pub(super) fn commit_menu_positioning(&self) -> OffsetPositioning {
        OffsetPositioning::from_axes(
            PositioningAxis::relative_to_parent(
                ParentOffsetBounds::ParentByPosition,
                OffsetType::Pixel(super::BRANCHES_WIDTH + 40.),
                AnchorPair::new(XAxisAnchor::Left, XAxisAnchor::Left),
            ),
            PositioningAxis::relative_to_stack_child(
                self.row_position_id.clone(),
                PositionedElementOffsetBounds::WindowByPosition,
                OffsetType::Pixel(0.),
                AnchorPair::new(YAxisAnchor::Bottom, YAxisAnchor::Top),
            )
            .with_conditional_anchor(),
        )
    }

    /// The confirmation or name prompt of a pending commit action.
    pub(super) fn render_pending(
        &self,
        pending: &PendingAction,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let theme = appearance.theme();
        let font = appearance.ui_font_family();
        let main = theme.main_text_color(theme.surface_1()).into_solid();
        let sub = theme.sub_text_color(theme.surface_1()).into_solid();
        let (title, body, confirm_label, danger): (String, Box<dyn Element>, &str, bool) =
            match pending {
                PendingAction::Confirm {
                    title,
                    message,
                    action,
                    ..
                } => (
                    title.clone(),
                    Text::new(message.clone(), font, FONT_SIZE)
                        .with_color(sub)
                        .finish(),
                    "OK",
                    matches!(
                        action,
                        warpium_git::actions::CommitAction::Reset(
                            warpium_git::actions::ResetMode::Hard
                        )
                    ),
                ),
                PendingAction::Reset { oid } => {
                    return self.render_reset_prompt(oid, appearance);
                }
                PendingAction::Name { tag, oid, error } => {
                    let mut column = Flex::column().with_child(
                        Container::new(
                            ConstrainedBox::new(
                                Clipped::new(ChildView::new(&self.name_editor).finish()).finish(),
                            )
                            .with_height(16.)
                            .finish(),
                        )
                        .with_uniform_padding(4.)
                        .with_border(Border::all(1.).with_border_fill(theme.surface_3()))
                        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.)))
                        .finish(),
                    );
                    if let Some(error) = error {
                        column.add_child(
                            Container::new(
                                Text::new(error.clone(), font, FONT_SIZE)
                                    .with_color(theme.ui_error_color())
                                    .finish(),
                            )
                            .with_margin_top(6.)
                            .finish(),
                        );
                    }
                    let what = if *tag { "tag" } else { "branch" };
                    (
                        format!("New {what} at {}", oid.short()),
                        column.finish(),
                        "Create",
                        false,
                    )
                }
            };
        let ui_builder = appearance.ui_builder();
        let confirm = ui_builder
            .button(
                if danger {
                    ButtonVariant::Error
                } else {
                    ButtonVariant::Accent
                },
                self.mouse_states.confirm.clone(),
            )
            .with_text_label(confirm_label.to_owned())
            .build()
            .on_click(|ctx, _, _| ctx.dispatch_typed_action(GitLogAction::ConfirmPending))
            .finish();
        let cancel = ui_builder
            .button(ButtonVariant::Outlined, self.mouse_states.cancel.clone())
            .with_text_label("Cancel".to_owned())
            .build()
            .on_click(|ctx, _, _| ctx.dispatch_typed_action(GitLogAction::CancelPending))
            .finish();
        let card = Flex::column()
            .with_child(
                Text::new(title, font, FONT_SIZE + 2.)
                    .with_color(main)
                    .finish(),
            )
            .with_child(Container::new(body).with_margin_top(8.).finish())
            .with_child(
                Container::new(
                    Flex::row()
                        .with_main_axis_size(MainAxisSize::Max)
                        .with_main_axis_alignment(MainAxisAlignment::End)
                        .with_child(cancel)
                        .with_child(Container::new(confirm).with_margin_left(8.).finish())
                        .finish(),
                )
                .with_margin_top(12.)
                .finish(),
            )
            .finish();
        ConstrainedBox::new(
            Container::new(card)
                .with_uniform_padding(14.)
                .with_background(theme.surface_1())
                .with_border(Border::all(1.).with_border_fill(theme.outline()))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.)))
                .finish(),
        )
        .with_width(CARD_WIDTH)
        .finish()
    }
}

impl GitLogView {
    /// Reset Current Branch to Here: the modes JetBrains IDEs offer, each a button.
    fn render_reset_prompt(
        &self,
        oid: &warpium_git::Oid,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        use warpium_git::actions::ResetMode;

        let theme = appearance.theme();
        let font = appearance.ui_font_family();
        let main = theme.main_text_color(theme.surface_1()).into_solid();
        let sub = theme.sub_text_color(theme.surface_1()).into_solid();
        let modes = [
            (
                ResetMode::Soft,
                "Soft",
                "Files keep their changes; the changes are staged.",
            ),
            (
                ResetMode::Mixed,
                "Mixed",
                "Files keep their changes; nothing is staged.",
            ),
            (
                ResetMode::Hard,
                "Hard",
                "Discards all local changes, staged or not.",
            ),
            (
                ResetMode::Keep,
                "Keep",
                "Keeps local changes; refuses if they touch files changed since this commit.",
            ),
        ];
        let mut column = Flex::column().with_child(
            Text::new(
                format!("Reset the current branch to {}", oid.short()),
                font,
                FONT_SIZE + 2.,
            )
            .with_color(main)
            .finish(),
        );
        for (index, (mode, label, description)) in modes.into_iter().enumerate() {
            let variant = match mode {
                ResetMode::Hard => ButtonVariant::Error,
                ResetMode::Mixed => ButtonVariant::Accent,
                ResetMode::Soft | ResetMode::Keep => ButtonVariant::Outlined,
            };
            let button = appearance
                .ui_builder()
                .button(variant, self.mouse_states.reset_modes[index].clone())
                .with_text_label(label.to_owned())
                .build()
                .on_click(move |ctx, _, _| {
                    ctx.dispatch_typed_action(GitLogAction::ConfirmReset(mode))
                })
                .finish();
            column.add_child(
                Container::new(
                    Flex::row()
                        .with_cross_axis_alignment(CrossAxisAlignment::Center)
                        .with_child(ConstrainedBox::new(button).with_width(80.).finish())
                        .with_child(
                            Shrinkable::new(
                                1.,
                                Container::new(
                                    Text::new(description.to_owned(), font, FONT_SIZE)
                                        .with_color(sub)
                                        .finish(),
                                )
                                .with_margin_left(10.)
                                .finish(),
                            )
                            .finish(),
                        )
                        .finish(),
                )
                .with_margin_top(8.)
                .finish(),
            );
        }
        let cancel = appearance
            .ui_builder()
            .button(ButtonVariant::Outlined, self.mouse_states.cancel.clone())
            .with_text_label("Cancel".to_owned())
            .build()
            .on_click(|ctx, _, _| ctx.dispatch_typed_action(GitLogAction::CancelPending))
            .finish();
        column.add_child(
            Container::new(
                Flex::row()
                    .with_main_axis_size(MainAxisSize::Max)
                    .with_main_axis_alignment(MainAxisAlignment::End)
                    .with_child(cancel)
                    .finish(),
            )
            .with_margin_top(12.)
            .finish(),
        );
        ConstrainedBox::new(
            Container::new(column.finish())
                .with_uniform_padding(14.)
                .with_background(theme.surface_1())
                .with_border(Border::all(1.).with_border_fill(theme.outline()))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.)))
                .finish(),
        )
        .with_width(CARD_WIDTH + 60.)
        .finish()
    }
}

fn render_items<T>(
    items: &Rc<Vec<T>>,
    range: Range<usize>,
    app: &AppContext,
    render: fn(&T, &Appearance) -> Box<dyn Element>,
) -> std::vec::IntoIter<Box<dyn Element>> {
    let appearance = Appearance::as_ref(app);
    range
        .filter_map(|index| items.get(index))
        .map(|item| render(item, appearance))
        .collect::<Vec<_>>()
        .into_iter()
}

fn section_header(title: &str, appearance: &Appearance) -> Box<dyn Element> {
    let theme = appearance.theme();
    ConstrainedBox::new(
        Container::new(
            Text::new_inline(
                title.to_owned(),
                appearance.ui_font_family(),
                FONT_SIZE - 1.,
            )
            .with_color(theme.sub_text_color(theme.background()).into_solid())
            .with_style(Properties::default().weight(Weight::Bold))
            .finish(),
        )
        .with_horizontal_padding(DETAILS_PADDING)
        .with_padding_top(6.)
        .finish(),
    )
    .with_height(SECTION_HEADER_HEIGHT)
    .finish()
}

#[allow(clippy::too_many_arguments)]
fn branch_row(
    label: String,
    icon: Option<Icon>,
    selected: bool,
    is_head: bool,
    mouse_state: MouseStateHandle,
    choice: BranchChoice,
    appearance: &Appearance,
) -> Box<dyn Element> {
    let theme = appearance.theme();
    let color = theme.main_text_color(theme.background()).into_solid();
    let icon_color = if is_head {
        theme.ansi_fg_yellow()
    } else {
        theme.sub_text_color(theme.background()).into_solid()
    };
    let mut row = Flex::row().with_cross_axis_alignment(CrossAxisAlignment::Center);
    if let Some(icon) = icon {
        row.add_child(
            Container::new(
                ConstrainedBox::new(icon.to_warpui_icon(icon_color.into()).finish())
                    .with_width(12.)
                    .with_height(12.)
                    .finish(),
            )
            .with_margin_right(6.)
            .finish(),
        );
    }
    let weight = if is_head {
        Weight::Bold
    } else {
        Weight::Normal
    };
    row.add_child(
        Shrinkable::new(
            1.,
            Text::new_inline(label, appearance.ui_font_family(), FONT_SIZE)
                .with_color(color)
                .with_style(Properties::default().weight(weight))
                .with_clip(ClipConfig::end())
                .finish(),
        )
        .finish(),
    );
    let row = row.finish();
    let selected_fill = theme.surface_2();
    let hover_fill = theme.surface_1();
    Hoverable::new(mouse_state, move |state| {
        let mut container =
            Container::new(ConstrainedBox::new(row).with_height(ROW_HEIGHT).finish())
                .with_horizontal_padding(DETAILS_PADDING);
        if selected {
            container = container.with_background(selected_fill);
        } else if state.is_hovered() {
            container = container.with_background(hover_fill);
        }
        container.finish()
    })
    .on_click(move |ctx, _, _| ctx.dispatch_typed_action(GitLogAction::SetBranch(choice.clone())))
    .with_cursor(Cursor::PointingHand)
    .finish()
}

fn directory_row(row: &TreeRow, files: usize, appearance: &Appearance) -> Box<dyn Element> {
    let theme = appearance.theme();
    let content = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_child(
            Shrinkable::new(
                1.,
                Text::new_inline(row.label.clone(), appearance.ui_font_family(), FONT_SIZE)
                    .with_color(theme.main_text_color(theme.background()).into_solid())
                    .with_clip(ClipConfig::start())
                    .finish(),
            )
            .finish(),
        )
        .with_child(
            Container::new(
                Text::new_inline(
                    format!("{files} file{}", if files == 1 { "" } else { "s" }),
                    appearance.ui_font_family(),
                    FONT_SIZE - 1.,
                )
                .with_color(theme.hint_text_color(theme.background()).into_solid())
                .finish(),
            )
            .with_margin_left(6.)
            .finish(),
        )
        .finish();
    Container::new(
        ConstrainedBox::new(content)
            .with_height(ROW_HEIGHT)
            .finish(),
    )
    .with_padding_left(DETAILS_PADDING + row.depth as f32 * TREE_INDENT)
    .with_padding_right(DETAILS_PADDING)
    .finish()
}

/// File names are colored by change, as in JetBrains IDEs: added green, modified blue, deleted
/// grey, renamed and copied magenta.
fn change_color(kind: &FileChangeKind, appearance: &Appearance) -> ColorU {
    let theme = appearance.theme();
    match kind {
        FileChangeKind::Added => theme.ansi_fg_green(),
        FileChangeKind::Deleted => theme.hint_text_color(theme.background()).into_solid(),
        FileChangeKind::Renamed { .. } | FileChangeKind::Copied { .. } => theme.ansi_fg_magenta(),
        FileChangeKind::Modified | FileChangeKind::TypeChanged | FileChangeKind::Other(_) => {
            theme.ansi_fg_blue()
        }
    }
}

fn file_row(
    row: &TreeRow,
    index: usize,
    change: &FileChange,
    mouse_state: MouseStateHandle,
    appearance: &Appearance,
) -> Box<dyn Element> {
    let theme = appearance.theme();
    let mut label = row.label.clone();
    if let Some(old) = change.kind.old_path() {
        label.push_str(&format!("  ← {old}"));
    }
    let content = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_child(
            ConstrainedBox::new(
                Text::new_inline(
                    change.kind.letter().to_string(),
                    appearance.monospace_font_family(),
                    FONT_SIZE,
                )
                .with_color(change_color(&change.kind, appearance))
                .with_style(Properties::default().weight(Weight::Bold))
                .finish(),
            )
            .with_width(16.)
            .finish(),
        )
        .with_child(
            Shrinkable::new(
                1.,
                Text::new_inline(label, appearance.ui_font_family(), FONT_SIZE)
                    .with_color(change_color(&change.kind, appearance))
                    .with_clip(ClipConfig::end())
                    .finish(),
            )
            .finish(),
        )
        .finish();
    let padding = DETAILS_PADDING + row.depth as f32 * TREE_INDENT;
    let hover_fill = theme.surface_1();
    Hoverable::new(mouse_state, move |state| {
        let mut container = Container::new(
            ConstrainedBox::new(content)
                .with_height(ROW_HEIGHT)
                .finish(),
        )
        .with_padding_left(padding)
        .with_padding_right(DETAILS_PADDING);
        if state.is_hovered() {
            container = container.with_background(hover_fill);
        }
        container.finish()
    })
    .on_click(move |ctx, _, _| ctx.dispatch_typed_action(GitLogAction::OpenFileDiff(index)))
    .with_cursor(Cursor::PointingHand)
    .finish()
}

fn mode_button(
    label: &'static str,
    active: bool,
    mouse_state: MouseStateHandle,
    appearance: &Appearance,
) -> Box<dyn Element> {
    let variant = if active {
        ButtonVariant::Secondary
    } else {
        ButtonVariant::Text
    };
    Container::new(
        appearance
            .ui_builder()
            .button(variant, mouse_state)
            .with_text_label(label.to_owned())
            .build()
            .on_click(move |ctx, _, _| {
                if !active {
                    ctx.dispatch_typed_action(GitLogAction::ToggleDiffMode)
                }
            })
            .finish(),
    )
    .with_margin_left(4.)
    .finish()
}

struct DiffStyle {
    font: warpui::fonts::FamilyId,
    font_size: f32,
    line_height: f32,
    hint: ColorU,
    main: ColorU,
    added: ColorU,
    removed: ColorU,
    banner: ColorU,
    banner_text: ColorU,
}

impl DiffStyle {
    fn new(appearance: &Appearance) -> Self {
        let theme = appearance.theme();
        let with_alpha = |mut color: ColorU, alpha| {
            color.a = alpha;
            color
        };
        let font_size = appearance.monospace_font_size();
        Self {
            font: appearance.monospace_font_family(),
            font_size,
            line_height: (font_size * appearance.line_height_ratio()).ceil(),
            hint: theme.hint_text_color(theme.background()).into_solid(),
            main: theme.main_text_color(theme.background()).into_solid(),
            added: with_alpha(theme.ansi_fg_green(), 48),
            removed: with_alpha(theme.ansi_fg_red(), 48),
            banner: with_alpha(theme.ansi_fg_blue(), 36),
            banner_text: theme.ansi_fg_cyan(),
        }
    }

    fn number(&self, number: Option<u32>) -> Box<dyn Element> {
        ConstrainedBox::new(
            Text::new_inline(
                number.map(|n| format!("{n:>5}")).unwrap_or_default(),
                self.font,
                self.font_size,
            )
            .with_color(self.hint)
            .finish(),
        )
        .with_width(self.font_size * 3.6)
        .finish()
    }

    fn code(&self, text: &str, color: ColorU) -> Box<dyn Element> {
        Shrinkable::new(
            1.,
            Clipped::new(
                Text::new_inline(text.replace('\t', "    "), self.font, self.font_size)
                    .with_color(color)
                    .finish(),
            )
            .finish(),
        )
        .finish()
    }

    fn row(&self, content: Box<dyn Element>, background: Option<ColorU>) -> Box<dyn Element> {
        let mut container = Container::new(
            ConstrainedBox::new(content)
                .with_height(self.line_height)
                .finish(),
        );
        if let Some(background) = background {
            container = container.with_background_color(background);
        }
        container.finish()
    }
}

fn render_unified_line(line: &DiffLine, appearance: &Appearance) -> Box<dyn Element> {
    let style = DiffStyle::new(appearance);
    let (prefix, color, background) = match line.kind {
        DiffLineKind::Added => ("+", style.main, Some(style.added)),
        DiffLineKind::Removed => ("-", style.main, Some(style.removed)),
        DiffLineKind::Hunk => (" ", style.banner_text, Some(style.banner)),
        DiffLineKind::Info => (" ", style.hint, None),
        DiffLineKind::Context => (" ", style.main, None),
    };
    let content = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_child(style.number(line.old_line))
        .with_child(style.number(line.new_line))
        .with_child(style.code(&format!("{prefix}{}", line.text), color))
        .finish();
    style.row(content, background)
}

fn render_side_row(row: &SideBySideRow, appearance: &Appearance) -> Box<dyn Element> {
    let style = DiffStyle::new(appearance);
    match row {
        SideBySideRow::Banner { kind, text } => {
            let (color, background) = match kind {
                DiffLineKind::Hunk => (style.banner_text, Some(style.banner)),
                DiffLineKind::Info
                | DiffLineKind::Context
                | DiffLineKind::Added
                | DiffLineKind::Removed => (style.hint, None),
            };
            let content = Flex::row()
                .with_child(style.number(None))
                .with_child(style.code(text, color))
                .finish();
            style.row(content, background)
        }
        SideBySideRow::Lines { left, right } => {
            let side = |cell: &Option<SideCell>, changed_fill: ColorU| -> Box<dyn Element> {
                let (number, text, background) = match cell {
                    Some(cell) => (
                        Some(cell.line),
                        cell.text.as_str(),
                        cell.changed.then_some(changed_fill),
                    ),
                    None => (None, "", None),
                };
                let content = Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_child(style.number(number))
                    .with_child(style.code(&format!(" {text}"), style.main))
                    .finish();
                style.row(content, background)
            };
            Flex::row()
                .with_child(Expanded::new(1., side(left, style.removed)).finish())
                .with_child(
                    Container::new(Expanded::new(1., side(right, style.added)).finish()).finish(),
                )
                .finish()
        }
    }
}
