//! Which bytes of a file are done.
//!
//! A download marks bytes done as its workers write them: a whole chunk, or,
//! when a link dies mid-chunk, just the part that arrived, so only the rest
//! is fetched again (PLAN.md section 3.6: never re-download what was already
//! paid for). What is still missing is then planned again.

use std::ops::Range;

/// Byte ranges, kept in order and merged, so no two overlap or touch.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Ranges(Vec<Range<u64>>);

impl Ranges {
    pub fn new() -> Self {
        Self::default()
    }

    /// Mark `range` done, merged with every range it overlaps or touches. An
    /// empty range changes nothing.
    pub fn insert(&mut self, range: Range<u64>) {
        if range.start >= range.end {
            return;
        }
        // The first range ending at or after the new one's start is the first
        // it can touch. From there on, every range starting at or before the
        // new one's end does touch it.
        let first = self.0.partition_point(|r| r.end < range.start);
        let last = first + self.0[first..].partition_point(|r| r.start <= range.end);
        let touching = &self.0[first..last];
        let start = touching
            .first()
            .map_or(range.start, |r| r.start.min(range.start));
        let end = touching.last().map_or(range.end, |r| r.end.max(range.end));
        self.0.splice(first..last, std::iter::once(start..end));
    }

    /// How many bytes are done.
    pub fn covered(&self) -> u64 {
        self.0.iter().map(|r| r.end - r.start).sum()
    }

    /// The bytes of a `length`-byte file not done yet, in order.
    pub fn missing(&self, length: u64) -> Vec<Range<u64>> {
        let mut gaps = Vec::new();
        let mut at = 0;
        for r in &self.0 {
            if r.start >= length {
                break;
            }
            if r.start > at {
                gaps.push(at..r.start);
            }
            at = r.end;
        }
        if at < length {
            gaps.push(at..length);
        }
        gaps
    }

    /// Whether every byte of a `length`-byte file is done.
    pub fn is_complete(&self, length: u64) -> bool {
        self.missing(length).is_empty()
    }

    /// The done ranges, in order.
    pub fn as_slice(&self) -> &[Range<u64>] {
        &self.0
    }
}

#[cfg(test)]
// Lists of byte ranges, often of one: not numbers meant to be collected.
#[allow(clippy::single_range_in_vec_init)]
mod tests {
    use super::*;

    fn ranges(inserted: &[Range<u64>]) -> Ranges {
        let mut done = Ranges::new();
        for r in inserted {
            done.insert(r.clone());
        }
        done
    }

    #[test]
    fn neighbours_and_overlaps_merge() {
        assert_eq!(ranges(&[0..4, 4..8]).as_slice(), [0..8]);
        assert_eq!(ranges(&[0..5, 3..9]).as_slice(), [0..9]);
        // Out of order, and one range joining two.
        assert_eq!(ranges(&[10..12, 0..2, 2..10]).as_slice(), [0..12]);
        // Inside another, and one spanning several.
        assert_eq!(ranges(&[0..10, 3..4]).as_slice(), [0..10]);
        assert_eq!(ranges(&[0..2, 4..6, 8..10, 1..9]).as_slice(), [0..10]);
        // Apart stays apart, in order.
        assert_eq!(ranges(&[10..12, 0..2]).as_slice(), [0..2, 10..12]);
    }

    #[test]
    fn an_empty_range_changes_nothing() {
        assert!(ranges(&[5..5]).as_slice().is_empty());
        assert_eq!(ranges(&[0..2, 9..9]).as_slice(), [0..2]);
    }

    #[test]
    fn missing_is_everything_not_done() {
        assert_eq!(ranges(&[2..4]).missing(10), [0..2, 4..10]);
        assert_eq!(ranges(&[0..3, 7..10]).missing(10), [3..7]);
        assert_eq!(ranges(&[]).missing(10), [0..10]);
        assert!(ranges(&[0..10]).missing(10).is_empty());
        assert!(ranges(&[]).missing(0).is_empty());
    }

    #[test]
    fn covered_counts_bytes_and_complete_means_nothing_missing() {
        let done = ranges(&[0..3, 7..10]);
        assert_eq!(done.covered(), 6);
        assert!(!done.is_complete(10));
        assert!(ranges(&[0..10]).is_complete(10));
        assert!(Ranges::new().is_complete(0));
    }
}
