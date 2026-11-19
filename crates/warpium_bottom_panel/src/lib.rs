//! Warpium: the bottom panel, a per-tab dock for panes whose processes keep running while they
//! are out of the split layout (JetBrains-style tool window).
//!
//! This crate holds the dock's model and the layout decisions the app makes when a pane moves in
//! or out of it. It is generic over the pane id so it can be tested without the app, which owns
//! the panes, renders them and keeps their processes alive.

mod dock;
mod layout;

pub use dock::{BottomDock, DockedPane};
pub use layout::{Axis, LayoutNode, RestoreHint, Side, layout_anchor, restore_hint};
