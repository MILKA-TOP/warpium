use std::sync::Arc;

use chrono::{DateTime, Local, TimeZone};

/// A log priority, from `android/log.h`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LogLevel {
    Verbose,
    Debug,
    Info,
    Warn,
    Error,
    /// `F` (fatal); also `A` (assert) in older tools.
    Fatal,
}

impl LogLevel {
    pub const ALL: [LogLevel; 6] = [
        LogLevel::Verbose,
        LogLevel::Debug,
        LogLevel::Info,
        LogLevel::Warn,
        LogLevel::Error,
        LogLevel::Fatal,
    ];

    /// From the priority byte of a binary entry. Unknown and default priorities count as
    /// verbose, and anything above fatal (`SILENT`) as fatal.
    pub fn from_priority(priority: u8) -> Self {
        match priority {
            0..=2 => LogLevel::Verbose,
            3 => LogLevel::Debug,
            4 => LogLevel::Info,
            5 => LogLevel::Warn,
            6 => LogLevel::Error,
            _ => LogLevel::Fatal,
        }
    }

    pub fn letter(self) -> char {
        match self {
            LogLevel::Verbose => 'V',
            LogLevel::Debug => 'D',
            LogLevel::Info => 'I',
            LogLevel::Warn => 'W',
            LogLevel::Error => 'E',
            LogLevel::Fatal => 'F',
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            LogLevel::Verbose => "Verbose",
            LogLevel::Debug => "Debug",
            LogLevel::Info => "Info",
            LogLevel::Warn => "Warn",
            LogLevel::Error => "Error",
            LogLevel::Fatal => "Fatal",
        }
    }
}

/// Device wall-clock time of an entry.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct LogTime {
    pub sec: i64,
    pub nsec: u32,
}

impl LogTime {
    /// `MM-DD HH:MM:SS.mmm` in `tz`, as `logcat -v threadtime` shows it.
    pub fn format_in<Tz: TimeZone>(self, tz: &Tz) -> String
    where
        Tz::Offset: std::fmt::Display,
    {
        match DateTime::from_timestamp(self.sec, self.nsec) {
            Some(time) => time
                .with_timezone(tz)
                .format("%m-%d %H:%M:%S%.3f")
                .to_string(),
            None => String::new(),
        }
    }

    /// [`Self::format_in`] the local time zone.
    pub fn format_local(self) -> String {
        self.format_in(&Local)
    }
}

/// One log message. Multi-line messages (stack traces) stay one entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogEntry {
    pub time: LogTime,
    pub pid: i32,
    pub tid: i32,
    pub level: LogLevel,
    /// Interned: tags repeat across most entries.
    pub tag: Arc<str>,
    pub message: Box<str>,
}
