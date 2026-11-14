//! SHA-256 checksums of downloaded release assets.

use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ChecksumError {
    #[error("the checksum file is not a SHA-256 checksum for {expected_name}")]
    Malformed { expected_name: String },
    #[error("the checksum file is for {found}, not {expected_name}")]
    WrongFile {
        expected_name: String,
        found: String,
    },
    #[error("the download is corrupt: its SHA-256 doesn't match the release's checksum")]
    Mismatch,
}

/// Reads the expected digest of `expected_name` from a `shasum -a 256` / `sha256sum` file
/// (`<hex>  <name>`, or `<hex> *<name>` in binary mode). A bare digest is accepted too.
pub fn parse_checksum_file(contents: &str, expected_name: &str) -> Result<[u8; 32], ChecksumError> {
    let malformed = || ChecksumError::Malformed {
        expected_name: expected_name.to_owned(),
    };
    let mut lines = contents
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty());
    let line = lines.next().ok_or_else(malformed)?;
    if lines.next().is_some() {
        return Err(malformed());
    }

    let (digest, name) = match line.split_once(char::is_whitespace) {
        Some((digest, name)) => (digest, Some(name.trim_start())),
        None => (line, None),
    };
    let mut bytes = [0u8; 32];
    hex::decode_to_slice(digest, &mut bytes).map_err(|_| malformed())?;
    if let Some(name) = name {
        let name = name.strip_prefix('*').unwrap_or(name);
        if name != expected_name {
            return Err(ChecksumError::WrongFile {
                expected_name: expected_name.to_owned(),
                found: name.to_owned(),
            });
        }
    }
    Ok(bytes)
}

/// An incremental SHA-256 of a download.
#[derive(Default)]
pub struct Sha256Hasher(Sha256);

impl Sha256Hasher {
    pub fn update(&mut self, chunk: &[u8]) {
        self.0.update(chunk);
    }

    /// Fails with [`ChecksumError::Mismatch`] unless the data hashed so far has `expected` as its
    /// digest.
    pub fn verify(self, expected: &[u8; 32]) -> Result<(), ChecksumError> {
        let digest: [u8; 32] = self.0.finalize().into();
        if &digest == expected {
            Ok(())
        } else {
            Err(ChecksumError::Mismatch)
        }
    }
}

#[cfg(test)]
#[path = "checksum_tests.rs"]
mod tests;
