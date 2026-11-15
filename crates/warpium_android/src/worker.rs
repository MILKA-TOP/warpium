//! Background threads that block on adb sockets and stop when their handle is dropped.

use std::io;
use std::net::{Shutdown, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

/// Shared between a worker thread and its [`WorkerHandle`].
#[derive(Clone, Default)]
pub(crate) struct WorkerControl {
    stop: Arc<AtomicBool>,
    /// The socket the thread is blocked on, so stopping can unblock it.
    socket: Arc<Mutex<Option<TcpStream>>>,
}

impl WorkerControl {
    pub fn is_stopped(&self) -> bool {
        self.stop.load(Ordering::Acquire)
    }

    /// Registers the socket the thread is about to block on. Returns `false` (and closes it) if
    /// the worker was stopped meanwhile.
    pub fn set_socket(&self, socket: &TcpStream) -> bool {
        let clone = socket.try_clone().ok();
        let mut slot = self.socket.lock().unwrap_or_else(|err| err.into_inner());
        if self.is_stopped() {
            let _ = socket.shutdown(Shutdown::Both);
            return false;
        }
        *slot = clone;
        true
    }

    /// Sleeps for `duration` in short steps; returns `false` if the worker was stopped.
    pub fn sleep(&self, duration: Duration) -> bool {
        const STEP: Duration = Duration::from_millis(100);
        let mut left = duration;
        while !left.is_zero() {
            if self.is_stopped() {
                return false;
            }
            let step = left.min(STEP);
            std::thread::sleep(step);
            left -= step;
        }
        !self.is_stopped()
    }

    fn stop(&self) {
        self.stop.store(true, Ordering::Release);
        let slot = self.socket.lock().unwrap_or_else(|err| err.into_inner());
        if let Some(socket) = slot.as_ref() {
            let _ = socket.shutdown(Shutdown::Both);
        }
    }
}

/// Owns a worker thread. Dropping it stops the thread without waiting for it, so it is safe to
/// drop on the UI thread.
pub struct WorkerHandle {
    control: WorkerControl,
    thread: Option<JoinHandle<()>>,
}

impl WorkerHandle {
    pub(crate) fn spawn(
        name: &str,
        body: impl FnOnce(WorkerControl) + Send + 'static,
    ) -> io::Result<Self> {
        let control = WorkerControl::default();
        let thread_control = control.clone();
        let thread = std::thread::Builder::new()
            .name(name.to_owned())
            .spawn(move || body(thread_control))?;
        Ok(Self {
            control,
            thread: Some(thread),
        })
    }

    pub fn is_finished(&self) -> bool {
        self.thread
            .as_ref()
            .is_none_or(|thread| thread.is_finished())
    }

    /// Stops the thread and waits for it to exit. For tests and shutdown paths off the UI thread.
    pub fn stop_and_join(mut self) {
        self.control.stop();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for WorkerHandle {
    fn drop(&mut self) {
        self.control.stop();
    }
}
