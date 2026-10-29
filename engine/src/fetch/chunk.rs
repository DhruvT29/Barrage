//! The chunks still to fetch.
//!
//! Workers pull chunks from one shared pile, so a faster link simply takes
//! more of them (PLAN.md section 3.6: work stealing). A chunk is 4 MiB, big
//! enough that the round trip each request pays is a small part of its time.
//! In the endgame, the last 5% of the file but never more than 64 MiB,
//! chunks are 256 KiB, so no worker is left holding a big one while the rest
//! wait: BitTorrent's endgame, for the same problem.

use std::ops::Range;

use super::ranges::Ranges;

/// A chunk before the endgame.
pub const CHUNK: u64 = 4 << 20;

/// A chunk in the endgame.
pub const ENDGAME_CHUNK: u64 = 256 << 10;

/// The endgame is the last 5% of the file, but never more than this. 5% of
/// 10 GiB would be 2,048 small requests, each paying a full round trip (the
/// tether's reached 934 ms in phase 1); 64 MiB is 256 of them, enough to keep
/// every worker busy while the last big chunks finish.
pub const ENDGAME_MAX: u64 = 64 << 20;

/// The chunks of a `length`-byte file still to fetch once `done` is, in file
/// order, together exactly what is missing.
///
/// A chunk that reaches the endgame is at most [`ENDGAME_CHUNK`]. Before it,
/// chunks are [`CHUNK`] and stop where it begins, unless that is near enough
/// for one small chunk to reach it: a 1000-byte file is one chunk, not one of
/// 950 bytes and one of 50.
///
/// One entry per chunk, so the caller makes sure `length` is real first: a
/// download sizes its file on disk before planning, and a server claiming an
/// impossible length fails there.
pub fn plan(length: u64, done: &Ranges) -> Vec<Range<u64>> {
    let endgame = length - (length / 20).min(ENDGAME_MAX);
    let mut chunks = Vec::new();
    for gap in done.missing(length) {
        let mut at = gap.start;
        while at < gap.end {
            let end = if at < endgame && endgame - at > ENDGAME_CHUNK {
                at.saturating_add(CHUNK).min(endgame)
            } else {
                at.saturating_add(ENDGAME_CHUNK)
            };
            let end = end.min(gap.end);
            chunks.push(at..end);
            at = end;
        }
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    const KIB: u64 = 1 << 10;
    const MIB: u64 = 1 << 20;
    const GIB: u64 = 1 << 30;

    fn plan_of(length: u64) -> Vec<Range<u64>> {
        plan(length, &Ranges::new())
    }

    fn sizes(chunks: &[Range<u64>]) -> Vec<u64> {
        chunks.iter().map(|c| c.end - c.start).collect()
    }

    #[test]
    fn chunks_cover_the_file_once_in_order() {
        let chunks = plan_of(100 * MIB);
        assert_eq!(chunks.first().map(|c| c.start), Some(0));
        assert_eq!(chunks.last().map(|c| c.end), Some(100 * MIB));
        for pair in chunks.windows(2) {
            assert_eq!(pair[0].end, pair[1].start, "a gap or an overlap: {pair:?}");
        }
    }

    /// PLAN.md section 3.6: 4 MB chunks, shrinking to about 256 KB in the
    /// final few percent, so nobody is left holding a big one at the end.
    #[test]
    fn chunks_are_4_mib_until_the_last_5_percent() {
        let mut want = vec![4 * MIB; 23]; // to 92 MiB
        want.push(3 * MIB); // stops where the last 5% begins, at 95 MiB
        want.extend([256 * KIB; 20]); // the last 5 MiB
        assert_eq!(sizes(&plan_of(100 * MIB)), want);
    }

    /// 5% of 10 GiB is 512 MiB: 2,048 small requests, each paying a full
    /// round trip. Capped at 64 MiB, 256 of them.
    #[test]
    fn a_big_files_endgame_is_capped_at_64_mib() {
        let chunks = plan_of(10 * GIB);
        let small = chunks.iter().position(|c| c.end - c.start < 4 * MIB);
        let small = small.expect("no small chunks at all");
        assert_eq!(chunks[small].start, 10 * GIB - 64 * MIB);
        assert_eq!(sizes(&chunks[small..]), vec![256 * KIB; 256]);
    }

    /// A file that fits in one small chunk is one request, not split at the
    /// start of its last 5%.
    #[test]
    #[allow(clippy::single_range_in_vec_init)] // a list of one chunk is meant
    fn an_empty_file_has_no_chunks_and_a_tiny_one_has_one() {
        assert!(plan_of(0).is_empty());
        assert_eq!(plan_of(1000), [0..1000]);
        assert_eq!(plan_of(256 * KIB), [0..256 * KIB]);
    }

    #[test]
    fn done_bytes_are_never_planned_again() {
        let mut done = Ranges::new();
        done.insert(0..5 * MIB);
        done.insert(10 * MIB..12 * MIB);
        let chunks = plan(100 * MIB, &done);

        let mut planned = Ranges::new();
        for c in &chunks {
            let clear = done
                .as_slice()
                .iter()
                .all(|d| c.end <= d.start || c.start >= d.end);
            assert!(clear, "{c:?} overlaps a done range");
            planned.insert(c.clone());
        }
        assert_eq!(planned.as_slice(), done.missing(100 * MIB));
    }

    /// Resuming near the end still gets small chunks: the last 5% is a place
    /// in the file, not a share of what is left.
    #[test]
    fn the_endgame_is_measured_on_the_file_not_on_what_is_left() {
        let mut done = Ranges::new();
        done.insert(0..99 * MIB);
        assert_eq!(sizes(&plan(100 * MIB, &done)), vec![256 * KIB; 4]);
    }

    /// The rules over odd sizes and odd holes: in order, no chunk over 4 MiB,
    /// none over 256 KiB once it reaches the last 5% (at most 64 MiB), and
    /// together exactly what is missing.
    #[test]
    fn every_chunk_keeps_the_rules() {
        let lengths = [
            1,
            1000,
            256 * KIB + 1,
            5 * MIB + 7,
            100 * MIB + 12_345,
            10 * GIB + 1,
        ];
        for length in lengths {
            let endgame = length - (length / 20).min(64 * MIB);
            for hole in [0..0, 0..length / 2, length / 3..length / 3 + 1] {
                let mut done = Ranges::new();
                done.insert(hole.clone());
                let chunks = plan(length, &done);

                let mut planned = Ranges::new();
                for c in &chunks {
                    assert!(
                        c.start < c.end && c.end - c.start <= 4 * MIB,
                        "{length}: {c:?}"
                    );
                    if c.end > endgame {
                        assert!(c.end - c.start <= 256 * KIB, "{length}: {c:?}");
                    }
                    planned.insert(c.clone());
                }
                assert!(chunks.windows(2).all(|p| p[0].end <= p[1].start));
                assert_eq!(
                    planned.as_slice(),
                    done.missing(length),
                    "{length} {hole:?}"
                );
            }
        }
    }
}
