//! Warpium: the working directories of a pane group whose project is locked to folders.
//!
//! While a tab's project follows its terminals, `refresh_working_directories_for_pane_group`
//! derives the explorer's roots from terminal and editor paths. A locked project shows its
//! folders instead, whatever the terminals do: terminals in other folders, inside containers
//! (paths that don't exist on this machine) or in SSH sessions neither add nor remove roots.
//! Terminals inside the project are still associated with their roots and repositories, so code
//! review and "cd to directory" keep working for them.

use std::collections::HashMap;
use std::path::PathBuf;

use warpium_project_root::{LockedRoots, TerminalCwd, TerminalPlacement, containing_root};
use warpui::{EntityId, ModelContext};

use super::{WorkingDirectoriesEvent, WorkingDirectoriesModel, WorkingDirectory};
use crate::code::buffer_location::LocalOrRemotePath;

impl WorkingDirectoriesModel {
    /// Refreshes a pane group whose project is locked to `roots`: they become its display roots
    /// (primary first); the repositories of the roots and of the terminals inside them become its
    /// repositories, and only terminals inside the roots are associated with roots. `roots` must
    /// be canonical, like terminal working directories.
    pub fn refresh_locked_project_for_pane_group(
        &mut self,
        pane_group_id: EntityId,
        roots: &LockedRoots,
        terminal_cwds: Vec<(EntityId, LocalOrRemotePath)>,
        focused_terminal_id: Option<EntityId>,
        ctx: &mut ModelContext<Self>,
    ) {
        let old_directories = self.working_directories_snapshot(pane_group_id);
        let old_repos: Vec<LocalOrRemotePath> = self
            .repository_roots
            .get(pane_group_id)
            .map(|repos| repos.iter().cloned().collect())
            .unwrap_or_default();
        let old_focused_repo = self.focused_repo.get(&pane_group_id).cloned().flatten();

        let terminals: Vec<(EntityId, TerminalCwd)> = terminal_cwds
            .into_iter()
            .map(|(terminal_id, cwd)| {
                let cwd = match cwd {
                    LocalOrRemotePath::Local(path) => TerminalCwd::Host(path),
                    LocalOrRemotePath::Remote(_) => TerminalCwd::NotOnHost,
                };
                (terminal_id, cwd)
            })
            .collect();
        let layout = warpium_project_root::locked_layout(roots, &terminals, focused_terminal_id);

        // Display roots are kept most recent last, so the primary root goes in last to be shown
        // (and auto-expanded) first.
        let directories = self.pane_groups.entry(pane_group_id).or_default();
        directories.clear();
        for root in layout.display_roots.iter().rev() {
            directories.insert(LocalOrRemotePath::Local(root.clone()));
        }

        let mut root_to_terminal: HashMap<LocalOrRemotePath, EntityId> = layout
            .root_terminals
            .iter()
            .map(|(root, terminal_id)| (LocalOrRemotePath::Local(root.clone()), *terminal_id))
            .collect();
        // Review panels look terminals up by repository root; the focused terminal wins.
        let mut project_terminals: Vec<(EntityId, PathBuf)> = terminals
            .iter()
            .filter_map(|(terminal_id, cwd)| match cwd {
                TerminalCwd::Host(path) if containing_root(roots.as_slice(), path).is_some() => {
                    Some((*terminal_id, path.clone()))
                }
                _ => None,
            })
            .collect();
        project_terminals.sort_by_key(|(terminal_id, _)| Some(*terminal_id) != focused_terminal_id);
        let mut terminal_repos: Vec<LocalOrRemotePath> = Vec::new();
        let mut focused_repo: Option<LocalOrRemotePath> = None;
        for (terminal_id, path) in &project_terminals {
            if let Some(repo_root) = self.get_repo_root_for_path(path, ctx) {
                let repo_root = LocalOrRemotePath::Local(repo_root);
                root_to_terminal
                    .entry(repo_root.clone())
                    .or_insert(*terminal_id);
                if Some(*terminal_id) == focused_terminal_id {
                    focused_repo = Some(repo_root.clone());
                }
                terminal_repos.push(repo_root);
            }
        }

        // The roots' repositories first, then those of terminals inside the project (a root may
        // be a folder of several repositories).
        let mut repo_roots: Vec<LocalOrRemotePath> = Vec::new();
        let root_repos = layout
            .display_roots
            .iter()
            .filter_map(|root| self.get_repo_root_for_path(root, ctx))
            .map(LocalOrRemotePath::Local);
        for repo_root in root_repos.chain(terminal_repos) {
            if !repo_roots.contains(&repo_root) {
                repo_roots.push(repo_root);
            }
        }
        // Repositories are kept most recent last too: the primary root's repository first.
        repo_roots.reverse();
        let focused_repo = focused_repo.or_else(|| {
            self.get_repo_root_for_path(&layout.focused_root, ctx)
                .map(LocalOrRemotePath::Local)
        });

        let orphaned_repos = self.repository_roots.set_paths(pane_group_id, repo_roots);
        self.directory_to_terminal
            .insert(pane_group_id, root_to_terminal);

        if old_directories != self.working_directories_snapshot(pane_group_id) {
            self.emit_directories_changed(pane_group_id, ctx);
        }
        let new_repos: Vec<LocalOrRemotePath> = self
            .repository_roots
            .get(pane_group_id)
            .map(|repos| repos.iter().cloned().collect())
            .unwrap_or_default();
        if old_repos != new_repos {
            self.drop_unused_diff_state_models(orphaned_repos, ctx);
            self.emit_repositories_changed(pane_group_id, ctx);
        }
        if old_focused_repo != focused_repo {
            self.focused_repo
                .insert(pane_group_id, focused_repo.clone());
            self.emit_focused_repo_changed(pane_group_id, focused_repo, ctx);
        }
    }

    /// Where the active terminal of the pane group is relative to its locked project, or `None`
    /// while the project follows its terminals.
    pub fn terminal_placement(&self, pane_group_id: EntityId) -> Option<TerminalPlacement> {
        self.terminal_placement.get(&pane_group_id).copied()
    }

    pub fn set_terminal_placement(
        &mut self,
        pane_group_id: EntityId,
        placement: Option<TerminalPlacement>,
        ctx: &mut ModelContext<Self>,
    ) {
        let previous = match placement {
            Some(placement) => self.terminal_placement.insert(pane_group_id, placement),
            None => self.terminal_placement.remove(&pane_group_id),
        };
        if previous != placement {
            ctx.emit(WorkingDirectoriesEvent::TerminalPlacementChanged { pane_group_id });
        }
    }

    /// The pane group's display roots with their terminals, in stored order.
    fn working_directories_snapshot(&self, pane_group_id: EntityId) -> Vec<WorkingDirectory> {
        self.pane_groups
            .get(&pane_group_id)
            .map(|dirs| {
                dirs.iter()
                    .map(|path| WorkingDirectory {
                        path: path.clone(),
                        terminal_id: self.get_terminal_id_for_root_path(pane_group_id, path),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

#[cfg(test)]
#[path = "warpium_locked_project_tests.rs"]
mod tests;
