use super::*;

fn leaf(id: u32) -> LayoutNode<u32> {
    LayoutNode::Leaf(id)
}

fn row(children: Vec<LayoutNode<u32>>) -> LayoutNode<u32> {
    LayoutNode::Split {
        axis: Axis::Horizontal,
        children,
    }
}

fn column(children: Vec<LayoutNode<u32>>) -> LayoutNode<u32> {
    LayoutNode::Split {
        axis: Axis::Vertical,
        children,
    }
}

#[test]
fn lone_pane_has_no_restore_hint() {
    assert_eq!(restore_hint(&leaf(1), 1), None);
}

#[test]
fn pane_missing_from_the_layout_has_no_restore_hint() {
    assert_eq!(restore_hint(&row(vec![leaf(1), leaf(2)]), 3), None);
}

#[test]
fn right_pane_of_a_row_goes_back_right_of_its_left_neighbor() {
    let layout = row(vec![leaf(1), leaf(2)]);

    assert_eq!(
        restore_hint(&layout, 2),
        Some(RestoreHint {
            anchor: 1,
            side: Side::Right
        })
    );
}

#[test]
fn first_pane_of_a_row_goes_back_left_of_its_right_neighbor() {
    let layout = row(vec![leaf(1), leaf(2), leaf(3)]);

    assert_eq!(
        restore_hint(&layout, 1),
        Some(RestoreHint {
            anchor: 2,
            side: Side::Left
        })
    );
}

#[test]
fn bottom_pane_of_a_column_goes_back_below_its_upper_neighbor() {
    let layout = column(vec![leaf(1), leaf(2)]);

    assert_eq!(
        restore_hint(&layout, 2),
        Some(RestoreHint {
            anchor: 1,
            side: Side::Down
        })
    );
}

#[test]
fn top_pane_of_a_column_goes_back_above_its_lower_neighbor() {
    let layout = column(vec![leaf(1), leaf(2)]);

    assert_eq!(
        restore_hint(&layout, 1),
        Some(RestoreHint {
            anchor: 2,
            side: Side::Up
        })
    );
}

#[test]
fn pane_in_a_nested_split_is_anchored_within_that_split() {
    let layout = row(vec![leaf(1), column(vec![leaf(2), leaf(3)])]);

    assert_eq!(
        restore_hint(&layout, 3),
        Some(RestoreHint {
            anchor: 2,
            side: Side::Down
        })
    );
}

#[test]
fn previous_sibling_split_anchors_on_its_closest_pane() {
    let layout = row(vec![column(vec![leaf(1), leaf(2)]), leaf(3)]);

    assert_eq!(
        restore_hint(&layout, 3),
        Some(RestoreHint {
            anchor: 2,
            side: Side::Right
        })
    );
}

#[test]
fn next_sibling_split_anchors_on_its_closest_pane() {
    let layout = row(vec![leaf(1), column(vec![leaf(2), leaf(3)])]);

    assert_eq!(
        restore_hint(&layout, 1),
        Some(RestoreHint {
            anchor: 2,
            side: Side::Left
        })
    );
}

#[test]
fn layout_anchor_prefers_the_most_recently_focused_visible_pane() {
    assert_eq!(layout_anchor(&[1, 2, 3], &[1, 2]), Some(2));
}

#[test]
fn layout_anchor_falls_back_to_the_first_visible_pane() {
    assert_eq!(layout_anchor(&[7, 8], &[4, 5]), Some(4));
}

#[test]
fn layout_anchor_is_none_without_visible_panes() {
    assert_eq!(layout_anchor(&[1], &[]), None);
}
