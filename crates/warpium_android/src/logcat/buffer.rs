//! A bounded ring of log entries with an incrementally maintained list of the rows that pass the
//! current filter, so a view can render any visible row in O(1) under a fast stream.

use std::collections::{HashSet, VecDeque};
use std::ops::Range;

use super::entry::{LogEntry, LogTime};
use super::filter::LogFilter;
use super::process::{ProcessEvent, parse_activity_manager, process_belongs_to};

/// One display line: a line of an entry's message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RowRef {
    seq: u64,
    /// Byte range of the line in the message.
    start: u32,
    end: u32,
}

/// A row to render.
#[derive(Clone, Copy, Debug)]
pub struct Row<'a> {
    pub entry: &'a LogEntry,
    pub line: &'a str,
    /// Whether this is the first line of its entry (which shows the time, pid and tag).
    pub is_first_line: bool,
    /// The entry's sequence number, stable while it stays in the buffer.
    pub seq: u64,
}

/// What a [`LogBuffer::push_batch`] changed, so a scrolled-up view can keep its place.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PushOutcome {
    pub added_rows: usize,
    /// Filtered rows dropped from the front because the oldest entries were evicted.
    pub evicted_rows: usize,
    /// The rows were rebuilt (a new process of the tracked package revealed earlier entries), so
    /// row indices from before the push are stale.
    pub rebuilt: bool,
}

/// Follows one app package's processes.
#[derive(Clone, Debug, PartialEq, Eq)]
struct PackageTracker {
    package: String,
    /// Every pid the package has had since tracking started, so logs from before a crash and
    /// restart stay visible.
    pids: HashSet<i32>,
    /// The main process's pid while it runs.
    running_pid: Option<i32>,
}

pub struct LogBuffer {
    entries: VecDeque<LogEntry>,
    /// Sequence number of `entries[0]`.
    first_seq: u64,
    capacity: usize,
    filter: LogFilter,
    rows: VecDeque<RowRef>,
    package: Option<PackageTracker>,
}

impl LogBuffer {
    /// A buffer keeping at most `capacity` entries.
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            first_seq: 0,
            capacity: capacity.max(1),
            filter: LogFilter::default(),
            rows: VecDeque::new(),
            package: None,
        }
    }

    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }

    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    pub fn filter(&self) -> &LogFilter {
        &self.filter
    }

    pub fn package(&self) -> Option<&str> {
        self.package
            .as_ref()
            .map(|tracker| tracker.package.as_str())
    }

    /// The pid of the tracked package's main process, while it runs.
    pub fn running_pid(&self) -> Option<i32> {
        self.package
            .as_ref()
            .and_then(|tracker| tracker.running_pid)
    }

    /// Whether the tracked package has had any process since tracking started.
    pub fn package_seen(&self) -> bool {
        self.package
            .as_ref()
            .is_some_and(|tracker| !tracker.pids.is_empty())
    }

    /// Appends entries, evicting the oldest beyond capacity. ActivityManager's process starts are
    /// applied before the entries after them are filtered, so an app's first lines after a restart
    /// are shown.
    pub fn push_batch(&mut self, batch: Vec<LogEntry>) -> PushOutcome {
        let mut outcome = PushOutcome::default();
        let mut new_pid = false;
        for entry in batch {
            new_pid |= self.track_process(&entry);
            let seq = self.first_seq + self.entries.len() as u64;
            if self.matches(&entry) {
                outcome.added_rows += push_rows(&mut self.rows, seq, &entry);
            }
            self.entries.push_back(entry);
        }
        let overflow = self.entries.len().saturating_sub(self.capacity);
        if overflow > 0 {
            self.entries.drain(..overflow);
            self.first_seq += overflow as u64;
            let before = self.rows.len();
            while self
                .rows
                .front()
                .is_some_and(|row| row.seq < self.first_seq)
            {
                self.rows.pop_front();
            }
            outcome.evicted_rows = before - self.rows.len();
        }
        // A forked app process logs a few lines before system_server announces it.
        if new_pid {
            self.refilter();
            outcome.rebuilt = true;
        }
        outcome
    }

    /// Drops the entries; the tracked package's known pids stay.
    pub fn clear(&mut self) {
        self.first_seq += self.entries.len() as u64;
        self.entries.clear();
        self.rows.clear();
    }

    /// Drops the entries and the tracked package's known pids, for a different device.
    pub fn reset(&mut self) {
        self.clear();
        if let Some(tracker) = &mut self.package {
            tracker.pids.clear();
            tracker.running_pid = None;
        }
    }

    /// The time of the newest entry.
    pub fn last_time(&self) -> Option<LogTime> {
        self.entries.back().map(|entry| entry.time)
    }

    pub fn set_filter(&mut self, filter: LogFilter) {
        if filter != self.filter {
            self.filter = filter;
            self.refilter();
        }
    }

    /// Restricts the rows to `package`'s processes (or lifts the restriction). Known pids are
    /// forgotten when the package changes.
    pub fn set_package(&mut self, package: Option<String>) {
        if self.package() == package.as_deref() {
            return;
        }
        self.package = package.map(|package| PackageTracker {
            package,
            pids: HashSet::new(),
            running_pid: None,
        });
        self.refilter();
    }

    /// Adds pids of the tracked package found by polling the device (`ps`), and records which one
    /// runs now. Returns whether anything changed.
    pub fn update_package_processes(&mut self, processes: &[(i32, String)]) -> bool {
        let Some(tracker) = &mut self.package else {
            return false;
        };
        let running = processes
            .iter()
            .find(|(_, process)| *process == tracker.package)
            .map(|(pid, _)| *pid);
        let mut added = false;
        for (pid, process) in processes {
            if process_belongs_to(process, &tracker.package) {
                added |= tracker.pids.insert(*pid);
            }
        }
        let running_changed = tracker.running_pid != running;
        tracker.running_pid = running;
        if added {
            self.refilter();
        }
        added || running_changed
    }

    /// The rows in `range` (clamped to the row count).
    pub fn rows(&self, range: Range<usize>) -> impl Iterator<Item = Row<'_>> {
        let end = range.end.min(self.rows.len());
        let start = range.start.min(end);
        self.rows
            .range(start..end)
            .filter_map(|row| self.row_for(row))
    }

    pub fn row(&self, index: usize) -> Option<Row<'_>> {
        self.rows.get(index).and_then(|row| self.row_for(row))
    }

    /// The row index of the first row of the entry with `seq`, if it is shown.
    pub fn row_index_of(&self, seq: u64) -> Option<usize> {
        let index = self.rows.partition_point(|row| row.seq < seq);
        (self.rows.get(index)?.seq == seq).then_some(index)
    }

    fn row_for(&self, row: &RowRef) -> Option<Row<'_>> {
        let entry = self.entries.get((row.seq - self.first_seq) as usize)?;
        Some(Row {
            entry,
            line: entry.message.get(row.start as usize..row.end as usize)?,
            is_first_line: row.start == 0,
            seq: row.seq,
        })
    }

    fn matches(&self, entry: &LogEntry) -> bool {
        self.filter
            .matches(entry, self.package.as_ref().map(|tracker| &tracker.pids))
    }

    fn refilter(&mut self) {
        let mut rows = VecDeque::with_capacity(self.rows.len());
        for (index, entry) in self.entries.iter().enumerate() {
            if self.matches(entry) {
                push_rows(&mut rows, self.first_seq + index as u64, entry);
            }
        }
        self.rows = rows;
    }

    /// Applies an ActivityManager process start or death; returns whether a new pid of the
    /// tracked package appeared.
    fn track_process(&mut self, entry: &LogEntry) -> bool {
        let Some(tracker) = &mut self.package else {
            return false;
        };
        if &*entry.tag != "ActivityManager" {
            return false;
        }
        match parse_activity_manager(&entry.message) {
            Some(ProcessEvent::Started { pid, process })
                if process_belongs_to(&process, &tracker.package) =>
            {
                if process == tracker.package {
                    tracker.running_pid = Some(pid);
                }
                tracker.pids.insert(pid)
            }
            Some(ProcessEvent::Died { pid, .. }) => {
                if tracker.running_pid == Some(pid) {
                    tracker.running_pid = None;
                }
                false
            }
            Some(ProcessEvent::Started { .. }) | None => false,
        }
    }
}

/// Appends one row per message line; returns how many.
fn push_rows(rows: &mut VecDeque<RowRef>, seq: u64, entry: &LogEntry) -> usize {
    let mut start = 0;
    let mut count = 0;
    for line in entry.message.split('\n') {
        let end = start + line.len();
        let trimmed_end = if line.ends_with('\r') { end - 1 } else { end };
        rows.push_back(RowRef {
            seq,
            start: start as u32,
            end: trimmed_end as u32,
        });
        count += 1;
        start = end + 1;
    }
    count
}

#[cfg(test)]
#[path = "buffer_tests.rs"]
mod tests;
