//! Warpium's git tools: the Git Log pane, change markers and Rollback in the code editor, and
//! Git Blame annotations. This module holds what they share: settings, palette commands and
//! running git off the main thread. The git work itself is the `warpium_git` crate.

#![cfg_attr(target_family = "wasm", allow(dead_code, unused_imports))]

use std::path::PathBuf;

use futures::channel::oneshot;
use warpui::keymap::EditableBinding;
use warpui::{AppContext, SingletonEntity as _};

use super::sync_settings::{
    GIT_GUTTER_MARKERS_CONTEXT_FLAG, INLINE_BLAME_CONTEXT_FLAG, WarpiumGitSettings,
};
#[cfg(not(target_family = "wasm"))]
use crate::code::local_code_editor::LocalCodeEditorAction;
use crate::settings_view::ToggleSettingActionPair;
use crate::util::bindings::BindingGroup;
use crate::workspace::WorkspaceAction;

/// Workspace-level git actions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum WarpiumGitAction {
    /// Opens the Git Log pane for the active tab's project.
    ShowLog,
    /// Opens the Git Log pane listing the history of one file.
    ShowFileHistory(PathBuf),
    /// Opens the Git Log pane on a commit of the repository holding `path`.
    ShowCommit {
        path: PathBuf,
        oid: String,
    },
    ToggleGutterMarkers,
    ToggleInlineBlame,
}

/// Code editor git actions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum EditorGitAction {
    /// Shows or hides the Git Blame annotations.
    ToggleBlame,
    ShowFileHistory,
    /// Asks to roll the whole file back to `HEAD`.
    RollbackFile,
    ConfirmRollbackFile,
    CancelRollbackFile,
    /// Shows the change at the cursor inline, with Rollback, Previous and Next.
    ShowChangeAtCursor,
    NextChange,
    PreviousChange,
    /// Opens a blamed commit in the Git Log pane.
    ShowCommit(String),
}

/// Whether the git tools are offered: Warpium (OSS channel) builds.
pub(crate) fn is_enabled() -> bool {
    #[cfg(not(target_family = "wasm"))]
    {
        super::is_warpium_channel()
    }
    #[cfg(target_family = "wasm")]
    {
        false
    }
}

/// Whether the code editor shows change markers against `HEAD`: `warpium.git_gutter_markers`.
pub(crate) fn gutter_markers_enabled(ctx: &AppContext) -> bool {
    is_enabled()
        && ctx.has_singleton_model::<WarpiumGitSettings>()
        && *WarpiumGitSettings::as_ref(ctx).git_gutter_markers
}

/// Whether the code editor shows the cursor line's blame: `warpium.inline_blame`.
pub(crate) fn inline_blame_enabled(ctx: &AppContext) -> bool {
    is_enabled()
        && ctx.has_singleton_model::<WarpiumGitSettings>()
        && *WarpiumGitSettings::as_ref(ctx).inline_blame
}

/// Runs blocking git work on its own thread; the receiver gets its result (it is cancelled if
/// the thread couldn't start).
pub(crate) fn run_in_background<T: Send + 'static>(
    name: &str,
    work: impl FnOnce() -> T + Send + 'static,
) -> oneshot::Receiver<T> {
    let (sender, receiver) = oneshot::channel();
    let spawned = std::thread::Builder::new()
        .name(name.to_owned())
        .spawn(move || {
            let _ = sender.send(work());
        });
    if let Err(err) = spawned {
        log::warn!("[Git] Couldn't start a thread for {name}: {err}");
    }
    receiver
}

#[cfg(not(target_family = "wasm"))]
pub(crate) fn init_bindings(app: &mut AppContext) {
    use warpui::keymap::macros::*;

    app.register_editable_bindings([EditableBinding::new(
        "workspace:warpium_show_git_log",
        "Show Git Log",
        WorkspaceAction::WarpiumGit(WarpiumGitAction::ShowLog),
    )
    .with_group(BindingGroup::Navigation.as_str())
    .with_context_predicate(id!("Workspace"))]);

    let in_editor = id!("LocalCodeEditorView") & !id!("IMEOpen");
    let editor_action = LocalCodeEditorAction::WarpiumGit;
    app.register_editable_bindings([
        EditableBinding::new(
            "editor:warpium_show_file_history",
            "Show history for current file",
            editor_action(EditorGitAction::ShowFileHistory),
        )
        .with_group(BindingGroup::Navigation.as_str())
        .with_context_predicate(in_editor.clone()),
        EditableBinding::new(
            "editor:warpium_annotate_with_git_blame",
            "Annotate with Git Blame",
            editor_action(EditorGitAction::ToggleBlame),
        )
        .with_group(BindingGroup::Navigation.as_str())
        .with_context_predicate(in_editor.clone()),
        EditableBinding::new(
            "editor:warpium_rollback_file",
            "Rollback…",
            editor_action(EditorGitAction::RollbackFile),
        )
        .with_group(BindingGroup::Navigation.as_str())
        .with_context_predicate(in_editor.clone()),
        // JetBrains' keymap uses ctrl-alt-shift-up/down, which macOS takes for Mission Control.
        EditableBinding::new(
            "editor:warpium_next_change",
            "Next change (git)",
            editor_action(EditorGitAction::NextChange),
        )
        .with_group(BindingGroup::Navigation.as_str())
        .with_context_predicate(in_editor.clone()),
        EditableBinding::new(
            "editor:warpium_previous_change",
            "Previous change (git)",
            editor_action(EditorGitAction::PreviousChange),
        )
        .with_group(BindingGroup::Navigation.as_str())
        .with_context_predicate(in_editor.clone()),
        EditableBinding::new(
            "editor:warpium_show_change",
            "Show change at cursor (git)",
            editor_action(EditorGitAction::ShowChangeAtCursor),
        )
        .with_group(BindingGroup::Navigation.as_str())
        .with_context_predicate(in_editor),
    ]);

    ToggleSettingActionPair::add_toggle_setting_action_pairs_as_bindings(
        vec![
            ToggleSettingActionPair::new(
                "git change markers in the code editor",
                WorkspaceAction::WarpiumGit(WarpiumGitAction::ToggleGutterMarkers),
                &id!("Workspace"),
                GIT_GUTTER_MARKERS_CONTEXT_FLAG,
            ),
            ToggleSettingActionPair::new(
                "inline git blame for the current line",
                WorkspaceAction::WarpiumGit(WarpiumGitAction::ToggleInlineBlame),
                &id!("Workspace"),
                INLINE_BLAME_CONTEXT_FLAG,
            ),
        ],
        app,
    );
}
