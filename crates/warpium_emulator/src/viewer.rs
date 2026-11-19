//! What a pane showing one emulator is doing, and when it should ask its [`Session`] for frames:
//! frames are requested at the pane's pixel size while the pane is painted and cancelled once it
//! hasn't been painted for [`PAUSE_AFTER_UNPAINTED`] (another tab is in front, the window is
//! hidden), so a pane nobody sees costs nothing.
//!
//! [`Session`]: crate::session::Session

use std::time::Duration;

use instant::Instant;

use crate::{
    frame_slot::Frame,
    session::{SessionError, SessionEvent},
};

/// A streaming pane that went unpainted this long is treated as hidden.
pub const PAUSE_AFTER_UNPAINTED: Duration = Duration::from_millis(500);

/// While streaming, the pane asks the window to repaint this often, so a visible pane keeps
/// proving it is visible even when nothing else redraws the window.
pub const PAINT_HEARTBEAT: Duration = Duration::from_millis(200);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ViewerStatus {
    Connecting,
    /// Connected, and no frame has arrived yet.
    WaitingForFrame,
    Showing,
    /// The emulator's display is off (it sent an empty frame).
    ScreenOff,
    Failed(SessionError),
}

/// What the pane should tell its session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamRequest {
    Stream { width: u32, height: u32 },
    Pause,
}

#[derive(Debug)]
pub struct Viewer {
    status: ViewerStatus,
    /// The size frames are being streamed at, if they are.
    streaming: Option<(u32, u32)>,
    last_paint: Option<Instant>,
    /// The pane's frame area in physical pixels at its last paint.
    paint_size: Option<(u32, u32)>,
}

impl Default for Viewer {
    fn default() -> Self {
        Self::new()
    }
}

impl Viewer {
    pub fn new() -> Self {
        Self {
            status: ViewerStatus::Connecting,
            streaming: None,
            last_paint: None,
            paint_size: None,
        }
    }

    pub fn status(&self) -> &ViewerStatus {
        &self.status
    }

    pub fn is_streaming(&self) -> bool {
        self.streaming.is_some()
    }

    fn is_connected(&self) -> bool {
        match self.status {
            ViewerStatus::WaitingForFrame | ViewerStatus::Showing | ViewerStatus::ScreenOff => true,
            ViewerStatus::Connecting | ViewerStatus::Failed(_) => false,
        }
    }

    pub fn on_session_event(
        &mut self,
        event: &SessionEvent,
        now: Instant,
    ) -> Option<StreamRequest> {
        match event {
            SessionEvent::Connected => {
                self.status = ViewerStatus::WaitingForFrame;
                self.request_if_shown(now)
            }
            SessionEvent::FrameReady => None,
            SessionEvent::Failed(err) => {
                self.status = ViewerStatus::Failed(err.clone());
                self.streaming = None;
                None
            }
        }
    }

    pub fn on_frame(&mut self, frame: &Frame) {
        if !self.is_connected() {
            return;
        }
        self.status = if frame.is_display_off() {
            ViewerStatus::ScreenOff
        } else {
            ViewerStatus::Showing
        };
    }

    /// The pane was painted with a frame area of `width`×`height` physical pixels.
    pub fn on_paint(&mut self, now: Instant, width: u32, height: u32) -> Option<StreamRequest> {
        self.last_paint = Some(now);
        self.paint_size = (width > 0 && height > 0).then_some((width, height));
        self.request_if_shown(now)
    }

    /// Called periodically while streaming with the time of the pane's latest paint.
    pub fn on_tick(&mut self, now: Instant, last_paint: Option<Instant>) -> Option<StreamRequest> {
        if let Some(last_paint) = last_paint {
            self.last_paint = Some(self.last_paint.map_or(last_paint, |t| t.max(last_paint)));
        }
        self.streaming?;
        let hidden = self
            .last_paint
            .is_none_or(|painted| now.saturating_duration_since(painted) >= PAUSE_AFTER_UNPAINTED);
        if hidden {
            self.streaming = None;
            Some(StreamRequest::Pause)
        } else {
            None
        }
    }

    fn request_if_shown(&mut self, now: Instant) -> Option<StreamRequest> {
        if !self.is_connected() {
            return None;
        }
        let (width, height) = self.paint_size?;
        let shown = self
            .last_paint
            .is_some_and(|painted| now.saturating_duration_since(painted) < PAUSE_AFTER_UNPAINTED);
        if !shown || self.streaming == Some((width, height)) {
            return None;
        }
        self.streaming = Some((width, height));
        Some(StreamRequest::Stream { width, height })
    }
}

#[cfg(test)]
#[path = "viewer_tests.rs"]
mod tests;
