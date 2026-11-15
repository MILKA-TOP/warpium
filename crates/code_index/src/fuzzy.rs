//! Name matching for the Go to Class/Symbol palettes.
//!
//! A name matches a query in one of these tiers, best first:
//! exact (ignoring case) > prefix > camel humps from the first hump ("UVM", "UsViMo" →
//! `UserViewModel`) > camel humps from a later hump ("VM" → `UserViewModel`) > any subsequence.

use crate::interner::char_bit;

/// Names longer than this are only matched as a prefix or subsequence.
const MAX_CAMEL_LEN: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MatchTier {
    Fuzzy = 1,
    WordStart = 2,
    Camel = 3,
    Prefix = 4,
    Exact = 5,
}

/// How well a name matched: the tier, then a score within the tier (higher is better).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NameMatch {
    pub tier: MatchTier,
    pub score: i32,
}

/// A query prepared for matching many names.
#[derive(Clone, Debug)]
pub struct Pattern {
    chars: Vec<u8>,
    mask: u64,
}

impl Pattern {
    /// Returns `None` for an empty query.
    pub fn new(query: &str) -> Option<Self> {
        let chars: Vec<u8> = query.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
        if chars.is_empty() {
            return None;
        }
        let mask = chars.iter().fold(0, |mask, byte| mask | char_bit(*byte));
        Some(Self { chars, mask })
    }

    /// Quick rejection: whether a name with character mask `name_mask` could match.
    pub fn may_match(&self, name_mask: u64) -> bool {
        name_mask & self.mask == self.mask
    }

    pub fn match_name(&self, name: &str) -> Option<NameMatch> {
        let name = name.as_bytes();
        let query = self.chars.as_slice();
        if query.len() > name.len() {
            return None;
        }
        let case_bonus = exact_case_prefix_len(query, name) as i32;
        let length_penalty = name.len() as i32;

        if starts_with_ignore_case(name, query) {
            let tier = if name.len() == query.len() {
                MatchTier::Exact
            } else {
                MatchTier::Prefix
            };
            return Some(NameMatch {
                tier,
                score: case_bonus * 4 - length_penalty,
            });
        }
        if name.len() <= MAX_CAMEL_LEN {
            let humps = hump_starts(name);
            if let Some(skipped) = camel_match(query, name, &humps, 0, None) {
                return Some(NameMatch {
                    tier: MatchTier::Camel,
                    score: -(skipped as i32) * 8 + case_bonus - length_penalty,
                });
            }
            for (hump_index, start) in humps.iter().enumerate().skip(1) {
                if let Some(skipped) = camel_match(query, name, &humps, *start, None) {
                    return Some(NameMatch {
                        tier: MatchTier::WordStart,
                        score: -((skipped + hump_index) as i32) * 8 - length_penalty,
                    });
                }
            }
        }
        subsequence_gaps(query, name).map(|gaps| NameMatch {
            tier: MatchTier::Fuzzy,
            score: -(gaps as i32) * 2 - length_penalty,
        })
    }

    /// Byte offsets in `name` of the matched query characters, for highlighting.
    pub fn matched_indices(&self, name: &str) -> Vec<usize> {
        let bytes = name.as_bytes();
        let query = self.chars.as_slice();
        if starts_with_ignore_case(bytes, query) {
            return (0..query.len()).collect();
        }
        if bytes.len() <= MAX_CAMEL_LEN {
            let humps = hump_starts(bytes);
            for start in &humps {
                let mut positions = Vec::with_capacity(query.len());
                if camel_match(query, bytes, &humps, *start, Some(&mut positions)).is_some() {
                    return positions;
                }
            }
        }
        let mut positions = Vec::with_capacity(query.len());
        let mut next = 0;
        for byte in query {
            match bytes[next..]
                .iter()
                .position(|candidate| candidate.eq_ignore_ascii_case(byte))
            {
                Some(offset) => {
                    positions.push(next + offset);
                    next += offset + 1;
                }
                None => return Vec::new(),
            }
        }
        positions
    }
}

fn starts_with_ignore_case(name: &[u8], query: &[u8]) -> bool {
    name.len() >= query.len() && name[..query.len()].eq_ignore_ascii_case(query)
}

fn exact_case_prefix_len(query: &[u8], name: &[u8]) -> usize {
    query
        .iter()
        .zip(name)
        .take_while(|(query, name)| query == name)
        .count()
}

/// Positions where a hump (word) starts: the first character, an uppercase letter, the first
/// letter after `_`/`$`/a digit, and the first digit of a run.
fn hump_starts(name: &[u8]) -> Vec<usize> {
    let mut starts = Vec::new();
    for (i, byte) in name.iter().enumerate() {
        let is_start = match i.checked_sub(1).map(|prev| name[prev]) {
            None => true,
            Some(prev) => {
                byte.is_ascii_uppercase()
                    || (byte.is_ascii_alphabetic() && !prev.is_ascii_alphabetic())
                    || (byte.is_ascii_digit() && !prev.is_ascii_digit())
            }
        };
        if is_start && byte.is_ascii_alphanumeric() {
            starts.push(i);
        }
    }
    starts
}

/// Matches `query` against `name` starting at hump `start`: each query character either
/// continues right after the previous match or starts a later hump. Returns how many humps were
/// skipped, and records match positions in `positions` when given.
fn camel_match(
    query: &[u8],
    name: &[u8],
    humps: &[usize],
    start: usize,
    positions: Option<&mut Vec<usize>>,
) -> Option<usize> {
    if !name[start].eq_ignore_ascii_case(&query[0]) {
        return None;
    }
    // failed[q * len + p]: query[q..] can't match with query[q] at name[p].
    let mut failed = vec![false; query.len() * name.len()];
    let mut path = vec![0; query.len()];
    let skipped = camel_from(query, name, humps, 0, start, &mut failed, &mut path)?;
    if let Some(positions) = positions {
        positions.extend_from_slice(&path);
    }
    Some(skipped)
}

fn camel_from(
    query: &[u8],
    name: &[u8],
    humps: &[usize],
    q: usize,
    p: usize,
    failed: &mut [bool],
    path: &mut [usize],
) -> Option<usize> {
    path[q] = p;
    if q + 1 == query.len() {
        return Some(0);
    }
    let next = query[q + 1];
    // Prefer continuing the current word, then the nearest later hump.
    if p + 1 < name.len()
        && name[p + 1].eq_ignore_ascii_case(&next)
        && !failed[(q + 1) * name.len() + p + 1]
    {
        if let Some(skipped) = camel_from(query, name, humps, q + 1, p + 1, failed, path) {
            return Some(skipped);
        }
        failed[(q + 1) * name.len() + p + 1] = true;
    }
    let first_later = humps.partition_point(|hump| *hump <= p + 1);
    for (skipped, hump) in humps[first_later..].iter().enumerate() {
        if !name[*hump].eq_ignore_ascii_case(&next) || failed[(q + 1) * name.len() + hump] {
            continue;
        }
        if let Some(more) = camel_from(query, name, humps, q + 1, *hump, failed, path) {
            return Some(skipped + more);
        }
        failed[(q + 1) * name.len() + hump] = true;
    }
    None
}

/// Total gap between consecutive matched characters of a greedy subsequence match.
fn subsequence_gaps(query: &[u8], name: &[u8]) -> Option<usize> {
    let mut next = 0;
    let mut gaps = 0;
    for (i, byte) in query.iter().enumerate() {
        let offset = name[next..]
            .iter()
            .position(|candidate| candidate.eq_ignore_ascii_case(byte))?;
        if i > 0 {
            gaps += offset;
        }
        next += offset + 1;
    }
    Some(gaps)
}

#[cfg(test)]
#[path = "fuzzy_tests.rs"]
mod tests;
