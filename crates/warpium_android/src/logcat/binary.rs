//! Incremental parser for `logcat -B` (binary `logger_entry` records), which, unlike the text
//! formats, keeps multi-line messages and odd characters unambiguous and costs no text parsing.
//!
//! Each record is a little-endian header (`len`, `hdr_size`, `pid`, `tid`, `sec`, `nsec`, then
//! `lid` and `uid` in newer versions) followed by `len` payload bytes: the priority, the
//! NUL-terminated tag and the message.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use super::entry::{LogEntry, LogLevel, LogTime};

/// `hdr_size` of version 1 headers, which stored 0 there.
const V1_HEADER_SIZE: usize = 20;
/// The largest header any logger version wrote, with room for growth.
const MAX_HEADER_SIZE: usize = 128;
/// Interned tags kept before the table is reset, so a tag-spamming app can't grow it forever.
const MAX_INTERNED_TAGS: usize = 4096;

#[derive(Debug, PartialEq, Eq)]
pub struct BinaryFormatError {
    pub header_size: usize,
}

impl fmt::Display for BinaryFormatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "logcat sent an unexpected binary header (size {})",
            self.header_size
        )
    }
}

impl std::error::Error for BinaryFormatError {}

#[derive(Default)]
pub struct BinaryParser {
    pending: Vec<u8>,
    tags: HashMap<Box<[u8]>, Arc<str>>,
}

impl BinaryParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Parses the complete records in `pending` bytes plus `data`, appending entries to `out`. An
    /// incomplete trailing record is kept for the next call.
    pub fn push(&mut self, data: &[u8], out: &mut Vec<LogEntry>) -> Result<(), BinaryFormatError> {
        self.pending.extend_from_slice(data);
        let mut offset = 0;
        let result = loop {
            let rest = &self.pending[offset..];
            if rest.len() < 4 {
                break Ok(());
            }
            let payload_len = u16::from_le_bytes([rest[0], rest[1]]) as usize;
            let header_size = match u16::from_le_bytes([rest[2], rest[3]]) as usize {
                0 => V1_HEADER_SIZE,
                size => size,
            };
            if !(V1_HEADER_SIZE..=MAX_HEADER_SIZE).contains(&header_size) {
                break Err(BinaryFormatError { header_size });
            }
            let record_len = header_size + payload_len;
            if rest.len() < record_len {
                break Ok(());
            }
            let header = &rest[..header_size];
            let payload = &rest[header_size..record_len];
            let read_i32 = |at: usize| {
                i32::from_le_bytes([header[at], header[at + 1], header[at + 2], header[at + 3]])
            };
            let pid = read_i32(4);
            let tid = read_i32(8);
            let time = LogTime {
                sec: read_i32(12) as u32 as i64,
                nsec: read_i32(16) as u32,
            };
            if let Some(entry) = decode_payload(payload, pid, tid, time, &mut self.tags) {
                out.push(entry);
            }
            offset += record_len;
        };
        self.pending.drain(..offset);
        result
    }
}

fn decode_payload(
    payload: &[u8],
    pid: i32,
    tid: i32,
    time: LogTime,
    tags: &mut HashMap<Box<[u8]>, Arc<str>>,
) -> Option<LogEntry> {
    let (&priority, rest) = payload.split_first()?;
    // Binary event-log records (which have no priority byte) are skipped.
    if priority > 8 {
        return None;
    }
    let tag_end = rest.iter().position(|&b| b == 0)?;
    let tag_bytes = &rest[..tag_end];
    let message_bytes = &rest[tag_end + 1..];
    let message_end = message_bytes
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(message_bytes.len());
    let message = String::from_utf8_lossy(&message_bytes[..message_end]);
    let message = message.trim_end_matches(['\n', '\r']);
    if tags.len() >= MAX_INTERNED_TAGS && !tags.contains_key(tag_bytes) {
        tags.clear();
    }
    let tag = tags
        .entry(tag_bytes.into())
        .or_insert_with(|| String::from_utf8_lossy(tag_bytes).into())
        .clone();
    Some(LogEntry {
        time,
        pid,
        tid,
        level: LogLevel::from_priority(priority),
        tag,
        message: message.into(),
    })
}

/// Encodes an entry as a version 4 record, the way logd does. For tests and fixtures.
pub fn encode_entry(entry: &LogEntry, priority: u8) -> Vec<u8> {
    let mut payload = vec![priority];
    payload.extend_from_slice(entry.tag.as_bytes());
    payload.push(0);
    payload.extend_from_slice(entry.message.as_bytes());
    payload.push(0);
    let mut record = Vec::with_capacity(28 + payload.len());
    record.extend_from_slice(&(payload.len() as u16).to_le_bytes());
    record.extend_from_slice(&28u16.to_le_bytes());
    record.extend_from_slice(&entry.pid.to_le_bytes());
    record.extend_from_slice(&entry.tid.to_le_bytes());
    record.extend_from_slice(&(entry.time.sec as u32).to_le_bytes());
    record.extend_from_slice(&entry.time.nsec.to_le_bytes());
    record.extend_from_slice(&0u32.to_le_bytes());
    record.extend_from_slice(&10123u32.to_le_bytes());
    record.extend_from_slice(&payload);
    record
}

#[cfg(test)]
#[path = "binary_tests.rs"]
mod tests;
