//! A bounded ring of log entries with an incrementally maintained list of the rows that pass the
//! current filter, so a view can render any visible row in O(1) under a fast stream.

use std::collections::{HashSet, VecDeque};
use std::ops::Range;

use super::entry::{LogEntry, LogTime};
use super::filter::LogFilter;
use super::process::{ProcessEvent, parse_activity_manager, process_belongs_to};

/// How many columns a tab advances; the view expands tabs to this many spaces.
pub const TAB_WIDTH: usize = 4;

/// One display line: a line of an entry's message, or a part of one when soft wrap is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RowRef {
    seq: u64,
    /// Byte range of the line (or its wrapped part) in the message.
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
    /// Message lines wider than this many columns are split into several rows.
    wrap_columns: Option<usize>,
    /// The widest row added since the rows were last rebuilt, in columns. Evictions don't lower
    /// it, so it can overestimate until the next rebuild.
    max_row_columns: usize,
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
            wrap_columns: None,
            max_row_columns: 0,
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

    /// The width of the widest row, in columns (a tab counts as [`TAB_WIDTH`]), kept up to date
    /// as rows are added rather than measured over the buffer.
    pub fn max_row_columns(&self) -> usize {
        self.max_row_columns
    }

    /// Splits message lines wider than `columns` into several rows (soft wrap), or stops
    /// splitting with `None`. Returns whether the rows were rebuilt.
    pub fn set_wrap_columns(&mut self, columns: Option<usize>) -> bool {
        let columns = columns.map(|columns| columns.max(1));
        if columns == self.wrap_columns {
            return false;
        }
        self.wrap_columns = columns;
        self.refilter();
        true
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
                outcome.added_rows += push_rows(
                    &mut self.rows,
                    seq,
                    &entry,
                    self.wrap_columns,
                    &mut self.max_row_columns,
                );
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
        self.max_row_columns = 0;
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
        let mut max_row_columns = 0;
        for (index, entry) in self.entries.iter().enumerate() {
            if self.matches(entry) {
                push_rows(
                    &mut rows,
                    self.first_seq + index as u64,
                    entry,
                    self.wrap_columns,
                    &mut max_row_columns,
                );
            }
        }
        self.rows = rows;
        self.max_row_columns = max_row_columns;
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

/// Appends one row per message line, or per `wrap_columns`-wide part of a line, and raises
/// `max_row_columns` to the widest; returns how many rows.
fn push_rows(
    rows: &mut VecDeque<RowRef>,
    seq: u64,
    entry: &LogEntry,
    wrap_columns: Option<usize>,
    max_row_columns: &mut usize,
) -> usize {
    let mut start = 0;
    let mut count = 0;
    for line in entry.message.split('\n') {
        let end = start + line.len();
        let trimmed = line.strip_suffix('\r').unwrap_or(line);
        let mut part_start = 0;
        let mut columns = 0;
        for (offset, ch) in trimmed.char_indices() {
            let width = if ch == '\t' { TAB_WIDTH } else { 1 };
            if wrap_columns.is_some_and(|wrap| columns > 0 && columns + width > wrap) {
                rows.push_back(row_ref(seq, start + part_start, start + offset));
                *max_row_columns = (*max_row_columns).max(columns);
                count += 1;
                part_start = offset;
                columns = 0;
            }
            columns += width;
        }
        rows.push_back(row_ref(seq, start + part_start, start + trimmed.len()));
        *max_row_columns = (*max_row_columns).max(columns);
        count += 1;
        start = end + 1;
    }
    count
}

fn row_ref(seq: u64, start: usize, end: usize) -> RowRef {
    RowRef {
        seq,
        start: start as u32,
        end: end as u32,
    }
}

#[cfg(test)]
#[path = "buffer_tests.rs"]
mod tests;
