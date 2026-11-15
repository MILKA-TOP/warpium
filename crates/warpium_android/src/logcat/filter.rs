use std::collections::HashSet;

use regex::Regex;

use super::entry::{LogEntry, LogLevel};

/// Tags to show or hide: a comma-separated list where `-tag` hides and any other term shows.
/// Terms match case-insensitively anywhere in the tag.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TagFilter {
    include: Vec<String>,
    exclude: Vec<String>,
}

impl TagFilter {
    pub fn parse(text: &str) -> Self {
        let mut filter = Self::default();
        for term in text
            .split(',')
            .map(str::trim)
            .filter(|term| !term.is_empty())
        {
            match term.strip_prefix('-') {
                Some(excluded) if !excluded.is_empty() => {
                    filter.exclude.push(excluded.to_ascii_lowercase())
                }
                Some(_) => {}
                None => filter.include.push(term.to_ascii_lowercase()),
            }
        }
        filter
    }

    pub fn is_empty(&self) -> bool {
        self.include.is_empty() && self.exclude.is_empty()
    }

    pub fn matches(&self, tag: &str) -> bool {
        if self
            .exclude
            .iter()
            .any(|term| contains_ignore_ascii_case(tag, term))
        {
            return false;
        }
        self.include.is_empty()
            || self
                .include
                .iter()
                .any(|term| contains_ignore_ascii_case(tag, term))
    }
}

/// Whether `haystack` contains `needle`, which must already be ASCII-lowercase.
fn contains_ignore_ascii_case(haystack: &str, needle: &str) -> bool {
    let (haystack, needle) = (haystack.as_bytes(), needle.as_bytes());
    needle.is_empty()
        || haystack
            .windows(needle.len())
            .any(|window| window.eq_ignore_ascii_case(needle))
}

/// Text to search for in the tag and message: plain text (case-insensitive) or a regex.
#[derive(Clone, Debug, Default)]
pub struct TextFilter {
    regex: Option<Regex>,
}

impl TextFilter {
    /// Builds the filter; an empty `text` matches everything. Fails only for an invalid regex.
    pub fn parse(text: &str, is_regex: bool) -> Result<Self, regex::Error> {
        if text.is_empty() {
            return Ok(Self::default());
        }
        let pattern = if is_regex {
            text.to_owned()
        } else {
            format!("(?i){}", regex::escape(text))
        };
        Ok(Self {
            regex: Some(Regex::new(&pattern)?),
        })
    }

    pub fn is_empty(&self) -> bool {
        self.regex.is_none()
    }

    pub fn matches(&self, entry: &LogEntry) -> bool {
        self.regex
            .as_ref()
            .is_none_or(|regex| regex.is_match(&entry.message) || regex.is_match(&entry.tag))
    }
}

impl PartialEq for TextFilter {
    fn eq(&self, other: &Self) -> bool {
        self.regex.as_ref().map(Regex::as_str) == other.regex.as_ref().map(Regex::as_str)
    }
}

/// The user's filters, besides the package (which the buffer tracks, since its pids change).
#[derive(Clone, Debug, PartialEq)]
pub struct LogFilter {
    pub min_level: LogLevel,
    pub tags: TagFilter,
    pub text: TextFilter,
}

impl Default for LogFilter {
    fn default() -> Self {
        Self {
            min_level: LogLevel::Verbose,
            tags: TagFilter::default(),
            text: TextFilter::default(),
        }
    }
}

impl LogFilter {
    /// Whether `entry` passes; `pids` restricts it to a package's processes when set.
    pub fn matches(&self, entry: &LogEntry, pids: Option<&HashSet<i32>>) -> bool {
        entry.level >= self.min_level
            && pids.is_none_or(|pids| pids.contains(&entry.pid))
            && self.tags.matches(&entry.tag)
            && self.text.matches(entry)
    }
}

#[cfg(test)]
#[path = "filter_tests.rs"]
mod tests;
