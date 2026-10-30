//! One connection's part of the file, and where to cut one for another.
//!
//! A connection asks for a long span in one request, not one 4 MiB chunk
//! after another: Hetzner's and Vultr's speed servers allow each address
//! about one request every five seconds (PLAN.md phase 3 item 8), so the
//! number of requests is what counts. When a connection has nothing left to
//! do and nothing is unclaimed, the claim that would finish last is cut and
//! it takes the back; the owner's answer runs on past its new end, so the
//! owner stops there. That is how the work stays balanced, as the shared
//! pile of chunks kept it before, and how the end of a file still splits:
//! once speeds are measured, the cut falls where both would finish together
//! and is made only when it brings the end at least `min_split_time` sooner,
//! so a slow link's last part is cut whatever its size and one about to
//! finish is left alone; before, a claim is halved while it has twice
//! `min_split` left.
//!
//! A claim is `start <= written <= reserved <= end`. Its connection reserves
//! the bytes it is about to write, then writes them, then counts them
//! written, so a cut, which falls past `reserved`, can never take bytes that
//! are being written: no byte is written twice.

use std::ops::Range;
use std::sync::Mutex;
use std::sync::atomic::AtomicU64;
use std::time::Duration;

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

    /// Cut what is left after its first `keep` bytes: the rest, someone
    /// else's from now on. `None` if that leaves either side nothing. Never
    /// cuts a reserved byte.
    pub fn split(&mut self, keep: u64) -> Option<Range<u64>> {
        if keep == 0 || keep >= self.left() {
            return None;
        }
        let cut = self.reserved + keep;
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
/// coordinator gives it work; `got` counts the bytes the seat has received
/// over all its claims, as each piece arrives, for its speed.
#[derive(Debug, Default)]
pub(crate) struct Hand {
    pub claim: Mutex<Claim>,
    pub ready: Notify,
    pub got: AtomicU64,
}

/// The least either side of a cut keeps when the owner's speed is known.
pub(crate) const LEAST_PART: u64 = 64 << 10;

/// What makes a claim worth cutting: with no speed known, at least twice
/// `min` bytes left; with the owner's known, a cut that brings its end at
/// least `min_time` sooner.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Floor {
    pub min: u64,
    pub min_time: Duration,
}

/// A cut for a connection with nothing to do: whose claim, and how many of
/// the bytes it has left its owner keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Cut {
    pub seat: u64,
    pub keep: u64,
}

/// A seat's speed in bits a second: its own, or, not measured yet, its
/// link's shared by the link's connections.
pub(crate) fn seat_rate(own: Option<u64>, link: Option<u64>, connections: u32) -> Option<u64> {
    let shared = || link.map(|bps| bps / u64::from(connections.max(1)));
    own.filter(|&bps| bps > 0)
        .or_else(shared)
        .filter(|&bps| bps > 0)
}

/// Where to cut for a connection whose speed is `taker`: of the claims
/// worth cutting, the one with the most time left at its owner's speed, the
/// lower seat of equals; the owner keeps a share by both speeds, so the two
/// finish together. With any claim's speed unknown, the one with the most
/// bytes left, halved, as before speeds were measured.
pub(crate) fn to_split(
    claims: &[(u64, Claim, Option<u64>)],
    taker: Option<u64>,
    floor: Floor,
) -> Option<Cut> {
    let timed = claims
        .iter()
        .all(|(_, _, rate)| rate.is_some_and(|r| r > 0));
    // The cut so far: its seat, what ranks it (seconds or bytes left), and
    // what its owner keeps.
    let mut best: Option<(u64, f64, u64)> = None;
    for &(seat, claim, rate) in claims {
        let left = claim.left();
        let (rank, keep) = match rate.filter(|_| timed) {
            Some(owner) => {
                // A taker of no known speed counts as fast as the owner.
                let taker = taker.filter(|&r| r > 0).unwrap_or(owner);
                let time = left as f64 * 8.0 / owner as f64;
                let sooner = time * taker as f64 / (owner as f64 + taker as f64);
                if sooner < floor.min_time.as_secs_f64() || left < 2 * LEAST_PART {
                    continue;
                }
                let share =
                    u128::from(left) * u128::from(owner) / (u128::from(owner) + u128::from(taker));
                let keep = u64::try_from(share).unwrap_or(left);
                (time, keep.clamp(LEAST_PART, left - LEAST_PART))
            }
            None => {
                if left < 2 * floor.min.max(1) {
                    continue;
                }
                (left as f64, left / 2)
            }
        };
        let better = best.is_none_or(|(at, most, _)| rank > most || (rank == most && seat < at));
        if better {
            best = Some((seat, rank, keep));
        }
    }
    best.map(|(seat, _, keep)| Cut { seat, keep })
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
    fn a_cut_keeps_what_it_is_told_past_the_reserved_bytes() {
        let mut claim = Claim::new(0..100 * MIB);
        claim.reserve(10 * MIB);
        assert_eq!(claim.split(45 * MIB), Some(55 * MIB..100 * MIB));
        assert_eq!(claim.end, 55 * MIB);
        assert_eq!(claim.reserved, 10 * MIB);
    }

    #[test]
    fn a_cut_never_leaves_a_side_with_nothing() {
        let mut claim = Claim::new(0..10);
        claim.reserve(4);
        assert_eq!(claim.split(0), None, "the owner keeps something");
        assert_eq!(claim.split(6), None, "the taker gets something");
        assert_eq!(claim.split(7), None);
        assert_eq!(claim.end, 10);
        assert_eq!(claim.split(5), Some(9..10));
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

    const FLOOR: Floor = Floor {
        min: 512 * KIB,
        min_time: Duration::from_millis(500),
    };

    /// Bits a second for `n` MiB a second.
    fn mib_s(n: u64) -> u64 {
        n * MIB * 8
    }

    fn claim(left: u64) -> Claim {
        Claim::new(0..left)
    }

    #[test]
    fn without_rates_the_most_bytes_left_is_halved() {
        let cut = |seat, keep| Some(Cut { seat, keep });
        let claims = [
            (3, claim(4 * MIB), None),
            (1, claim(8 * MIB), None),
            (2, claim(8 * MIB), None),
        ];
        assert_eq!(
            to_split(&claims, None, FLOOR),
            cut(1, 4 * MIB),
            "the lower seat of equals"
        );
        let mut part_written = claim(16 * MIB);
        part_written.reserve(12 * MIB);
        // 16 MiB long, but only 4 left: the 8 MiB one goes first.
        let claims = [(1, part_written, None), (2, claim(8 * MIB), None)];
        assert_eq!(to_split(&claims, None, FLOOR), cut(2, 4 * MIB));
        // Under twice the floor, never cut; exactly twice, halved.
        let small = [(1, claim(MIB - 1), None), (2, claim(MIB / 2), None)];
        assert_eq!(to_split(&small, None, FLOOR), None);
        assert_eq!(
            to_split(&[(1, claim(MIB), None)], None, FLOOR),
            cut(1, MIB / 2)
        );
        assert_eq!(to_split(&[], None, FLOOR), None);
        // One claim's rate unknown: bytes decide for all, as the seconds
        // left of one cannot be weighed against the bytes of another.
        let mixed = [
            (1, claim(8 * MIB), Some(mib_s(1))),
            (2, claim(9 * MIB), None),
        ];
        assert_eq!(to_split(&mixed, None, FLOOR), cut(2, 9 * MIB / 2));
    }

    #[test]
    fn the_claim_with_most_time_left_is_cut() {
        // A: 40 MiB at 80 Mbps, 4 s. B: 20 MiB at 8 Mbps, 20 s.
        let claims = [
            (1, claim(40 * MIB), Some(80_000_000)),
            (2, claim(20 * MIB), Some(8_000_000)),
        ];
        let got = to_split(&claims, None, FLOOR).map(|c| c.seat);
        assert_eq!(got, Some(2));
        // A taker of no known speed: halved.
        assert_eq!(
            to_split(&claims, None, FLOOR).map(|c| c.keep),
            Some(10 * MIB)
        );
    }

    #[test]
    fn an_unmeasured_seat_takes_its_links_rate_per_connection() {
        assert_eq!(seat_rate(Some(5), Some(80), 4), Some(5), "its own first");
        assert_eq!(seat_rate(None, Some(80), 4), Some(20));
        // A first sample with nothing in it yet is no measure either.
        assert_eq!(seat_rate(Some(0), Some(80), 4), Some(20));
        assert_eq!(seat_rate(None, Some(80), 0), Some(80));
        assert_eq!(seat_rate(None, Some(0), 4), None);
        assert_eq!(seat_rate(None, None, 4), None);
    }

    #[test]
    fn a_slow_claim_under_a_mebibyte_is_still_cut() {
        let slow = [(1, claim(900 * KIB), Some(mib_s(1)))];
        let cut = to_split(&slow, Some(mib_s(16)), FLOOR).expect("not cut");
        // A seventeenth is under the least part: it keeps that.
        assert_eq!(
            cut,
            Cut {
                seat: 1,
                keep: LEAST_PART
            }
        );
        let fast = [(1, claim(900 * KIB), Some(mib_s(16)))];
        assert_eq!(to_split(&fast, Some(mib_s(16)), FLOOR), None);
    }

    #[test]
    fn the_cut_leaves_both_finishing_together() {
        let claims = [(1, claim(17 * MIB), Some(mib_s(1)))];
        let cut = to_split(&claims, Some(mib_s(16)), FLOOR);
        assert_eq!(cut, Some(Cut { seat: 1, keep: MIB }));
    }

    #[test]
    fn a_fast_owner_is_not_cut_for_a_slow_taker() {
        // 1 s left; shared with a taker sixteen times slower it ends 59 ms
        // sooner, which a new request and what is in flight cost more than.
        let claims = [(1, claim(16 * MIB), Some(mib_s(16)))];
        assert_eq!(to_split(&claims, Some(mib_s(1)), FLOOR), None);
        // Twice as long left, it would save 118 ms: still not worth it.
        let claims = [(1, claim(32 * MIB), Some(mib_s(16)))];
        assert_eq!(to_split(&claims, Some(mib_s(1)), FLOOR), None);
        // A taker as fast: half of 1 s saved.
        let claims = [(1, claim(16 * MIB), Some(mib_s(16)))];
        let cut = to_split(&claims, Some(mib_s(16)), FLOOR);
        assert_eq!(
            cut,
            Some(Cut {
                seat: 1,
                keep: 8 * MIB
            })
        );
    }
}
