//! A connection to one emulator on its own thread: it streams frames into a [`FrameSlot`] while
//! the viewer asks for them, keeps the emulator's idle timer reset while streaming, and reports
//! what happened through a callback. Dropping the [`Session`] cancels everything and ends the
//! thread.

use std::{
    future, io,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use tokio::{
    sync::mpsc,
    time::{Instant, sleep_until},
};

use crate::{
    client::{ClientError, EmulatorClient, FrameStream, KEEPALIVE_INTERVAL},
    discovery::RunningEmulator,
    frame_slot::{Frame, FrameSlot},
};

/// Timing knobs, overridable for tests.
#[derive(Clone, Copy, Debug)]
pub struct SessionOptions {
    pub keepalive_interval: Duration,
    /// How long a new frame size must hold before the stream is restarted for it, so dragging a
    /// pane edge doesn't restart the stream on every layout.
    pub resize_debounce: Duration,
    /// Delay before re-requesting frames after the emulator ended a stream on its own.
    pub restart_delay: Duration,
}

impl Default for SessionOptions {
    fn default() -> Self {
        Self {
            keepalive_interval: KEEPALIVE_INTERVAL,
            resize_debounce: Duration::from_millis(150),
            restart_delay: Duration::from_secs(1),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionEvent {
    /// The endpoint accepted the token; frames can be requested.
    Connected,
    /// The slot went from empty to holding a frame.
    FrameReady,
    /// The session ended; no further events follow.
    Failed(SessionError),
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SessionError {
    #[error("the emulator's gRPC endpoint requires TLS")]
    TlsRequired,
    #[error("the emulator rejected the gRPC token")]
    Unauthorized,
    #[error("couldn't connect to the emulator's gRPC endpoint")]
    Unreachable,
    #[error("the emulator exited")]
    Exited,
    #[error("{0}")]
    Other(String),
}

impl SessionError {
    /// Maps a client failure. `connected` tells a refused connection (the endpoint was never
    /// reached) from one that dropped later (the emulator went away).
    fn from_client(err: &ClientError, connected: bool) -> Self {
        if let ClientError::Rpc(status) = err
            && matches!(
                status.code(),
                tonic::Code::Unauthenticated | tonic::Code::PermissionDenied
            )
        {
            return Self::Unauthorized;
        }
        match err {
            ClientError::TlsRequired => Self::TlsRequired,
            err if err.is_disconnect() => {
                if connected {
                    Self::Exited
                } else {
                    Self::Unreachable
                }
            }
            ClientError::Rpc(status) => Self::Other(format!(
                "emulator call failed: {} ({:?})",
                status.message(),
                status.code()
            )),
            err => Self::Other(err.to_string()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Command {
    Stream { width: u32, height: u32 },
    Pause,
}

pub struct Session {
    commands: mpsc::UnboundedSender<Command>,
    slot: Arc<FrameSlot>,
    frames_received: Arc<AtomicU64>,
}

impl Session {
    /// Connects to `emulator` on a new thread. `on_event` is called on that thread.
    pub fn start(
        emulator: RunningEmulator,
        options: SessionOptions,
        on_event: impl Fn(SessionEvent) + Send + 'static,
    ) -> io::Result<Self> {
        let (commands, receiver) = mpsc::unbounded_channel();
        let slot = Arc::new(FrameSlot::new());
        let frames_received = Arc::new(AtomicU64::new(0));
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let worker = Worker {
            options,
            commands: receiver,
            slot: slot.clone(),
            frames_received: frames_received.clone(),
            on_event: Box::new(on_event),
        };
        std::thread::Builder::new()
            .name(format!("warpium-emulator-{}", emulator.console_port))
            .spawn(move || {
                runtime.block_on(worker.run(emulator));
                log::debug!("[Emulator session] Thread finished");
            })?;
        Ok(Self {
            commands,
            slot,
            frames_received,
        })
    }

    /// Streams frames scaled to fit `width`×`height` physical pixels, starting or resizing the
    /// stream as needed.
    pub fn stream(&self, width: u32, height: u32) {
        let _ = self.commands.send(Command::Stream { width, height });
    }

    /// Cancels the frame stream (the emulator stops reading back its screen) and the keepalive.
    pub fn pause(&self) {
        let _ = self.commands.send(Command::Pause);
    }

    /// The latest frame not taken yet.
    pub fn take_frame(&self) -> Option<Frame> {
        self.slot.take()
    }

    pub fn frames_received(&self) -> u64 {
        self.frames_received.load(Ordering::Relaxed)
    }

    /// Frames replaced in the slot before they were taken.
    pub fn frames_dropped(&self) -> u64 {
        self.slot.dropped_frames()
    }
}

struct Worker {
    options: SessionOptions,
    commands: mpsc::UnboundedReceiver<Command>,
    slot: Arc<FrameSlot>,
    frames_received: Arc<AtomicU64>,
    on_event: Box<dyn Fn(SessionEvent) + Send>,
}

impl Worker {
    async fn run(mut self, emulator: RunningEmulator) {
        let result = self.serve(emulator).await;
        if let Err(err) = result {
            log::info!("[Emulator session] Ended: {err}");
            (self.on_event)(SessionEvent::Failed(err));
        }
    }

    /// Runs until the session is dropped (`Ok`) or fails.
    async fn serve(&mut self, emulator: RunningEmulator) -> Result<(), SessionError> {
        let mut client = EmulatorClient::connect(&emulator)
            .await
            .map_err(|err| SessionError::from_client(&err, false))?;
        // The connection is lazy about auth; a cheap call surfaces a rejected token now rather
        // than on the first frame request.
        client
            .vm_state()
            .await
            .map_err(|err| SessionError::from_client(&err, false))?;
        log::info!(
            "[Emulator session] Connected to emulator-{}",
            emulator.console_port
        );
        (self.on_event)(SessionEvent::Connected);

        let mut wanted: Option<(u32, u32)> = None;
        let mut streaming: Option<(u32, u32)> = None;
        let mut stream: Option<FrameStream> = None;
        let mut start_at: Option<Instant> = None;
        let mut keepalive_at = Instant::now();
        loop {
            tokio::select! {
                command = self.commands.recv() => match command {
                    None => return Ok(()),
                    Some(Command::Stream { width, height }) => {
                        wanted = Some((width, height));
                        if stream.is_none() {
                            start_at.get_or_insert_with(Instant::now);
                        } else if streaming != wanted {
                            start_at = Some(Instant::now() + self.options.resize_debounce);
                        } else {
                            start_at = None;
                        }
                    }
                    Some(Command::Pause) => {
                        if stream.take().is_some() {
                            log::debug!("[Emulator session] Frame stream paused");
                        }
                        wanted = None;
                        streaming = None;
                        start_at = None;
                    }
                },
                () = sleep_until_opt(start_at) => {
                    start_at = None;
                    let Some((width, height)) = wanted else { continue };
                    // Cancel the old call first so the emulator never reads back for two.
                    drop(stream.take());
                    stream = Some(
                        client
                            .stream_screenshot(width, height)
                            .await
                            .map_err(|err| SessionError::from_client(&err, true))?,
                    );
                    streaming = wanted;
                    keepalive_at = Instant::now() + self.options.keepalive_interval;
                    log::debug!("[Emulator session] Frame stream started at {width}x{height}");
                }
                frame = next_frame(&mut stream) => match frame {
                    Ok(Some(frame)) => {
                        self.frames_received.fetch_add(1, Ordering::Relaxed);
                        if self.slot.put(frame) {
                            (self.on_event)(SessionEvent::FrameReady);
                        }
                    }
                    Ok(None) => {
                        stream = None;
                        streaming = None;
                        // A clean end of the stream is how a shutting-down emulator says goodbye;
                        // otherwise ask for frames again shortly.
                        client
                            .vm_state()
                            .await
                            .map_err(|err| SessionError::from_client(&err, true))?;
                        log::debug!("[Emulator session] Frame stream ended; restarting");
                        start_at = Some(Instant::now() + self.options.restart_delay);
                    }
                    Err(err) => return Err(SessionError::from_client(&err, true)),
                },
                () = sleep_until(keepalive_at), if stream.is_some() => {
                    client
                        .keepalive()
                        .await
                        .map_err(|err| SessionError::from_client(&err, true))?;
                    keepalive_at = Instant::now() + self.options.keepalive_interval;
                }
            }
        }
    }
}

async fn sleep_until_opt(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => sleep_until(deadline).await,
        None => future::pending().await,
    }
}

async fn next_frame(stream: &mut Option<FrameStream>) -> Result<Option<Frame>, ClientError> {
    match stream {
        Some(stream) => stream.next_frame().await,
        None => future::pending().await,
    }
}

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;
