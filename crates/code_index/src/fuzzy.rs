//! Name matching for the Go to Class/Symbol palettes.
//!
//! A name matches a query in one of these tiers, best first:
//! exact (ignoring case) > prefix > camel humps from the first hump ("UVM", "UsViMo" →
//! `UserViewModel`) > camel humps from a later hump ("VM" → `UserViewModel`) > any subsequence.

use crate::interner::char_bit;

/// Names longer than this, or queries longer than `MAX_CAMEL_QUERY`, are only matched as a
/// prefix or subsequence.
const MAX_CAMEL_LEN: usize = 128;
const MAX_CAMEL_QUERY: usize = 32;

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
        // Every tier implies a subsequence match, so this cheap check rejects most names.
        let gaps = subsequence_gaps(query, name)?;
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
        if let Some(mut camel) = Camel::new(query, name) {
            if let Some(skipped) = camel.match_from(0) {
                return Some(NameMatch {
                    tier: MatchTier::Camel,
                    score: -(skipped as i32) * 8 + case_bonus - length_penalty,
                });
            }
            for hump_index in 1..camel.hump_count {
                if let Some(skipped) = camel.match_from(hump_index) {
                    return Some(NameMatch {
                        tier: MatchTier::WordStart,
                        score: -((skipped + hump_index) as i32) * 8 - length_penalty,
                    });
                }
            }
        }
        Some(NameMatch {
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
        if let Some(mut camel) = Camel::new(query, bytes) {
            for hump_index in 0..camel.hump_count {
                if camel.match_from(hump_index).is_some() {
                    return camel.path[..query.len()]
                        .iter()
                        .map(|position| usize::from(*position))
                        .collect();
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

/// Camel-hump matching of one query against one name, on the stack: this runs for every
/// candidate name on every keystroke.
struct Camel<'a> {
    query: &'a [u8],
    name: &'a [u8],
    /// Positions where a hump (word) starts: the first character, an uppercase letter, the first
    /// letter after `_`/`$`/a digit, and the first digit of a run.
    humps: [u8; MAX_CAMEL_LEN],
    hump_count: usize,
    /// Bit `q * MAX_CAMEL_LEN + p`: `query[q..]` can't match with `query[q]` at `name[p]`. This
    /// doesn't depend on where the match started, so it's shared by all start humps.
    failed: [u64; MAX_CAMEL_QUERY * MAX_CAMEL_LEN / 64],
    path: [u8; MAX_CAMEL_QUERY],
}

impl<'a> Camel<'a> {
    fn new(query: &'a [u8], name: &'a [u8]) -> Option<Self> {
        if name.len() > MAX_CAMEL_LEN || query.len() > MAX_CAMEL_QUERY {
            return None;
        }
        let mut humps = [0; MAX_CAMEL_LEN];
        let mut hump_count = 0;
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
                humps[hump_count] = i as u8;
                hump_count += 1;
            }
        }
        Some(Self {
            query,
            name,
            humps,
            hump_count,
            failed: [0; MAX_CAMEL_QUERY * MAX_CAMEL_LEN / 64],
            path: [0; MAX_CAMEL_QUERY],
        })
    }

    /// Matches the query starting at hump `hump_index`: each query character either continues
    /// right after the previous match or starts a later hump. Returns how many humps were
    /// skipped; the matched positions are left in `path`.
    fn match_from(&mut self, hump_index: usize) -> Option<usize> {
        let start = usize::from(self.humps[hump_index]);
        if !self.name[start].eq_ignore_ascii_case(&self.query[0]) {
            return None;
        }
        self.match_at(0, start)
    }

    fn match_at(&mut self, q: usize, p: usize) -> Option<usize> {
        self.path[q] = p as u8;
        if q + 1 == self.query.len() {
            return Some(0);
        }
        let next = self.query[q + 1];
        // Prefer continuing the current word, then the nearest later hump.
        if p + 1 < self.name.len()
            && self.name[p + 1].eq_ignore_ascii_case(&next)
            && !self.has_failed(q + 1, p + 1)
        {
            if let Some(skipped) = self.match_at(q + 1, p + 1) {
                return Some(skipped);
            }
            self.mark_failed(q + 1, p + 1);
        }
        let first_later =
            self.humps[..self.hump_count].partition_point(|hump| usize::from(*hump) <= p + 1);
        for (skipped, hump_index) in (first_later..self.hump_count).enumerate() {
            let hump = usize::from(self.humps[hump_index]);
            if !self.name[hump].eq_ignore_ascii_case(&next) || self.has_failed(q + 1, hump) {
                continue;
            }
            if let Some(more) = self.match_at(q + 1, hump) {
                return Some(skipped + more);
            }
            self.mark_failed(q + 1, hump);
        }
        None
    }

    fn has_failed(&self, q: usize, p: usize) -> bool {
        let bit = q * MAX_CAMEL_LEN + p;
        self.failed[bit / 64] & (1 << (bit % 64)) != 0
    }

    fn mark_failed(&mut self, q: usize, p: usize) {
        let bit = q * MAX_CAMEL_LEN + p;
        self.failed[bit / 64] |= 1 << (bit % 64);
    }
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
