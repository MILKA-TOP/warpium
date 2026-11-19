//! Warpium: the bottom panel in the session snapshot (`TabSnapshot::warpium_bottom_panel`).
//!
//! Docked panes aren't in the layout tree, so the tree snapshot never sees them. The panel is
//! saved on its own: sub-tab order, titles and origins, the selection, expanded/maximized state and
//! height. Terminals come back with their block history (saved under their uuid like any terminal)
//! and working directory; their processes, like everywhere in Warp, don't. Logcat starts a fresh
//! stream, and only if the window has no Logcat pane yet.

use std::collections::HashMap;
use std::path::PathBuf;

use warpium_bottom_panel::{
    BottomDock, DockedPane, EntryKind, EntrySnapshot, PanelSnapshot, decode_hex, encode_hex,
};
use warpui::ViewContext;
use warpui::elements::resizable_state_handle;

use super::{PaneGroup, PaneId, is_enabled};
use crate::ai::blocklist::SerializedBlockListItem;
use crate::app_state::{LeafContents, PaneUuid};
use crate::pane_group::{AnyPaneContent, TerminalPane, TerminalViewResources};
use crate::terminal::shared_session::IsSharedSessionCreator;

impl PaneGroup {
    /// The panel's saved form, or `None` when nothing is docked.
    pub(crate) fn warpium_bottom_panel_snapshot(
        &self,
        app: &warpui::AppContext,
    ) -> Option<PanelSnapshot> {
        let dock = &self.warpium_bottom_dock.model;
        let mut entries = Vec::new();
        let mut active = None;
        for entry in dock.entries() {
            let Some(pane) = self.pane_contents.get(&entry.id) else {
                continue;
            };
            let kind = match pane.as_pane().snapshot(app) {
                LeafContents::Terminal(terminal) => EntryKind::Terminal {
                    uuid: encode_hex(&terminal.uuid),
                    cwd: terminal.cwd,
                },
                LeafContents::WarpiumLogcat => EntryKind::Logcat,
                _ => continue,
            };
            if dock.active() == Some(entry.id) {
                active = Some(entries.len());
            }
            entries.push(EntrySnapshot {
                kind,
                title: entry.title.clone(),
                origin: entry.origin.clone(),
            });
        }
        if entries.is_empty() {
            return None;
        }
        let height = self
            .warpium_bottom_dock
            .height
            .lock()
            .map(|state| state.size())
            .ok();
        Some(PanelSnapshot {
            height,
            expanded: dock.is_expanded(),
            maximized: dock.is_maximized(),
            active,
            entries,
        })
    }

    /// Rebuilds the panel of a restored tab. Entries that can't be restored (an invalid uuid, a
    /// second Logcat in the window) are skipped.
    pub(crate) fn restore_warpium_bottom_panel(
        &mut self,
        snapshot: &PanelSnapshot,
        block_lists: &HashMap<PaneUuid, Vec<SerializedBlockListItem>>,
        ctx: &mut ViewContext<Self>,
    ) {
        if snapshot.is_empty() || !is_enabled(ctx) || !self.warpium_bottom_dock.model.is_empty() {
            return;
        }
        let mut restored = Vec::with_capacity(snapshot.entries.len());
        for entry in &snapshot.entries {
            let pane = match &entry.kind {
                EntryKind::Terminal { uuid, cwd } => {
                    self.restored_docked_terminal(uuid, cwd.as_deref(), block_lists, ctx)
                }
                EntryKind::Logcat => self.restored_docked_logcat(ctx),
            };
            let docked = pane
                .and_then(|pane| self.attach_child_pane_off_tree(pane, ctx))
                .map(|pane_id| {
                    let generation = self.output_generation(pane_id, ctx);
                    DockedPane::new(pane_id, entry.origin.clone(), generation)
                        .with_title(entry.title.clone())
                });
            restored.push(docked);
        }
        let dock = BottomDock::from_restored(
            restored,
            snapshot.active,
            snapshot.expanded,
            snapshot.maximized,
        );
        let pane_ids: Vec<PaneId> = dock.ids().collect();
        self.warpium_bottom_dock.model = dock;
        if let Some(height) = snapshot.height {
            self.warpium_bottom_dock.height = resizable_state_handle(height);
        }
        for pane_id in pane_ids {
            self.register_docked_pane(pane_id, ctx);
        }
        ctx.notify();
    }

    fn restored_docked_terminal(
        &self,
        uuid: &str,
        cwd: Option<&str>,
        block_lists: &HashMap<PaneUuid, Vec<SerializedBlockListItem>>,
        ctx: &mut ViewContext<Self>,
    ) -> Option<Box<dyn AnyPaneContent>> {
        let Some(uuid) = decode_hex(uuid) else {
            log::warn!("[Bottom panel] Saved terminal has an invalid uuid; not restoring it");
            return None;
        };
        let startup_directory = cwd.map(PathBuf::from).filter(|path| path.is_dir());
        let resources = TerminalViewResources {
            tips_completed: self.tips_completed.clone(),
            server_api: self.server_api.clone(),
            model_event_sender: self.model_event_sender.clone(),
        };
        let view_bounds = Self::estimated_view_bounds(ctx);
        let (view, terminal_manager) = PaneGroup::create_session(
            startup_directory,
            HashMap::new(),
            uuid.as_slice(),
            IsSharedSessionCreator::No,
            resources,
            block_lists.get(&PaneUuid(uuid.clone())),
            None, /* conversation_restoration */
            self.user_default_shell_unsupported_banner_model_handle
                .clone(),
            view_bounds.size(),
            self.model_event_sender.clone(),
            None, /* chosen_shell */
            None, /* initial_input_config */
            ctx,
        );
        let pane = TerminalPane::new(
            uuid,
            terminal_manager,
            view,
            self.model_event_sender.clone(),
            ctx,
        );
        Some(Box::new(pane))
    }

    #[cfg(not(target_family = "wasm"))]
    fn restored_docked_logcat(
        &self,
        ctx: &mut ViewContext<Self>,
    ) -> Option<Box<dyn AnyPaneContent>> {
        use warpui::SingletonEntity as _;

        use crate::warpium::android::AndroidModel;
        use crate::warpium::logcat::{LogcatPaneManager, default_package};

        if !crate::warpium::android::is_enabled(ctx)
            || LogcatPaneManager::as_ref(ctx)
                .find_pane(ctx.window_id())
                .is_some()
        {
            return None;
        }
        let project_root = self.project_root().primary().map(|root| root.to_path_buf());
        AndroidModel::handle(ctx).update(ctx, |android, ctx| {
            android.ensure_started(project_root.as_deref(), ctx)
        });
        let package = project_root.as_deref().and_then(default_package);
        Some(Box::new(crate::pane_group::LogcatPane::new(
            project_root,
            package,
            ctx,
        )))
    }

    #[cfg(target_family = "wasm")]
    fn restored_docked_logcat(
        &self,
        _ctx: &mut ViewContext<Self>,
    ) -> Option<Box<dyn AnyPaneContent>> {
        None
    }
}
