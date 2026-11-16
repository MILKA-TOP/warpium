//! Logcat: streaming from a device, parsing, filtering and a bounded buffer for display.

pub mod binary;
pub mod buffer;
pub mod entry;
pub mod filter;
pub mod process;
pub mod stream;

pub use buffer::{LogBuffer, PushOutcome, Row, TAB_WIDTH};
pub use entry::{LogEntry, LogLevel, LogTime};
pub use filter::{LogFilter, TagFilter, TextFilter};
pub use stream::{LogcatEvent, LogcatOptions, spawn_logcat, spawn_process_poller};
