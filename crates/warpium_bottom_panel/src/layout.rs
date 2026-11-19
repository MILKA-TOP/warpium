/// How a split lays out its children.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    /// Children side by side, left to right.
    Horizontal,
    /// Children stacked, top to bottom.
    Vertical,
}

/// Which side of its anchor a pane is put back on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
    Up,
    Down,
}

/// A simplified view of a split layout, enough to find a pane's neighbors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LayoutNode<Id> {
    Leaf(Id),
    Split {
        axis: Axis,
        children: Vec<LayoutNode<Id>>,
    },
}

/// Where a pane sat in the layout: splitting `anchor` towards `side` puts it back there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RestoreHint<Id> {
    pub anchor: Id,
    pub side: Side,
}

/// Finds where `id` sits in `root` relative to a neighboring pane: after the closest pane of its
/// previous sibling, or else before the closest pane of its next sibling. `None` when `id` is the
/// whole layout or isn't in it.
pub fn restore_hint<Id: Copy + Eq>(root: &LayoutNode<Id>, id: Id) -> Option<RestoreHint<Id>> {
    let LayoutNode::Split { axis, children } = root else {
        return None;
    };
    if let Some(index) = children
        .iter()
        .position(|child| matches!(child, LayoutNode::Leaf(leaf) if *leaf == id))
    {
        let (after, before) = match axis {
            Axis::Horizontal => (Side::Right, Side::Left),
            Axis::Vertical => (Side::Down, Side::Up),
        };
        if let Some(previous) = index.checked_sub(1).and_then(|i| children.get(i)) {
            return last_leaf(previous).map(|anchor| RestoreHint {
                anchor,
                side: after,
            });
        }
        return children
            .get(index + 1)
            .and_then(first_leaf)
            .map(|anchor| RestoreHint {
                anchor,
                side: before,
            });
    }
    children.iter().find_map(|child| restore_hint(child, id))
}

/// The pane to split next to when there is no better position: the most recently focused of the
/// `visible` panes (`history` is oldest first), or else the first visible one.
pub fn layout_anchor<Id: Copy + Eq>(history: &[Id], visible: &[Id]) -> Option<Id> {
    history
        .iter()
        .rev()
        .find(|id| visible.contains(id))
        .or_else(|| visible.first())
        .copied()
}

/// Which side of a pane at (`x`, `y`, `width`, `height`) something dropped at `point` goes on:
/// the side of the pane's center the point is furthest towards, relative to the pane's size.
pub fn drop_side(target: (f32, f32, f32, f32), point: (f32, f32)) -> Side {
    let (x, y, width, height) = target;
    let dx = (point.0 - (x + width / 2.)) / width.max(1.);
    let dy = (point.1 - (y + height / 2.)) / height.max(1.);
    if dy.abs() > dx.abs() {
        if dy > 0. { Side::Down } else { Side::Up }
    } else if dx >= 0. {
        Side::Right
    } else {
        Side::Left
    }
}

/// Where a fresh terminal that replaces the layout's last pane starts: the tab's locked project
/// root, else the working directory of the pane it replaces. `None` leaves it to the user's
/// new-session setting.
pub fn fresh_terminal_directory<P>(
    locked_project_root: Option<P>,
    replaced_pane_cwd: Option<P>,
) -> Option<P> {
    locked_project_root.or(replaced_pane_cwd)
}

fn first_leaf<Id: Copy>(node: &LayoutNode<Id>) -> Option<Id> {
    match node {
        LayoutNode::Leaf(id) => Some(*id),
        LayoutNode::Split { children, .. } => children.first().and_then(first_leaf),
    }
}

fn last_leaf<Id: Copy>(node: &LayoutNode<Id>) -> Option<Id> {
    match node {
        LayoutNode::Leaf(id) => Some(*id),
        LayoutNode::Split { children, .. } => children.last().and_then(last_leaf),
    }
}

#[cfg(test)]
#[path = "layout_tests.rs"]
mod tests;
