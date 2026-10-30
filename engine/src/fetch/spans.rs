//! One connection's part of the file, and where to cut one for another.
//!
//! A connection asks for a long span in one request, not one 4 MiB chunk
//! after another: Hetzner's and Vultr's speed servers allow each address
//! about one request every five seconds (PLAN.md phase 3 item 8), so the
//! number of requests is what counts. When a connection has nothing left to
//! do and nothing is unclaimed, the claim with the most left is cut in two
//! and it takes the back half; the owner's answer runs on past its new end,
//! so the owner stops there. That is how the work stays balanced, as the
//! shared pile of chunks kept it before, and how the end of a file still
//! splits finely: halving goes on until a claim has less than twice
//! `min_split` left.
//!
//! A claim is `start <= written <= reserved <= end`. Its connection reserves
//! the bytes it is about to write, then writes them, then counts them
//! written, so a cut, which falls past `reserved`, can never take bytes that
//! are being written: no byte is written twice.

use std::ops::Range;
use std::sync::Mutex;

use tokio::sync::Notify;

/// One connection's part of the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Claim {
    pub start: u64,
    /// Written to the file up to here.
    pub written: u64,
    /// Handed to a write up to here.
    pub reserved: u64,
    pub end: u64,
}

impl Claim {
    pub fn new(range: Range<u64>) -> Self {
        Self {
            start: range.start,
            written: range.start,
            reserved: range.start,
            end: range.end,
        }
    }

    /// Bytes not yet handed to a write.
    pub fn left(&self) -> u64 {
        self.end - self.reserved
    }

    pub fn is_done(&self) -> bool {
        self.reserved == self.end
    }

    /// The next bytes to write, at most `n`: empty once at the end.
    pub fn reserve(&mut self, n: u64) -> Range<u64> {
        let at = self.reserved;
        self.reserved += n.min(self.left());
        at..self.reserved
    }

    /// `n` reserved bytes are now in the file.
    pub fn wrote(&mut self, n: u64) {
        self.written = (self.written + n).min(self.reserved);
    }

    /// Cut what is left in two, if at least twice `min` is left: the back
    /// half, someone else's from now on. Never cuts a reserved byte.
    pub fn split(&mut self, min: u64) -> Option<Range<u64>> {
        let left = self.left();
        if left < 2 * min.max(1) {
            return None;
        }
        let cut = self.reserved + left / 2;
        let tail = cut..self.end;
        self.end = cut;
        Some(tail)
    }

    /// Everything not reserved yet, which this connection gives up.
    pub fn give_up(&mut self) -> Option<Range<u64>> {
        let rest = self.reserved..self.end;
        self.end = self.reserved;
        (!rest.is_empty()).then_some(rest)
    }
}

/// A seat's claim, shared by the coordinator, which hands out and cuts, and
/// the seat's worker, which writes. The lock is held for a few instructions
/// at a time, never across an `.await`. `ready` wakes the worker when the
/// coordinator gives it work.
#[derive(Debug, Default)]
pub(crate) struct Hand {
    pub claim: Mutex<Claim>,
    pub ready: Notify,
}

/// The seat whose claim to cut for a connection with nothing to do: the one
/// with the most left, the lower seat of equals, if it can be cut at all.
pub(crate) fn to_split(claims: &[(u64, Claim)], min: u64) -> Option<u64> {
    claims
        .iter()
        .filter(|(_, c)| c.left() >= 2 * min.max(1))
        .min_by_key(|(seat, c)| (std::cmp::Reverse(c.left()), *seat))
        .map(|(seat, _)| *seat)
}

#[cfg(test)]
mod tests {
    use super::*;

    const KIB: u64 = 1 << 10;
    const MIB: u64 = 1 << 20;

    #[test]
    fn a_claim_reserves_in_order_and_never_past_its_end() {
        let mut claim = Claim::new(0..10);
        assert_eq!(claim.reserve(4), 0..4);
        claim.wrote(4);
        assert_eq!(claim.reserve(8), 4..10);
        assert_eq!(claim.reserve(1), 10..10);
        assert!(claim.is_done());
        assert_eq!(claim.written, 4, "only what was written counts");
        claim.wrote(6);
        assert_eq!(claim.written, 10);
        // Never written past what was reserved.
        claim.wrote(5);
        assert_eq!(claim.written, 10);
    }

    #[test]
    fn a_cut_halves_what_is_left_past_the_reserved_bytes() {
        let mut claim = Claim::new(0..100 * MIB);
        claim.reserve(10 * MIB);
        assert_eq!(claim.split(512 * KIB), Some(55 * MIB..100 * MIB));
        assert_eq!(claim.end, 55 * MIB);
        assert_eq!(claim.reserved, 10 * MIB);
    }

    #[test]
    fn a_claim_too_small_to_halve_is_not_cut() {
        let min = 512 * KIB;
        let mut short = Claim::new(0..MIB - 1);
        assert_eq!(short.split(min), None);
        assert_eq!(short.end, MIB - 1);
        let mut enough = Claim::new(0..MIB);
        assert_eq!(enough.split(min), Some(512 * KIB..MIB));
        // With no floor at all, still never an empty half.
        assert_eq!(Claim::new(0..1).split(0), None);
        assert_eq!(Claim::new(0..2).split(0), Some(1..2));
    }

    #[test]
    fn giving_up_returns_only_what_is_not_reserved() {
        let mut claim = Claim::new(100..200);
        claim.reserve(30);
        claim.wrote(30);
        assert_eq!(claim.give_up(), Some(130..200));
        assert!(claim.is_done());
        assert_eq!(claim.give_up(), None);
        assert_eq!(Claim::default().give_up(), None);
    }

    #[test]
    fn the_claim_with_most_left_is_cut() {
        let min = 512 * KIB;
        let claim = |left| Claim::new(0..left);
        let claims = [
            (3, claim(4 * MIB)),
            (1, claim(8 * MIB)),
            (2, claim(8 * MIB)),
        ];
        assert_eq!(to_split(&claims, min), Some(1), "the lower seat of equals");
        let mut part_written = claim(16 * MIB);
        part_written.reserve(12 * MIB);
        // 16 MiB long, but only 4 left: the 8 MiB ones go first.
        let claims = [(1, part_written), (2, claim(8 * MIB))];
        assert_eq!(to_split(&claims, min), Some(2));
        let small = [(1, claim(MIB - 1)), (2, claim(MIB / 2))];
        assert_eq!(to_split(&small, min), None);
        assert_eq!(to_split(&[], min), None);
    }
}
