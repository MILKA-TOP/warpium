//! Warpium: the bottom panel, a per-tab dock for panes whose processes keep running while they
//! are out of the split layout (JetBrains-style tool window).
//!
//! This crate holds the dock's model (sub-tabs, selection, collapsed and maximized state, unread
//! output, reuse of a command's sub-tab), the chips' status, the saved form and the layout
//! decisions the app makes when a pane moves in or out of it. It is generic over the pane id so it
//! can be tested without the app, which owns the panes, renders them and keeps their processes
//! alive.

mod dock;
mod layout;
mod snapshot;
mod status;

pub use dock::{BottomDock, DockOrigin, DockedPane};
pub use layout::{
    Axis, LayoutNode, RestoreHint, Side, drop_side, fresh_terminal_directory, layout_anchor,
    restore_hint,
};
pub use snapshot::{EntryKind, EntrySnapshot, PanelSnapshot, decode_hex, encode_hex};
pub use status::SubTabStatus;
