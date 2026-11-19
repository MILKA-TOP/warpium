use bytes::Bytes;

use super::*;

fn frame(seq: u32) -> Frame {
    Frame {
        width: 1,
        height: 1,
        rotation: DisplayRotation::Portrait,
        seq,
        timestamp_us: 0,
        pixels: Bytes::from_static(&[1, 2, 3, 4]),
    }
}

#[test]
fn first_frame_requests_notification() {
    let slot = FrameSlot::new();

    assert!(slot.put(frame(1)));
}

#[test]
fn frame_arriving_before_take_does_not_notify_again() {
    let slot = FrameSlot::new();
    slot.put(frame(1));

    assert!(!slot.put(frame(2)));
}

#[test]
fn slow_consumer_gets_only_latest_frame() {
    let slot = FrameSlot::new();
    slot.put(frame(1));
    slot.put(frame(2));
    slot.put(frame(3));

    let taken = slot.take();

    assert_eq!(taken.map(|frame| frame.seq), Some(3));
    assert_eq!(slot.take(), None);
    assert_eq!(slot.dropped_frames(), 2);
}

#[test]
fn frame_after_take_notifies_again() {
    let slot = FrameSlot::new();
    slot.put(frame(1));
    slot.take();

    assert!(slot.put(frame(2)));
    assert_eq!(slot.dropped_frames(), 0);
}

#[test]
fn display_off_frame_is_empty() {
    let off = Frame {
        width: 0,
        height: 0,
        pixels: Bytes::new(),
        ..frame(1)
    };

    assert!(off.is_display_off());
    assert!(!frame(2).is_display_off());
}
