//! Warpium: git in the code editor. File editors compare the buffer with the file in `HEAD`, so
//! the gutter marks added, changed and deleted lines; clicking a marker shows the change inline
//! with Rollback, Previous and Next (Warp's diff navigation, which code review uses). Rollback…
//! restores the whole file. Annotate with Git Blame shows each line's commit left of the gutter,
//! and the inline blame setting shows the cursor line's commit at its right.

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use instant::Instant;
use warp_core::ui::appearance::Appearance;
use warp_editor::model::CoreEditorModel as _;
use warp_editor::render::model::LineCount;
use warpium_git::blame::{Blame, blame_args};
use warpium_git::format::{relative_time, truncate_chars};
use warpium_git::process::{relative_to_repo, repo_toplevel, run_git_with_input};
use warpium_git::rollback::{FileState, base_text, rollback};
use warpui::elements::{
    AnchorPair, Border, ChildAnchor, ConstrainedBox, Container, CornerRadius, CrossAxisAlignment,
    Flex, MainAxisAlignment, MainAxisSize, MouseStateHandle, OffsetPositioning, OffsetType,
    ParentAnchor, ParentElement, ParentOffsetBounds, PositionedElementOffsetBounds,
    PositioningAxis, Radius, Stack, Text, XAxisAnchor, YAxisAnchor,
};
use warpui::ui_components::button::ButtonVariant;
use warpui::ui_components::components::UiComponent as _;
use warpui::{AppContext, Element, SingletonEntity as _, TypedActionView as _, ViewContext};

use super::editor::warpium_blame_column::LineAnnotations;
use super::local_code_editor::{LocalCodeEditorAction, LocalCodeEditorView};
use crate::menu::{MenuItem, MenuItemFields};
use crate::view_components::DismissibleToast;
use crate::warpium::git_tools::{
    self, EditorGitAction, WarpiumGitAction, gutter_markers_enabled, inline_blame_enabled,
    run_in_background,
};
use crate::warpium::sync_settings::WarpiumGitSettings;
use crate::workspace::{ToastStack, WorkspaceAction};

/// Focus changes refresh the base at most this often.
const FOCUS_REFRESH_SECONDS: f32 = 2.;
const CONFIRM_WIDTH: f32 = 420.;

/// Per-editor git state.
#[derive(Default)]
pub(crate) struct WarpiumGitEditorState {
    /// Set for file editors (not code review, diffs or new files).
    enabled: bool,
    has_base: bool,
    base_generation: u64,
    last_base_refresh: Option<Instant>,
    blame: Option<Arc<Blame>>,
    blame_generation: u64,
    annotations_visible: bool,
    confirm_rollback: bool,
    rollback_mouse_state: MouseStateHandle,
    cancel_mouse_state: MouseStateHandle,
}

/// The work tree root and `path` relative to it, or `None` outside a repository.
fn locate(path: &Path) -> Option<(PathBuf, String)> {
    let toplevel = repo_toplevel(path.parent()?).ok()?;
    let relative = relative_to_repo(&toplevel, path)?;
    Some((toplevel, relative))
}

impl LocalCodeEditorView {
    /// Turns on change markers, Rollback and blame for this editor (a file editor pane).
    pub(crate) fn warpium_enable_git(&mut self, ctx: &mut ViewContext<Self>) {
        if !git_tools::is_enabled() {
            return;
        }
        self.warpium_git.enabled = true;
        self.editor.update(ctx, |editor, ctx| {
            editor.enable_warpium_change_markers("Rollback", ctx)
        });
        ctx.subscribe_to_model(&WarpiumGitSettings::handle(ctx), |me, _, _, ctx| {
            me.warpium_refresh_base(ctx);
            if inline_blame_enabled(ctx) && me.warpium_git.blame.is_none() {
                me.warpium_refresh_blame(ctx);
            }
            ctx.notify();
        });
    }

    pub(super) fn warpium_on_file_loaded(&mut self, ctx: &mut ViewContext<Self>) {
        if !self.warpium_git.enabled {
            return;
        }
        self.warpium_refresh_base(ctx);
        if inline_blame_enabled(ctx) {
            self.warpium_refresh_blame(ctx);
        }
    }

    /// The file changed on disk (saved, or rewritten by a checkout or rollback).
    pub(super) fn warpium_on_disk_change(&mut self, ctx: &mut ViewContext<Self>) {
        if !self.warpium_git.enabled {
            return;
        }
        self.warpium_refresh_base(ctx);
        if self.warpium_git.annotations_visible || inline_blame_enabled(ctx) {
            self.warpium_refresh_blame(ctx);
        }
    }

    /// Commits made elsewhere change `HEAD`; coming back to the editor picks them up.
    pub(super) fn warpium_on_focus(&mut self, ctx: &mut ViewContext<Self>) {
        if !self.warpium_git.enabled {
            return;
        }
        let recent = self
            .warpium_git
            .last_base_refresh
            .is_some_and(|last| last.elapsed().as_secs_f32() < FOCUS_REFRESH_SECONDS);
        if !recent {
            self.warpium_refresh_base(ctx);
        }
    }

    fn warpium_clear_base(&mut self, ctx: &mut ViewContext<Self>) {
        if self.warpium_git.has_base {
            self.warpium_git.has_base = false;
            self.editor
                .update(ctx, |editor, ctx| editor.clear_base(ctx));
        }
    }

    /// Compares the buffer with the file in `HEAD` (nothing for untracked files).
    fn warpium_refresh_base(&mut self, ctx: &mut ViewContext<Self>) {
        if !gutter_markers_enabled(ctx) {
            self.warpium_clear_base(ctx);
            return;
        }
        let Some(path) = self.file_path().map(Path::to_path_buf) else {
            return;
        };
        self.warpium_git.last_base_refresh = Some(Instant::now());
        self.warpium_git.base_generation += 1;
        let generation = self.warpium_git.base_generation;
        let receiver = run_in_background("git-base", move || {
            let (toplevel, relative) = locate(&path)?;
            base_text(&toplevel, &relative).ok().flatten()
        });
        ctx.spawn(
            async move { receiver.await.ok().flatten() },
            move |me, base, ctx| {
                if me.warpium_git.base_generation != generation || !gutter_markers_enabled(ctx) {
                    return;
                }
                match base {
                    Some(text) => {
                        me.warpium_git.has_base = true;
                        me.editor
                            .update(ctx, |editor, ctx| editor.set_base(&text, true, ctx));
                    }
                    None => me.warpium_clear_base(ctx),
                }
                ctx.notify();
            },
        );
    }

    /// Blames the buffer as it is now (unsaved edits show as not committed).
    fn warpium_refresh_blame(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(path) = self.file_path().map(Path::to_path_buf) else {
            return;
        };
        let contents = self.editor.as_ref(ctx).text(ctx).into_string();
        self.warpium_git.blame_generation += 1;
        let generation = self.warpium_git.blame_generation;
        let receiver = run_in_background("git-blame", move || {
            let Some((toplevel, relative)) = locate(&path) else {
                return Err("the file isn't in a git repository".to_owned());
            };
            let output =
                run_git_with_input(&toplevel, &blame_args(&relative, true), contents.as_bytes())
                    .map_err(|err| err.to_string())?;
            Blame::parse_porcelain(&String::from_utf8_lossy(&output))
        });
        ctx.spawn(
            async move { receiver.await.ok() },
            move |me, result, ctx| {
                if me.warpium_git.blame_generation != generation {
                    return;
                }
                match result {
                    Some(Ok(blame)) => {
                        let blame = Arc::new(blame);
                        me.warpium_git.blame = Some(blame.clone());
                        if me.warpium_git.annotations_visible {
                            let annotations =
                                LineAnnotations::new(blame, crate::warpium::unix_now() as i64);
                            me.editor.update(ctx, |editor, ctx| {
                                editor.set_warpium_line_annotations(Some(annotations), ctx)
                            });
                        }
                    }
                    Some(Err(err)) => {
                        me.warpium_git.blame = None;
                        if me.warpium_git.annotations_visible {
                            me.warpium_git.annotations_visible = false;
                            me.editor.update(ctx, |editor, ctx| {
                                editor.set_warpium_line_annotations(None, ctx)
                            });
                            me.warpium_toast(format!("Couldn't annotate the file: {err}"), ctx);
                        }
                    }
                    None => {}
                }
                ctx.notify();
            },
        );
    }

    fn warpium_toast(&self, message: String, ctx: &mut ViewContext<Self>) {
        let window_id = ctx.window_id();
        ToastStack::handle(ctx).update(ctx, |stack, ctx| {
            stack.add_ephemeral_toast(DismissibleToast::error(message), window_id, ctx);
        });
    }

    fn warpium_cursor_line(&self, ctx: &AppContext) -> usize {
        self.editor.as_ref(ctx).cursor_lsp_position(ctx).line
    }

    fn warpium_cursor_range(&self, ctx: &AppContext) -> Range<LineCount> {
        let line = self.warpium_cursor_line(ctx);
        LineCount::from(line)..LineCount::from(line + 1)
    }

    fn warpium_has_changes(&self, ctx: &AppContext) -> bool {
        let model = self.editor.as_ref(ctx).model.as_ref(ctx);
        self.warpium_git.has_base && model.diff().as_ref(ctx).diff_hunk_count() > 0
    }

    pub(super) fn handle_warpium_git_action(
        &mut self,
        action: &EditorGitAction,
        ctx: &mut ViewContext<Self>,
    ) {
        if !git_tools::is_enabled() {
            return;
        }
        match action {
            EditorGitAction::ToggleBlame => {
                if self.warpium_git.annotations_visible {
                    self.warpium_git.annotations_visible = false;
                    self.editor.update(ctx, |editor, ctx| {
                        editor.set_warpium_line_annotations(None, ctx)
                    });
                } else {
                    self.warpium_git.annotations_visible = true;
                    self.warpium_refresh_blame(ctx);
                }
                ctx.notify();
            }
            EditorGitAction::ShowFileHistory => {
                if let Some(path) = self.file_path().map(Path::to_path_buf) {
                    ctx.dispatch_typed_action_deferred(WorkspaceAction::WarpiumGit(
                        WarpiumGitAction::ShowFileHistory(path),
                    ));
                }
            }
            EditorGitAction::ShowCommit(oid) => {
                if let Some(path) = self.file_path().map(Path::to_path_buf) {
                    ctx.dispatch_typed_action_deferred(WorkspaceAction::WarpiumGit(
                        WarpiumGitAction::ShowCommit {
                            path,
                            oid: oid.clone(),
                        },
                    ));
                }
            }
            EditorGitAction::RollbackFile => {
                if self.file_path().is_some() {
                    self.warpium_git.confirm_rollback = true;
                    ctx.notify();
                }
            }
            EditorGitAction::CancelRollbackFile => {
                self.warpium_git.confirm_rollback = false;
                ctx.notify();
            }
            EditorGitAction::ConfirmRollbackFile => {
                self.warpium_git.confirm_rollback = false;
                self.warpium_rollback_file(ctx);
                ctx.notify();
            }
            EditorGitAction::ShowChangeAtCursor => {
                if !self.warpium_has_changes(ctx) {
                    self.warpium_toast("No changes since the last commit".to_owned(), ctx);
                    return;
                }
                let range = self.warpium_cursor_range(ctx);
                self.editor
                    .update(ctx, |editor, ctx| editor.toggle_diff_nav(Some(range), ctx));
            }
            EditorGitAction::NextChange | EditorGitAction::PreviousChange => {
                if !self.warpium_has_changes(ctx) {
                    self.warpium_toast("No changes since the last commit".to_owned(), ctx);
                    return;
                }
                let active = self
                    .editor
                    .as_ref(ctx)
                    .model
                    .as_ref(ctx)
                    .diff_nav_is_active();
                let range = self.warpium_cursor_range(ctx);
                let next = matches!(action, EditorGitAction::NextChange);
                self.editor.update(ctx, |editor, ctx| {
                    if !active {
                        editor.toggle_diff_nav(Some(range), ctx);
                    } else if next {
                        editor.navigate_next_diff_hunk(ctx);
                    } else {
                        editor.navigate_previous_diff_hunk(ctx);
                    }
                });
            }
        }
    }

    fn warpium_rollback_file(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(path) = self.file_path().map(Path::to_path_buf) else {
            return;
        };
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let receiver = run_in_background("git-rollback", move || {
            let Some((toplevel, relative)) = locate(&path) else {
                return Err("the file isn't in a git repository".to_owned());
            };
            rollback(&toplevel, &relative).map_err(|err| err.to_string())
        });
        ctx.spawn(
            async move { receiver.await.ok() },
            move |me, result, ctx| match result {
                Some(Ok(FileState::Committed | FileState::Added)) => {
                    // Reload from disk, dropping unsaved edits, so the editor shows the restored
                    // file and auto-save can't write the old edits back.
                    me.handle_action(&LocalCodeEditorAction::DiscardUnsavedChanges, ctx);
                    me.warpium_on_disk_change(ctx);
                }
                Some(Ok(FileState::Untracked)) => me.warpium_toast(
                    format!("{name} isn't tracked by git, so there is nothing to roll back to"),
                    ctx,
                ),
                Some(Err(err)) => {
                    log::warn!("[Git] Rollback failed: {err}");
                    me.warpium_toast(format!("Couldn't roll back {name}: {err}"), ctx);
                }
                None => {}
            },
        );
    }

    /// Context menu entries for file editors.
    pub(super) fn warpium_git_context_menu_items(&self) -> Vec<MenuItem<LocalCodeEditorAction>> {
        if !self.warpium_git.enabled || self.file_path().is_none() {
            return Vec::new();
        }
        let action = LocalCodeEditorAction::WarpiumGit;
        let blame_label = if self.warpium_git.annotations_visible {
            "Close Git Blame annotations"
        } else {
            "Annotate with Git Blame"
        };
        vec![
            MenuItemFields::new(blame_label)
                .with_on_select_action(action(EditorGitAction::ToggleBlame))
                .into_item(),
            MenuItemFields::new("Show history for file")
                .with_on_select_action(action(EditorGitAction::ShowFileHistory))
                .into_item(),
            MenuItemFields::new("Show change at cursor")
                .with_on_select_action(action(EditorGitAction::ShowChangeAtCursor))
                .into_item(),
            MenuItemFields::new("Rollback…")
                .with_on_select_action(action(EditorGitAction::RollbackFile))
                .into_item(),
        ]
    }

    /// The rollback confirmation and the inline blame of the cursor line.
    pub(super) fn warpium_render_overlays(&self, stack: &mut Stack, app: &AppContext) {
        if !self.warpium_git.enabled {
            return;
        }
        if self.warpium_git.confirm_rollback {
            stack.add_positioned_overlay_child(
                self.warpium_render_rollback_confirmation(app),
                OffsetPositioning::offset_from_parent(
                    warpui::geometry::vector::vec2f(0., 12.),
                    ParentOffsetBounds::ParentByPosition,
                    ParentAnchor::TopMiddle,
                    ChildAnchor::TopMiddle,
                ),
            );
        }
        if let Some(inline) = self.warpium_render_inline_blame(app) {
            let editor = self.editor.as_ref(app);
            let cursor_id = editor
                .model
                .as_ref(app)
                .render_state()
                .as_ref(app)
                .saved_positions()
                .cursor_id();
            stack.add_positioned_child(
                inline,
                OffsetPositioning::from_axes(
                    PositioningAxis::relative_to_parent(
                        ParentOffsetBounds::ParentByPosition,
                        OffsetType::Pixel(-24.),
                        AnchorPair::new(XAxisAnchor::Right, XAxisAnchor::Right),
                    ),
                    PositioningAxis::relative_to_stack_child(
                        cursor_id,
                        PositionedElementOffsetBounds::Unbounded,
                        OffsetType::Pixel(0.),
                        AnchorPair::new(YAxisAnchor::Middle, YAxisAnchor::Middle),
                    )
                    .with_conditional_anchor(),
                ),
            );
        }
    }

    fn warpium_render_inline_blame(&self, app: &AppContext) -> Option<Box<dyn Element>> {
        if !inline_blame_enabled(app) || !self.editor.as_ref(app).is_focused(app) {
            return None;
        }
        let blame = self.warpium_git.blame.as_ref()?;
        let commit = blame.line(self.warpium_cursor_line(app))?;
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let text = if commit.is_uncommitted() {
            "You · Not committed yet".to_owned()
        } else {
            format!(
                "{}, {} · {}",
                commit.author,
                relative_time(commit.author_time, crate::warpium::unix_now() as i64),
                truncate_chars(&commit.summary, 60)
            )
        };
        Some(
            Container::new(
                Text::new_inline(text, appearance.ui_font_family(), 11.)
                    .with_color(theme.hint_text_color(theme.background()).into_solid())
                    .finish(),
            )
            .with_horizontal_padding(6.)
            .with_background(theme.background())
            .finish(),
        )
    }

    fn warpium_render_rollback_confirmation(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let font = appearance.ui_font_family();
        let name = self
            .file_path()
            .and_then(Path::file_name)
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let rollback = appearance
            .ui_builder()
            .button(
                ButtonVariant::Error,
                self.warpium_git.rollback_mouse_state.clone(),
            )
            .with_text_label("Rollback".to_owned())
            .build()
            .on_click(|ctx, _, _| {
                ctx.dispatch_typed_action(LocalCodeEditorAction::WarpiumGit(
                    EditorGitAction::ConfirmRollbackFile,
                ))
            })
            .finish();
        let cancel = appearance
            .ui_builder()
            .button(
                ButtonVariant::Outlined,
                self.warpium_git.cancel_mouse_state.clone(),
            )
            .with_text_label("Cancel".to_owned())
            .build()
            .on_click(|ctx, _, _| {
                ctx.dispatch_typed_action(LocalCodeEditorAction::WarpiumGit(
                    EditorGitAction::CancelRollbackFile,
                ))
            })
            .finish();
        let body = Flex::column()
            .with_child(
                Text::new(format!("Roll back {name}?"), font, 14.)
                    .with_color(theme.main_text_color(theme.surface_1()).into_solid())
                    .finish(),
            )
            .with_child(
                Container::new(
                    Text::new(
                        "Discards every change since the last commit, staged ones too. A new \
                         file is only unstaged and stays on disk."
                            .to_owned(),
                        font,
                        12.,
                    )
                    .with_color(theme.sub_text_color(theme.surface_1()).into_solid())
                    .finish(),
                )
                .with_margin_top(6.)
                .finish(),
            )
            .with_child(
                Container::new(
                    Flex::row()
                        .with_main_axis_size(MainAxisSize::Max)
                        .with_main_axis_alignment(MainAxisAlignment::End)
                        .with_cross_axis_alignment(CrossAxisAlignment::Center)
                        .with_child(cancel)
                        .with_child(Container::new(rollback).with_margin_left(8.).finish())
                        .finish(),
                )
                .with_margin_top(12.)
                .finish(),
            )
            .finish();
        ConstrainedBox::new(
            Container::new(body)
                .with_uniform_padding(14.)
                .with_background(theme.surface_1())
                .with_border(Border::all(1.).with_border_fill(theme.outline()))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.)))
                .finish(),
        )
        .with_width(CONFIRM_WIDTH)
        .finish()
    }
}
