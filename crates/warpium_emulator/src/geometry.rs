//! Maps pointer positions on the rendered frame to emulator display coordinates, accounting for
//! aspect-fit scaling, letterboxing and display rotation.

use crate::proto::rotation::SkinRotation;

/// A point in logical (scale-independent) view coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

/// A rectangle in logical view coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// The emulator display size in physical pixels, in its natural (unrotated) orientation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DisplaySize {
    pub width: u32,
    pub height: u32,
}

/// A position on the emulator display in physical pixels, natural orientation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DisplayPoint {
    pub x: i32,
    pub y: i32,
}

/// The quadrant the display is rotated into; frames arrive already rotated.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum DisplayRotation {
    #[default]
    Portrait,
    /// Rotated 90°.
    Landscape,
    ReversePortrait,
    /// Rotated −90°.
    ReverseLandscape,
}

impl DisplayRotation {
    pub fn from_proto(rotation: i32) -> Self {
        match SkinRotation::try_from(rotation) {
            Ok(SkinRotation::Portrait) | Err(_) => Self::Portrait,
            Ok(SkinRotation::Landscape) => Self::Landscape,
            Ok(SkinRotation::ReversePortrait) => Self::ReversePortrait,
            Ok(SkinRotation::ReverseLandscape) => Self::ReverseLandscape,
        }
    }

    /// The z-axis angle for `setPhysicalModel(ROTATION)`, matching Android Studio's rotate
    /// actions.
    pub fn physical_angle(self) -> f32 {
        match self {
            Self::Portrait => 0.0,
            Self::Landscape => 90.0,
            Self::ReversePortrait => -180.0,
            Self::ReverseLandscape => -90.0,
        }
    }

    pub fn is_landscape(self) -> bool {
        match self {
            Self::Landscape | Self::ReverseLandscape => true,
            Self::Portrait | Self::ReversePortrait => false,
        }
    }
}

/// The largest rectangle with the frame's aspect ratio that fits in `container`, centered.
/// `None` for an empty frame or container.
pub fn fit_frame(container: Rect, frame_width: u32, frame_height: u32) -> Option<Rect> {
    if frame_width == 0 || frame_height == 0 || container.width <= 0.0 || container.height <= 0.0 {
        return None;
    }
    let scale = (container.width / frame_width as f32).min(container.height / frame_height as f32);
    let width = frame_width as f32 * scale;
    let height = frame_height as f32 * scale;
    Some(Rect {
        x: container.x + (container.width - width) / 2.0,
        y: container.y + (container.height - height) / 2.0,
        width,
        height,
    })
}

/// Maps a point on the frame drawn in `frame_rect` to display coordinates. `None` when the
/// point is outside the frame.
pub fn map_to_display(
    point: Point,
    frame_rect: Rect,
    display: DisplaySize,
    rotation: DisplayRotation,
) -> Option<DisplayPoint> {
    let (u, v) = normalized(point, frame_rect)?;
    if !(0.0..=1.0).contains(&u) || !(0.0..=1.0).contains(&v) {
        return None;
    }
    Some(rotate_into_display(u, v, display, rotation))
}

/// Like [`map_to_display`], but clamps points outside the frame to its edge, for drags that leave
/// the frame (the caller then lifts the finger).
pub fn map_to_display_clamped(
    point: Point,
    frame_rect: Rect,
    display: DisplaySize,
    rotation: DisplayRotation,
) -> Option<DisplayPoint> {
    let (u, v) = normalized(point, frame_rect)?;
    Some(rotate_into_display(
        u.clamp(0.0, 1.0),
        v.clamp(0.0, 1.0),
        display,
        rotation,
    ))
}

fn normalized(point: Point, frame_rect: Rect) -> Option<(f32, f32)> {
    if frame_rect.width <= 0.0 || frame_rect.height <= 0.0 {
        return None;
    }
    Some((
        (point.x - frame_rect.x) / frame_rect.width,
        (point.y - frame_rect.y) / frame_rect.height,
    ))
}

/// `u`/`v` are in `0..=1` across the rendered (rotated) frame; the formulas follow Android
/// Studio's `EmulatorView`.
fn rotate_into_display(
    u: f32,
    v: f32,
    display: DisplaySize,
    rotation: DisplayRotation,
) -> DisplayPoint {
    let (x, y) = match rotation {
        DisplayRotation::Portrait => (u, v),
        DisplayRotation::Landscape => (1.0 - v, u),
        DisplayRotation::ReversePortrait => (1.0 - u, 1.0 - v),
        DisplayRotation::ReverseLandscape => (v, 1.0 - u),
    };
    DisplayPoint {
        x: to_pixel(x, display.width),
        y: to_pixel(y, display.height),
    }
}

fn to_pixel(fraction: f32, extent: u32) -> i32 {
    let max = extent.saturating_sub(1) as f32;
    (fraction * extent as f32).round().clamp(0.0, max) as i32
}

#[cfg(test)]
#[path = "geometry_tests.rs"]
mod tests;
