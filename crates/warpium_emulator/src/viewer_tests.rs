use std::time::Duration;

use bytes::Bytes;
use instant::Instant;

use super::*;
use crate::geometry::DisplayRotation;

fn frame(width: u32, height: u32) -> Frame {
    Frame {
        width,
        height,
        rotation: DisplayRotation::Portrait,
        seq: 1,
        timestamp_us: 0,
        pixels: Bytes::from(vec![0; width as usize * height as usize * 4]),
    }
}

fn ms(millis: u64) -> Duration {
    Duration::from_millis(millis)
}

fn connected_viewer(now: Instant) -> Viewer {
    let mut viewer = Viewer::new();
    viewer.on_session_event(&SessionEvent::Connected, now);
    viewer
}

#[test]
fn starts_connecting_without_requesting_frames() {
    let now = Instant::now();
    let mut viewer = Viewer::new();

    let request = viewer.on_paint(now, 800, 1600);

    assert_eq!(request, None);
    assert_eq!(viewer.status(), &ViewerStatus::Connecting);
}

#[test]
fn connecting_after_a_recent_paint_requests_frames_at_the_painted_size() {
    let start = Instant::now();
    let mut viewer = Viewer::new();
    viewer.on_paint(start, 800, 1600);

    let request = viewer.on_session_event(&SessionEvent::Connected, start + ms(100));

    assert_eq!(
        request,
        Some(StreamRequest::Stream {
            width: 800,
            height: 1600
        })
    );
    assert_eq!(viewer.status(), &ViewerStatus::WaitingForFrame);
}

#[test]
fn connecting_long_after_the_last_paint_waits_for_the_next_paint() {
    let start = Instant::now();
    let mut viewer = Viewer::new();
    viewer.on_paint(start, 800, 1600);

    let request = viewer.on_session_event(&SessionEvent::Connected, start + ms(2000));

    assert_eq!(request, None);
    assert!(!viewer.is_streaming());
}

#[test]
fn paint_when_connected_requests_frames() {
    let now = Instant::now();
    let mut viewer = connected_viewer(now);

    let request = viewer.on_paint(now, 720, 1280);

    assert_eq!(
        request,
        Some(StreamRequest::Stream {
            width: 720,
            height: 1280
        })
    );
    assert!(viewer.is_streaming());
}

#[test]
fn repaint_at_the_same_size_does_not_request_again() {
    let now = Instant::now();
    let mut viewer = connected_viewer(now);
    viewer.on_paint(now, 720, 1280);

    let request = viewer.on_paint(now + ms(16), 720, 1280);

    assert_eq!(request, None);
}

#[test]
fn resized_paint_requests_the_new_size() {
    let now = Instant::now();
    let mut viewer = connected_viewer(now);
    viewer.on_paint(now, 720, 1280);

    let request = viewer.on_paint(now + ms(16), 600, 1280);

    assert_eq!(
        request,
        Some(StreamRequest::Stream {
            width: 600,
            height: 1280
        })
    );
}

#[test]
fn empty_paint_area_does_not_request_frames() {
    let now = Instant::now();
    let mut viewer = connected_viewer(now);

    let request = viewer.on_paint(now, 0, 1280);

    assert_eq!(request, None);
}

#[test]
fn tick_keeps_streaming_while_the_pane_is_painted() {
    let start = Instant::now();
    let mut viewer = connected_viewer(start);
    viewer.on_paint(start, 720, 1280);

    let request = viewer.on_tick(start + ms(700), Some(start + ms(400)));

    assert_eq!(request, None);
    assert!(viewer.is_streaming());
}

#[test]
fn tick_pauses_after_500ms_without_paint() {
    let start = Instant::now();
    let mut viewer = connected_viewer(start);
    viewer.on_paint(start, 720, 1280);

    let request = viewer.on_tick(start + ms(500), Some(start));

    assert_eq!(request, Some(StreamRequest::Pause));
    assert!(!viewer.is_streaming());
}

#[test]
fn tick_when_paused_does_nothing() {
    let start = Instant::now();
    let mut viewer = connected_viewer(start);
    viewer.on_paint(start, 720, 1280);
    viewer.on_tick(start + ms(600), Some(start));

    let request = viewer.on_tick(start + ms(900), Some(start));

    assert_eq!(request, None);
}

#[test]
fn paint_after_a_pause_resumes_at_the_same_size() {
    let start = Instant::now();
    let mut viewer = connected_viewer(start);
    viewer.on_paint(start, 720, 1280);
    viewer.on_tick(start + ms(600), Some(start));

    let request = viewer.on_paint(start + ms(5000), 720, 1280);

    assert_eq!(
        request,
        Some(StreamRequest::Stream {
            width: 720,
            height: 1280
        })
    );
}

#[test]
fn frames_switch_between_showing_and_screen_off() {
    let now = Instant::now();
    let mut viewer = connected_viewer(now);

    viewer.on_frame(&frame(4, 8));
    let after_frame = viewer.status().clone();
    viewer.on_frame(&frame(0, 0));

    assert_eq!(after_frame, ViewerStatus::Showing);
    assert_eq!(viewer.status(), &ViewerStatus::ScreenOff);
}

#[test]
fn failure_stops_streaming_and_ignores_later_paints_and_frames() {
    let now = Instant::now();
    let mut viewer = connected_viewer(now);
    viewer.on_paint(now, 720, 1280);

    viewer.on_session_event(&SessionEvent::Failed(SessionError::Exited), now);
    let request = viewer.on_paint(now + ms(16), 600, 1000);
    viewer.on_frame(&frame(4, 8));

    assert_eq!(request, None);
    assert!(!viewer.is_streaming());
    assert_eq!(viewer.status(), &ViewerStatus::Failed(SessionError::Exited));
}
