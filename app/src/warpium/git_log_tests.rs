use warpium_git::diff::FileChangeKind;

use super::*;

#[test]
fn diff_titles_show_renames() {
    let modified = FileChange {
        path: "src/a.kt".to_owned(),
        kind: FileChangeKind::Modified,
    };
    assert_eq!(diff_title(&modified), "src/a.kt");
    let renamed = FileChange {
        path: "src/b.kt".to_owned(),
        kind: FileChangeKind::Renamed {
            from: "src/a.kt".to_owned(),
        },
    };
    assert_eq!(diff_title(&renamed), "src/a.kt → src/b.kt");
}

#[test]
fn pane_manager_only_forgets_its_own_pane() {
    use crate::pane_group::PaneId;

    let mut manager = GitLogPaneManager::default();
    let window_id = WindowId::new();
    let pane_group_id = warpui::EntityId::new();
    let pane_id = PaneId::dummy_pane_id();
    manager.register_pane(
        window_id,
        PaneViewLocator {
            pane_group_id,
            pane_id,
        },
    );
    assert!(manager.find_pane(window_id).is_some());
    manager.deregister_pane(&window_id, pane_id);
    assert!(manager.find_pane(window_id).is_none());
}
