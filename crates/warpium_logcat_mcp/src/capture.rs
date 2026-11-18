//! One device's log capture: a logcat stream on a background thread feeding a bounded
//! [`LogBuffer`], plus the processes ActivityManager announced, so package queries follow an app
//! across restarts.

use std::collections::HashMap;
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

/// A process that had a pid over a stretch of the stream (sequence numbers `from..to`).
#[derive(Clone, Debug)]
struct PidOwner {
    process: String,
    from: u64,
    /// `None` while the process is still alive as far as the log tells.
    to: Option<u64>,
}

/// The stretches of the stream in which pids belonged to one package: what package-scoped
/// queries show. Pids are reused, so a pid alone doesn't name a process.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct PidRanges(Vec<(i32, u64, Option<u64>)>);

impl PidRanges {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Whether the entry with sequence number `seq` came from the package.
    pub fn contains(&self, seq: u64, pid: i32) -> bool {
        self.0.iter().any(|(owner_pid, from, to)| {
            *owner_pid == pid && seq >= *from && to.is_none_or(|to| seq < to)
        })
    }
}

/// How long before ActivityManager announces a process its lines are taken to be its own.
const START_SLACK_SECS: i64 = 5;

pub(crate) struct State {
    pub buffer: LogBuffer,
    /// Process name of every pid ActivityManager or `ps` reported, newest wins.
    pub processes: HashMap<i32, String>,
    /// Which process had each pid when, from ActivityManager's start and death lines and `ps`.
    owners: HashMap<i32, Vec<PidOwner>>,
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
    /// The stretches of the stream in which `package` (or one of its `package:name` processes)
    /// had each of its pids.
    pub fn pids_of(&self, package: &str) -> PidRanges {
        let mut ranges: Vec<(i32, u64, Option<u64>)> = self
            .owners
            .iter()
            .flat_map(|(pid, owners)| {
                owners
                    .iter()
                    .filter(|owner| process_belongs_to(&owner.process, package))
                    .map(|owner| (*pid, owner.from, owner.to))
            })
            .collect();
        ranges.sort_unstable();
        PidRanges(ranges)
    }

    /// `process` has `pid` from the entry with sequence number `seq` on, announced at `time`
    /// (`None` when `ps` found it running). A process logs a few lines before ActivityManager
    /// announces it, so its stretch starts up to [`START_SLACK_SECS`] earlier, but never before
    /// the pid's previous owner's ended.
    fn record_start(&mut self, pid: i32, process: String, seq: u64, time: Option<LogTime>) {
        self.processes.insert(pid, process.clone());
        let owners = self.owners.entry(pid).or_default();
        let previous_end = match owners.last_mut() {
            Some(last) if last.to.is_none() && last.process == process => return,
            Some(last) => *last.to.get_or_insert(seq),
            None => 0,
        };
        let from = match time {
            Some(time) => {
                let earliest = time.sec - START_SLACK_SECS;
                self.buffer
                    .entries_from(previous_end)
                    .take_while(|(entry_seq, _)| *entry_seq < seq)
                    .find(|(_, entry)| entry.pid == pid && entry.time.sec >= earliest)
                    .map_or(seq, |(entry_seq, _)| entry_seq)
            }
            None => previous_end,
        };
        self.owners.entry(pid).or_default().push(PidOwner {
            process,
            from,
            to: None,
        });
    }

    /// `process` died at the entry with sequence number `seq`: later lines of `pid` belong to
    /// whichever process gets it next.
    fn record_death(&mut self, pid: i32, process: &str, seq: u64) {
        if let Some(last) = self
            .owners
            .get_mut(&pid)
            .and_then(|owners| owners.last_mut())
            && last.to.is_none()
            && last.process == process
        {
            last.to = Some(seq + 1);
        }
    }

    fn apply(&mut self, mut batch: Vec<warpium_android::logcat::LogEntry>) {
        if let Some(skip_through) = self.skip_through {
            batch.retain(|entry| entry.time > skip_through);
        }
        let first_seq = self.buffer.next_seq();
        let events: Vec<(u64, LogTime, ProcessEvent)> = batch
            .iter()
            .enumerate()
            .filter(|(_, entry)| &*entry.tag == "ActivityManager")
            .filter_map(|(offset, entry)| {
                let event = parse_activity_manager(&entry.message)?;
                Some((first_seq + offset as u64, entry.time, event))
            })
            .collect();
        // Pushed first, so a start can look back at the lines its process logged before it.
        self.buffer.push_batch(batch);
        for (seq, time, event) in events {
            match event {
                ProcessEvent::Started { pid, process } => {
                    self.record_start(pid, process, seq, Some(time))
                }
                ProcessEvent::Died { pid, process } => self.record_death(pid, &process, seq),
            }
        }
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
                owners: HashMap::new(),
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
                let seq = state.buffer.next_seq();
                for (pid, process) in processes {
                    state.record_start(pid, process, seq, None);
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
