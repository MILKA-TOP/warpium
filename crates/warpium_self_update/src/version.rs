//! Warpium release versions: an upstream Warp release tag plus a fork counter, e.g.
//! `v0.2026.06.03.09.49.stable_00.1` (see `script/warpium/version`).

use std::cmp::Ordering;
use std::fmt;

/// A parsed Warpium release tag. Ordered the way the release workflow picks "latest": the
/// upstream part first (its date, then its channel build), then the fork counter, all numeric.
#[derive(Debug, Clone)]
pub struct WarpiumVersion {
    tag: String,
    major: u32,
    /// Year, month, day, hour and minute of the upstream release.
    timestamp: [u32; 5],
    /// The upstream channel name, e.g. `stable`.
    channel: String,
    /// The upstream channel build, the `00` of `stable_00`.
    channel_build: u32,
    fork_counter: u32,
}

impl WarpiumVersion {
    /// Parses a Warpium release tag (`vN.YYYY.MM.DD.HH.MM.channel_NN.counter`). Returns `None`
    /// for anything else, including plain upstream Warp tags without a fork counter.
    pub fn parse(tag: &str) -> Option<Self> {
        let tag = tag.trim();
        let parts: Vec<&str> = tag.strip_prefix('v')?.split('.').collect();
        let [major, year, month, day, hour, minute, channel_part, counter] = parts[..] else {
            return None;
        };

        let major = parse_digits(major, None)?;
        let year = parse_digits(year, Some(4))?;
        let month = parse_digits(month, Some(2))?;
        let day = parse_digits(day, Some(2))?;
        let hour = parse_digits(hour, Some(2))?;
        let minute = parse_digits(minute, Some(2))?;

        let (channel, channel_build) = channel_part.split_once('_')?;
        if channel.is_empty() || !channel.bytes().all(|b| b.is_ascii_lowercase()) {
            return None;
        }
        let channel_build = parse_digits(channel_build, Some(2))?;

        if counter.starts_with('0') {
            return None;
        }
        let fork_counter = parse_digits(counter, None)?;

        Some(Self {
            tag: tag.to_owned(),
            major,
            timestamp: [year, month, day, hour, minute],
            channel: channel.to_owned(),
            channel_build,
            fork_counter,
        })
    }

    /// The release tag, e.g. `v0.2026.06.03.09.49.stable_00.1`.
    pub fn tag(&self) -> &str {
        &self.tag
    }

    /// The bundle's `CFBundleShortVersionString`, as `script/warpium/version --plist` derives it,
    /// e.g. `0.2026.06.03.09.49.00.1`.
    pub fn plist_version(&self) -> String {
        let [year, month, day, hour, minute] = self.timestamp;
        format!(
            "{}.{year:04}.{month:02}.{day:02}.{hour:02}.{minute:02}.{:02}.{}",
            self.major, self.channel_build, self.fork_counter
        )
    }

    fn sort_key(&self) -> (u32, [u32; 5], &str, u32, u32) {
        (
            self.major,
            self.timestamp,
            self.channel.as_str(),
            self.channel_build,
            self.fork_counter,
        )
    }
}

fn parse_digits(value: &str, len: Option<usize>) -> Option<u32> {
    let well_formed = !value.is_empty()
        && value.bytes().all(|b| b.is_ascii_digit())
        && len.is_none_or(|len| value.len() == len);
    well_formed.then(|| value.parse().ok()).flatten()
}

impl PartialEq for WarpiumVersion {
    fn eq(&self, other: &Self) -> bool {
        self.sort_key() == other.sort_key()
    }
}

impl Eq for WarpiumVersion {}

impl PartialOrd for WarpiumVersion {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for WarpiumVersion {
    fn cmp(&self, other: &Self) -> Ordering {
        self.sort_key().cmp(&other.sort_key())
    }
}

impl fmt::Display for WarpiumVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.tag)
    }
}

/// The version the running build reports: a valid runtime override (for testing the updater)
/// wins over the tag embedded at build time. `None` for builds without a release tag, such as
/// local development builds, which never offer updates.
pub fn resolve_current_version(
    runtime_override: Option<&str>,
    embedded: Option<&str>,
) -> Option<WarpiumVersion> {
    runtime_override
        .and_then(WarpiumVersion::parse)
        .or_else(|| embedded.and_then(WarpiumVersion::parse))
}

#[cfg(test)]
#[path = "version_tests.rs"]
mod tests;
