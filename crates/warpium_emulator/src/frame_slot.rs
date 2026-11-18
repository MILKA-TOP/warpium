//! A single-slot mailbox between the frame stream and the renderer: the producer overwrites the
//! latest frame, the consumer takes it, and frames the consumer never saw are dropped.

use std::sync::{
    Mutex, PoisonError,
    atomic::{AtomicU64, Ordering},
};

use bytes::Bytes;

use crate::geometry::DisplayRotation;

/// One RGBA8888 frame from `streamScreenshot`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub rotation: DisplayRotation,
    /// Monotonic, with gaps where the emulator skipped frames.
    pub seq: u32,
    /// When the emulator produced the frame, in its own clock.
    pub timestamp_us: u64,
    /// `width * height * 4` bytes, row-major RGBA.
    pub pixels: Bytes,
}

impl Frame {
    /// The emulator sends one empty frame when the display turns off.
    pub fn is_display_off(&self) -> bool {
        self.width == 0 || self.height == 0
    }
}

#[derive(Debug, Default)]
pub struct FrameSlot {
    latest: Mutex<Option<Frame>>,
    dropped: AtomicU64,
}

impl FrameSlot {
    pub fn new() -> Self {
        Self::default()
    }

    /// Stores `frame`, replacing an untaken one. Returns `true` when the slot was empty, i.e. the
    /// consumer has caught up and needs a notification; otherwise one is already pending.
    pub fn put(&self, frame: Frame) -> bool {
        let replaced = self
            .latest
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .replace(frame);
        match replaced {
            Some(_) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
                false
            }
            None => true,
        }
    }

    pub fn take(&self) -> Option<Frame> {
        self.latest
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }

    /// Frames replaced before the consumer took them.
    pub fn dropped_frames(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
#[path = "frame_slot_tests.rs"]
mod tests;
