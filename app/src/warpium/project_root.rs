//! Warpium: a tab's project root, following its terminals or locked to folders.
//!
//! The decision logic lives in the `warpium_project_root` crate; the state is
//! `PaneGroup::project_root`, the workspace applies changes
//! (`workspace/view/warpium_project_root.rs`), the Project Explorer header shows and toggles it
//! (`workspace/view/warpium_project_header.rs`) and the working directories model lays out locked
//! tabs (`pane_group/warpium_locked_project.rs`).

use std::path::PathBuf;

use warpui::AppContext;
use warpui::keymap::EditableBinding;

use crate::util::bindings::BindingGroup;
use crate::workspace::WorkspaceAction;

/// Workspace keymap context flag set while the active tab's project is locked.
pub(crate) const PROJECT_LOCKED_CONTEXT_FLAG: &str = "WarpiumProjectLocked";

/// Changes to the active tab's project root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectRootAction {
    /// Picks a folder and opens it in a new tab whose project is locked to it.
    OpenFolderAsProject,
    /// Locks the project to the folder the explorer currently shows for the active terminal.
    LockToCurrentFolder,
    /// Follows the tab's terminals and editors again.
    Unlock,
    /// Locks to the current folder while following, unlocks while locked.
    ToggleLock,
    /// Picks a folder and locks the project to it alone.
    ChangeFolder,
    /// Picks a folder and adds it to the project (locking it first if needed).
    AddFolder,
    /// Removes a folder from a locked project with several folders.
    RemoveFolder(PathBuf),
}

/// Registers the command palette actions (and the Open Folder as Project shortcut).
pub(crate) fn init_bindings(app: &mut AppContext) {
    use warpui::keymap::macros::*;

    let workspace = id!("Workspace");
    let locked = id!(PROJECT_LOCKED_CONTEXT_FLAG);
    let action = WorkspaceAction::WarpiumProjectRoot;
    app.register_editable_bindings([
        // Warp binds cmd-shift-O to /open-repo, so this adds alt.
        EditableBinding::new(
            "workspace:open_folder_as_project",
            "Open Folder as Project…",
            action(ProjectRootAction::OpenFolderAsProject),
        )
        .with_group(BindingGroup::Navigation.as_str())
        .with_context_predicate(workspace.clone())
        .with_mac_key_binding("cmd-alt-shift-O"),
        EditableBinding::new(
            "workspace:lock_project_to_current_folder",
            "Lock project to current folder",
            action(ProjectRootAction::LockToCurrentFolder),
        )
        .with_group(BindingGroup::Navigation.as_str())
        .with_context_predicate(workspace.clone() & !locked.clone()),
        EditableBinding::new(
            "workspace:unlock_project",
            "Unlock project (follow terminals)",
            action(ProjectRootAction::Unlock),
        )
        .with_group(BindingGroup::Navigation.as_str())
        .with_context_predicate(workspace.clone() & locked.clone()),
        EditableBinding::new(
            "workspace:change_project_folder",
            "Change project folder…",
            action(ProjectRootAction::ChangeFolder),
        )
        .with_group(BindingGroup::Navigation.as_str())
        .with_context_predicate(workspace.clone()),
        EditableBinding::new(
            "workspace:add_folder_to_project",
            "Add folder to project…",
            action(ProjectRootAction::AddFolder),
        )
        .with_group(BindingGroup::Navigation.as_str())
        .with_context_predicate(workspace & locked),
    ]);
}
