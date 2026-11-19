//! Object ids, stored inline so commits and graph lanes stay small.

use std::fmt;

/// A SHA-1 or SHA-256 object id.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Oid {
    bytes: [u8; 32],
    len: u8,
}

/// Hex digits shown for a commit in lists, as JetBrains IDEs show them.
pub const SHORT_HEX_LEN: usize = 8;

impl Oid {
    /// Parses a full 40 (SHA-1) or 64 (SHA-256) digit hex id.
    pub fn from_hex(hex: &str) -> Option<Self> {
        let hex = hex.as_bytes();
        if hex.len() != 40 && hex.len() != 64 {
            return None;
        }
        let mut bytes = [0; 32];
        for (index, pair) in hex.chunks_exact(2).enumerate() {
            bytes[index] = (hex_value(pair[0])? << 4) | hex_value(pair[1])?;
        }
        Some(Self {
            bytes,
            len: (hex.len() / 2) as u8,
        })
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len as usize]
    }

    pub fn to_hex(&self) -> String {
        let mut hex = String::with_capacity(self.len as usize * 2);
        for byte in self.as_bytes() {
            hex.push(HEX[(byte >> 4) as usize] as char);
            hex.push(HEX[(byte & 0xf) as usize] as char);
        }
        hex
    }

    pub fn short(&self) -> String {
        let mut hex = self.to_hex();
        hex.truncate(SHORT_HEX_LEN);
        hex
    }

    /// The all-zero id git uses for lines and changes that aren't committed yet.
    pub fn is_zero(&self) -> bool {
        self.as_bytes().iter().all(|byte| *byte == 0)
    }

    /// Whether the hex form starts with `prefix` (case-insensitive).
    pub fn starts_with_hex(&self, prefix: &str) -> bool {
        let prefix = prefix.to_ascii_lowercase();
        !prefix.is_empty() && self.to_hex().starts_with(&prefix)
    }
}

const HEX: &[u8; 16] = b"0123456789abcdef";

fn hex_value(digit: u8) -> Option<u8> {
    match digit {
        b'0'..=b'9' => Some(digit - b'0'),
        b'a'..=b'f' => Some(digit - b'a' + 10),
        b'A'..=b'F' => Some(digit - b'A' + 10),
        _ => None,
    }
}

/// Whether `text` looks like an abbreviated or full commit id (4 to 64 hex digits).
pub fn looks_like_hex_id(text: &str) -> bool {
    (4..=64).contains(&text.len()) && text.bytes().all(|byte| byte.is_ascii_hexdigit())
}

impl fmt::Display for Oid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl fmt::Debug for Oid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Oid({})", self.short())
    }
}

#[cfg(test)]
#[path = "oid_tests.rs"]
mod tests;
