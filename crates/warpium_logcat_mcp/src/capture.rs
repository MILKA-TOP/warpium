//! One device's log capture: a logcat stream on a background thread feeding a bounded
//! [`LogBuffer`], plus the processes ActivityManager announced, so package queries follow an app
//! across restarts.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use instant::Instant;
use warpium_android::AdbClient;
use warpium_android::WorkerHandle;
use warpium_android::logcat::process::{
    PS_COMMAND, ProcessEvent, parse_activity_manager, parse_ps, process_belongs_to,
};
use warpium_android::logcat::{LogBuffer, LogTime, LogcatEvent, LogcatOptions, spawn_logcat};

/// Sizes and timings of a capture.
#[derive(Clone, Debug)]
pub(crate) struct CaptureConfig {
    /// Entries kept per device.
    pub capacity: usize,
    /// Recent entries requested when the stream (re)starts.
    pub backlog: usize,
    /// The backlog counts as loaded once the stream has been quiet this long.
    pub settle: Duration,
    /// The longest a (re)start waits for the backlog.
    pub settle_max: Duration,
}

pub(crate) struct State {
    pub buffer: LogBuffer,
    /// Process name of every pid ActivityManager or `ps` reported, newest wins.
    pub processes: HashMap<i32, String>,
    /// Why the stream ended, while it isn't running.
    pub ended: Option<String>,
    running: bool,
    batches: u64,
    last_batch: Option<Instant>,
    /// After a restart, entries up to this time were already received (logcat replays its
    /// backlog).
    skip_through: Option<LogTime>,
}

impl State {
    /// Every pid `package` (or one of its `package:name` processes) has had.
    pub fn pids_of(&self, package: &str) -> HashSet<i32> {
        self.processes
            .iter()
            .filter(|(_, process)| process_belongs_to(process, package))
            .map(|(pid, _)| *pid)
            .collect()
    }

    fn apply(&mut self, mut batch: Vec<warpium_android::logcat::LogEntry>) {
        if let Some(skip_through) = self.skip_through {
            batch.retain(|entry| entry.time > skip_through);
        }
        for entry in &batch {
            if &*entry.tag == "ActivityManager"
                && let Some(ProcessEvent::Started { pid, process }) =
                    parse_activity_manager(&entry.message)
            {
                self.processes.insert(pid, process);
            }
        }
        self.buffer.push_batch(batch);
        self.batches += 1;
        self.last_batch = Some(Instant::now());
    }
}

struct Shared {
    state: Mutex<State>,
    changed: Condvar,
}

pub(crate) struct Capture {
    serial: String,
    client: AdbClient,
    config: CaptureConfig,
    shared: Arc<Shared>,
    worker: Option<WorkerHandle>,
    /// Seconds the device's clock is ahead of the host's.
    pub clock_offset_secs: i64,
    /// Set by `clear_logs`: queries without `since` start here.
    pub mark: Option<u64>,
}

impl Capture {
    /// Starts streaming from `serial` and waits for the backlog.
    pub fn start(client: AdbClient, serial: String, config: CaptureConfig) -> Self {
        let clock_offset_secs = read_clock_offset(&client, &serial);
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                buffer: LogBuffer::new(config.capacity),
                processes: HashMap::new(),
                ended: None,
                running: false,
                batches: 0,
                last_batch: None,
                skip_through: None,
            }),
            changed: Condvar::new(),
        });
        let mut capture = Self {
            serial,
            client,
            config,
            shared,
            worker: None,
            clock_offset_secs,
            mark: None,
        };
        capture.ensure_running();
        capture
    }

    /// Restarts the stream if it ended (the device rebooted or went away), keeping what was
    /// received. Waits for the backlog after a (re)start.
    pub fn ensure_running(&mut self) {
        let batches_before = {
            let mut state = self.lock();
            if state.running {
                return;
            }
            state.running = true;
            state.ended = None;
            state.skip_through = state.buffer.last_time();
            state.batches
        };
        let shared = self.shared.clone();
        let spawned = spawn_logcat(
            self.client.clone(),
            self.serial.clone(),
            LogcatOptions {
                backlog: self.config.backlog,
            },
            move |event| {
                let mut state = shared.state.lock().unwrap_or_else(|err| err.into_inner());
                match event {
                    LogcatEvent::Entries(batch) => state.apply(batch),
                    LogcatEvent::Ended(reason) => {
                        state.running = false;
                        state.ended =
                            Some(reason.unwrap_or_else(|| "the device closed logcat".to_owned()));
                    }
                }
                drop(state);
                shared.changed.notify_all();
            },
        );
        match spawned {
            Ok(worker) => self.worker = Some(worker),
            Err(err) => {
                let mut state = self.lock();
                state.running = false;
                state.ended = Some(format!("couldn't start a thread: {err}"));
                return;
            }
        }
        self.settle(batches_before);
    }

    /// Waits until the stream has delivered something and gone quiet, or ended.
    fn settle(&self, batches_before: u64) {
        let started = Instant::now();
        let mut state = self.lock();
        loop {
            let quiet = state.batches > batches_before
                && state
                    .last_batch
                    .is_some_and(|last| last.elapsed() >= self.config.settle);
            let elapsed = started.elapsed();
            if quiet || !state.running || elapsed >= self.config.settle_max {
                return;
            }
            let wait = self
                .config
                .settle
                .min(self.config.settle_max.saturating_sub(elapsed));
            state = self.wait(state, wait);
        }
    }

    pub fn serial(&self) -> &str {
        &self.serial
    }

    pub fn lock(&self) -> MutexGuard<'_, State> {
        self.shared
            .state
            .lock()
            .unwrap_or_else(|err| err.into_inner())
    }

    /// Waits up to `timeout` for new entries or the stream's end.
    pub fn wait<'a>(
        &self,
        guard: MutexGuard<'a, State>,
        timeout: Duration,
    ) -> MutexGuard<'a, State> {
        match self.shared.changed.wait_timeout(guard, timeout) {
            Ok((guard, _)) => guard,
            Err(err) => err.into_inner().0,
        }
    }

    /// Asks the device which processes of `package` run now, so a package that started before
    /// the capture (and whose start isn't in the backlog) is known.
    pub fn refresh_processes(&self, package: &str) {
        match self.client.shell(&self.serial, PS_COMMAND) {
            Ok(output) => {
                let processes = parse_ps(&output, package);
                let mut state = self.lock();
                for (pid, process) in processes {
                    state.processes.insert(pid, process);
                }
            }
            Err(err) => log::debug!("[Logcat MCP] Couldn't list processes: {err}"),
        }
    }
}

fn read_clock_offset(client: &AdbClient, serial: &str) -> i64 {
    let host = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or_default();
    client
        .shell(serial, "date +%s")
        .ok()
        .and_then(|output| output.trim().parse::<i64>().ok())
        .map_or(0, |device| device - host)
}
