//! Warpium: block history for terminals docked in a tab's bottom panel.
//!
//! Saved blocks are only read for terminals with a `terminal_panes` row, which docked terminals
//! don't have (they aren't in the layout tree). Their blocks are still saved under their uuid, so
//! they are read here by the uuids in each tab's saved panel.

use std::collections::{HashMap, HashSet};

use diesel::prelude::*;
use diesel::sqlite::SqliteConnection;
use warpium_bottom_panel::{EntryKind, decode_hex};

use super::model::Block;
use super::schema;
use crate::ai::blocklist::SerializedBlockListItem;
use crate::app_state::{PaneUuid, WindowSnapshot};

/// Like the layout's terminals (`MAX_TERMINAL_BLOCKS_TO_PERSIST_PER_SESSION`).
const MAX_BLOCKS_PER_TERMINAL: usize = 100;

/// Adds the saved blocks of the docked terminals in `windows` to `block_lists`.
pub(super) fn add_docked_terminal_blocks(
    conn: &mut SqliteConnection,
    windows: &[WindowSnapshot],
    block_lists: &mut HashMap<PaneUuid, Vec<SerializedBlockListItem>>,
) -> Result<(), diesel::result::Error> {
    let uuids: HashSet<Vec<u8>> = windows
        .iter()
        .flat_map(|window| &window.tabs)
        .filter_map(|tab| tab.warpium_bottom_panel.as_ref())
        .flat_map(|panel| &panel.entries)
        .filter_map(|entry| match &entry.kind {
            EntryKind::Terminal { uuid, .. } => decode_hex(uuid),
            EntryKind::Logcat => None,
        })
        .filter(|uuid| !block_lists.contains_key(&PaneUuid(uuid.clone())))
        .collect();
    if uuids.is_empty() {
        return Ok(());
    }
    let blocks = schema::blocks::table
        .filter(schema::blocks::columns::pane_leaf_uuid.eq_any(uuids))
        .order_by(schema::blocks::columns::id.asc())
        .select(Block::as_select())
        .load::<Block>(conn)?;
    let mut docked: HashMap<PaneUuid, Vec<SerializedBlockListItem>> = HashMap::new();
    for block in blocks {
        docked
            .entry(PaneUuid(block.pane_leaf_uuid.clone()))
            .or_default()
            .push(block.into());
    }
    for (uuid, mut items) in docked {
        items.sort_by_key(|item| item.start_ts());
        let excess = items.len().saturating_sub(MAX_BLOCKS_PER_TERMINAL);
        items.drain(0..excess);
        block_lists.insert(uuid, items);
    }
    Ok(())
}
