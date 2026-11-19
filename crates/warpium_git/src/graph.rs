//! Incremental commit graph layout, one row per commit as `git log --date-order` lists them
//! (children before their parents).
//!
//! The builder keeps the lanes open below the current row, each waiting for one commit. A
//! commit takes the leftmost lane waiting for it (lanes from other children join it), or a new
//! lane on the right when nothing waits for it (a branch tip). Its first parent continues its
//! lane; other parents join a lane already waiting for them or open one. Lanes are compacted to
//! the left after every row. Each row is two halves: segments from the top edge to the middle,
//! where the commit's node is, and from the middle to the bottom edge, so consecutive rows join
//! at their shared edge.

use crate::oid::Oid;

/// Lanes beyond this are laid out but not drawn, so a repository with thousands of open branches
/// doesn't make every row huge.
pub const MAX_DRAWN_LANES: usize = 48;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Half {
    /// From the row's top edge (at `from`) to its middle (at `to`).
    Top,
    /// From the row's middle (at `from`) to its bottom edge (at `to`).
    Bottom,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Segment {
    pub half: Half,
    pub from: u16,
    pub to: u16,
    /// An index into the app's lane palette (taken modulo its size).
    pub color: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphRow {
    /// The lane of the commit's node.
    pub node: u16,
    pub color: u16,
    pub segments: Box<[Segment]>,
    /// Lanes the row spans, at most [`MAX_DRAWN_LANES`].
    pub width: u16,
}

impl GraphRow {
    /// A row without a graph, for filtered lists.
    pub fn single() -> Self {
        Self {
            node: 0,
            color: 0,
            segments: Box::new([]),
            width: 1,
        }
    }
}

#[derive(Clone, Copy)]
struct Lane {
    waiting_for: Oid,
    color: u16,
}

#[derive(Default)]
pub struct GraphBuilder {
    lanes: Vec<Lane>,
    next_color: u16,
}

impl GraphBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Lanes open below the last row.
    pub fn open_lanes(&self) -> usize {
        self.lanes.len()
    }

    fn new_color(&mut self) -> u16 {
        let color = self.next_color;
        self.next_color = self.next_color.wrapping_add(1);
        color
    }

    pub fn push(&mut self, oid: Oid, parents: &[Oid]) -> GraphRow {
        let mut segments = Vec::new();
        let waiting = self.lanes.iter().position(|lane| lane.waiting_for == oid);
        let (node, color) = match waiting {
            Some(index) => (index, self.lanes[index].color),
            None => (self.lanes.len(), self.new_color()),
        };
        for (index, lane) in self.lanes.iter().enumerate() {
            let to = if lane.waiting_for == oid { node } else { index };
            push_segment(&mut segments, Half::Top, index, to, lane.color);
        }

        // The lanes below this row before compaction, and whether each passes the node by.
        let mut slots: Vec<Option<(Lane, bool)>> = self
            .lanes
            .drain(..)
            .map(|lane| (lane.waiting_for != oid).then_some((lane, true)))
            .collect();
        if node == slots.len() {
            slots.push(None);
        }
        let mut node_edges = Vec::new();
        if let Some(first) = parents.first() {
            slots[node] = Some((
                Lane {
                    waiting_for: *first,
                    color,
                },
                false,
            ));
            node_edges.push(node);
        }
        for parent in parents.iter().skip(1) {
            let existing = slots
                .iter()
                .position(|slot| slot.is_some_and(|(lane, _)| lane.waiting_for == *parent));
            if let Some(existing) = existing {
                if !node_edges.contains(&existing) {
                    node_edges.push(existing);
                }
                continue;
            }
            let lane = Lane {
                waiting_for: *parent,
                color: self.new_color(),
            };
            let free = slots
                .iter()
                .skip(node + 1)
                .position(Option::is_none)
                .map(|offset| node + 1 + offset);
            let slot = match free {
                Some(slot) => slot,
                None => {
                    slots.push(None);
                    slots.len() - 1
                }
            };
            slots[slot] = Some((lane, false));
            node_edges.push(slot);
        }

        let mut compacted_index = vec![usize::MAX; slots.len()];
        let mut next = 0;
        for (index, slot) in slots.iter().enumerate() {
            if slot.is_some() {
                compacted_index[index] = next;
                next += 1;
            }
        }
        for (index, slot) in slots.iter().enumerate() {
            if let Some((lane, true)) = slot {
                push_segment(
                    &mut segments,
                    Half::Bottom,
                    index,
                    compacted_index[index],
                    lane.color,
                );
            }
        }
        for slot in &node_edges {
            if let Some((lane, _)) = &slots[*slot] {
                push_segment(
                    &mut segments,
                    Half::Bottom,
                    node,
                    compacted_index[*slot],
                    lane.color,
                );
            }
        }

        let width = slots.len().max(node + 1).min(MAX_DRAWN_LANES) as u16;
        self.lanes = slots.into_iter().flatten().map(|(lane, _)| lane).collect();
        GraphRow {
            node: node.min(MAX_DRAWN_LANES - 1) as u16,
            color,
            segments: segments.into_boxed_slice(),
            width,
        }
    }
}

fn push_segment(segments: &mut Vec<Segment>, half: Half, from: usize, to: usize, color: u16) {
    if from >= MAX_DRAWN_LANES || to >= MAX_DRAWN_LANES {
        return;
    }
    segments.push(Segment {
        half,
        from: from as u16,
        to: to as u16,
        color,
    });
}

/// The endpoints of a segment in a row's local coordinates, with lanes `lane_width` apart and
/// centered in their lane.
pub fn segment_line(segment: &Segment, lane_width: f32, row_height: f32) -> [(f32, f32); 2] {
    let x = |lane: u16| (lane as f32 + 0.5) * lane_width;
    let middle = row_height / 2.;
    match segment.half {
        Half::Top => [(x(segment.from), 0.), (x(segment.to), middle)],
        Half::Bottom => [(x(segment.from), middle), (x(segment.to), row_height)],
    }
}

/// Points at most `spacing` apart along a line, both ends included, for drawing it from small
/// squares.
pub fn line_stamps(from: (f32, f32), to: (f32, f32), spacing: f32) -> Vec<(f32, f32)> {
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    let length = (dx * dx + dy * dy).sqrt();
    let steps = (length / spacing.max(0.1)).ceil().max(1.) as usize;
    (0..=steps)
        .map(|step| {
            let t = step as f32 / steps as f32;
            (from.0 + dx * t, from.1 + dy * t)
        })
        .collect()
}

#[cfg(test)]
#[path = "graph_tests.rs"]
mod tests;
