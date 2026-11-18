use super::*;

const DISPLAY: DisplaySize = DisplaySize {
    width: 1000,
    height: 2000,
};
const PORTRAIT_FRAME: Rect = Rect {
    x: 100.0,
    y: 50.0,
    width: 250.0,
    height: 500.0,
};
const LANDSCAPE_FRAME: Rect = Rect {
    x: 0.0,
    y: 0.0,
    width: 500.0,
    height: 250.0,
};

fn point(x: f32, y: f32) -> Point {
    Point { x, y }
}

fn display_point(x: i32, y: i32) -> DisplayPoint {
    DisplayPoint { x, y }
}

#[test]
fn portrait_maps_scaled_offset_point() {
    let mapped = map_to_display(
        point(150.0, 150.0),
        PORTRAIT_FRAME,
        DISPLAY,
        DisplayRotation::Portrait,
    );

    assert_eq!(mapped, Some(display_point(200, 400)));
}

#[test]
fn portrait_maps_center_to_display_center() {
    let mapped = map_to_display(
        point(225.0, 300.0),
        PORTRAIT_FRAME,
        DISPLAY,
        DisplayRotation::Portrait,
    );

    assert_eq!(mapped, Some(display_point(500, 1000)));
}

#[test]
fn landscape_swaps_axes() {
    let mapped = map_to_display(
        point(100.0, 50.0),
        LANDSCAPE_FRAME,
        DISPLAY,
        DisplayRotation::Landscape,
    );

    assert_eq!(mapped, Some(display_point(800, 400)));
}

#[test]
fn reverse_portrait_flips_both_axes() {
    let frame = Rect {
        x: 0.0,
        y: 0.0,
        width: 250.0,
        height: 500.0,
    };

    let mapped = map_to_display(
        point(50.0, 100.0),
        frame,
        DISPLAY,
        DisplayRotation::ReversePortrait,
    );

    assert_eq!(mapped, Some(display_point(800, 1600)));
}

#[test]
fn reverse_landscape_swaps_and_flips() {
    let mapped = map_to_display(
        point(100.0, 50.0),
        LANDSCAPE_FRAME,
        DISPLAY,
        DisplayRotation::ReverseLandscape,
    );

    assert_eq!(mapped, Some(display_point(200, 1600)));
}

#[test]
fn frame_corners_map_inside_display() {
    let top_left = map_to_display(
        point(100.0, 50.0),
        PORTRAIT_FRAME,
        DISPLAY,
        DisplayRotation::Portrait,
    );
    let bottom_right = map_to_display(
        point(350.0, 550.0),
        PORTRAIT_FRAME,
        DISPLAY,
        DisplayRotation::Portrait,
    );

    assert_eq!(top_left, Some(display_point(0, 0)));
    assert_eq!(bottom_right, Some(display_point(999, 1999)));
}

#[test]
fn point_outside_frame_is_ignored() {
    let mapped = map_to_display(
        point(99.0, 300.0),
        PORTRAIT_FRAME,
        DISPLAY,
        DisplayRotation::Portrait,
    );

    assert_eq!(mapped, None);
}

#[test]
fn fit_frame_pillarboxes_tall_frame_in_wide_container() {
    let container = Rect {
        x: 0.0,
        y: 0.0,
        width: 800.0,
        height: 600.0,
    };

    let fitted = fit_frame(container, 1000, 2000);

    assert_eq!(
        fitted,
        Some(Rect {
            x: 250.0,
            y: 0.0,
            width: 300.0,
            height: 600.0
        })
    );
}

#[test]
fn fit_frame_letterboxes_frame_in_tall_container() {
    let container = Rect {
        x: 0.0,
        y: 0.0,
        width: 400.0,
        height: 1200.0,
    };

    let fitted = fit_frame(container, 1000, 2000);

    assert_eq!(
        fitted,
        Some(Rect {
            x: 0.0,
            y: 200.0,
            width: 400.0,
            height: 800.0
        })
    );
}

#[test]
fn fit_frame_of_empty_frame_is_none() {
    let container = Rect {
        x: 0.0,
        y: 0.0,
        width: 400.0,
        height: 1200.0,
    };

    assert_eq!(fit_frame(container, 0, 2000), None);
}

#[test]
fn click_in_pillarbox_bar_is_ignored() {
    let frame = Rect {
        x: 250.0,
        y: 0.0,
        width: 300.0,
        height: 600.0,
    };

    let mapped = map_to_display(
        point(100.0, 300.0),
        frame,
        DISPLAY,
        DisplayRotation::Portrait,
    );

    assert_eq!(mapped, None);
}

#[test]
fn click_in_letterboxed_frame_maps_through_offset() {
    let frame = Rect {
        x: 0.0,
        y: 200.0,
        width: 400.0,
        height: 800.0,
    };

    let in_bar = map_to_display(
        point(200.0, 100.0),
        frame,
        DISPLAY,
        DisplayRotation::Portrait,
    );
    let in_frame = map_to_display(
        point(200.0, 600.0),
        frame,
        DISPLAY,
        DisplayRotation::Portrait,
    );

    assert_eq!(in_bar, None);
    assert_eq!(in_frame, Some(display_point(500, 1000)));
}

#[test]
fn drag_past_left_edge_clamps_to_edge() {
    let mapped = map_to_display_clamped(
        point(-50.0, 300.0),
        PORTRAIT_FRAME,
        DISPLAY,
        DisplayRotation::Portrait,
    );

    assert_eq!(mapped, Some(display_point(0, 1000)));
}

#[test]
fn drag_past_corner_in_landscape_clamps_to_display_corner() {
    let mapped = map_to_display_clamped(
        point(600.0, -10.0),
        LANDSCAPE_FRAME,
        DISPLAY,
        DisplayRotation::Landscape,
    );

    assert_eq!(mapped, Some(display_point(999, 1999)));
}

#[test]
fn unknown_proto_rotation_falls_back_to_portrait() {
    assert_eq!(DisplayRotation::from_proto(1), DisplayRotation::Landscape);
    assert_eq!(DisplayRotation::from_proto(42), DisplayRotation::Portrait);
}

#[test]
fn physical_angles_match_android_studio() {
    assert_eq!(DisplayRotation::Portrait.physical_angle(), 0.0);
    assert_eq!(DisplayRotation::Landscape.physical_angle(), 90.0);
    assert_eq!(DisplayRotation::ReversePortrait.physical_angle(), -180.0);
    assert_eq!(DisplayRotation::ReverseLandscape.physical_angle(), -90.0);
}
