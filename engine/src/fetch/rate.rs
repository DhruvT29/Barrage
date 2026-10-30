//! A speed that follows a change in seconds: an average of the rate between
//! samples, each weighted by the time it covers.
//!
//! A link's speed decides where its work is cut (spans.rs) and is what the
//! page shows. Measured over a fixed window, as phase 3 did, one slow moment
//! pulls it down for the whole window and the figure jumps when that moment
//! drops out. Weighted by time, `1 - 2^(-dt / half_life)`, a change is half
//! followed after one half-life however often it is sampled, and one tick
//! with no bytes moves it only a little.
//!
//! The average starts from nothing and is divided by the weight it has
//! gathered, `1 - 2^(-t / half_life)` after `t`: the first interval counts
//! at its own speed, and each after it averages in, so one interval that
//! happened to see a lump of bytes does not set the figure for seconds.

use std::time::Duration;

use tokio::time::Instant;

/// A speed in bits a second, from a byte count sampled now and then.
#[derive(Debug, Clone, Copy, Default)]
pub struct Rate {
    last: Option<(Instant, u64)>,
    /// The weighted sum of each interval's speed.
    sum: f64,
    /// The weight gathered since bytes first arrived.
    weight: f64,
}

impl Rate {
    /// `total` bytes so far, at `now`. Until bytes first arrive there is no
    /// speed: a new connection's first moments are its wait for an answer,
    /// and an average begun there would start at nothing and climb.
    pub fn sample(&mut self, now: Instant, total: u64, half_life: Duration) {
        let Some((then, before)) = self.last else {
            self.last = Some((now, total));
            return;
        };
        // Nothing to learn from no time at all; the bytes count next time.
        let dt = now.saturating_duration_since(then).as_secs_f64();
        if dt <= 0.0 {
            return;
        }
        self.last = Some((now, total));
        let got = total.saturating_sub(before) as f64 * 8.0 / dt;
        if self.weight == 0.0 && got == 0.0 {
            return;
        }
        let keep = (-dt / half_life.as_secs_f64()).exp2();
        self.sum = self.sum * keep + (1.0 - keep) * got;
        self.weight = self.weight * keep + (1.0 - keep);
    }

    /// The speed, once bytes have arrived over two samples.
    pub fn bps(&self) -> Option<u64> {
        (self.weight > 0.0).then(|| (self.sum / self.weight).round() as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HALF_LIFE: Duration = Duration::from_millis(1500);

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// Bytes a second for `mbps` megabits a second.
    fn bytes_a_second(mbps: f64) -> f64 {
        mbps * 1e6 / 8.0
    }

    /// Feed `rate` `mbps` for `time` in ticks of `tick`, from `at` and
    /// `total`; where it ends.
    fn feed(
        rate: &mut Rate,
        at: &mut Instant,
        total: &mut f64,
        mbps: f64,
        time: Duration,
        tick: Duration,
    ) {
        let ticks = (time.as_millis() / tick.as_millis()) as u32;
        for _ in 0..ticks {
            *at += tick;
            *total += bytes_a_second(mbps) * tick.as_secs_f64();
            rate.sample(*at, total.round() as u64, HALF_LIFE);
        }
    }

    fn mbps(rate: &Rate) -> f64 {
        rate.bps().expect("no estimate") as f64 / 1e6
    }

    fn started() -> (Rate, Instant, f64) {
        let mut rate = Rate::default();
        let at = Instant::now();
        rate.sample(at, 0, HALF_LIFE);
        (rate, at, 0.0)
    }

    #[test]
    fn a_steady_speed_is_its_own_estimate() {
        let (mut rate, mut at, mut total) = started();
        feed(&mut rate, &mut at, &mut total, 80.0, ms(5000), ms(250));
        assert!((mbps(&rate) - 80.0).abs() < 0.8, "{}", mbps(&rate));
    }

    #[test]
    fn the_estimate_follows_a_change_within_seconds() {
        let (mut rate, mut at, mut total) = started();
        feed(&mut rate, &mut at, &mut total, 80.0, ms(10_000), ms(250));
        feed(&mut rate, &mut at, &mut total, 16.0, ms(1500), ms(250));
        assert!((mbps(&rate) - 48.0).abs() < 48.0 * 0.05, "{}", mbps(&rate));
        feed(&mut rate, &mut at, &mut total, 16.0, ms(5500), ms(250));
        assert!(mbps(&rate) < 16.0 * 1.2, "{}", mbps(&rate));
    }

    #[test]
    fn one_slow_moment_does_not_make_it_jump() {
        let (mut rate, mut at, mut total) = started();
        feed(&mut rate, &mut at, &mut total, 80.0, ms(10_000), ms(250));
        feed(&mut rate, &mut at, &mut total, 0.0, ms(250), ms(250));
        assert!(mbps(&rate) > 70.0, "{}", mbps(&rate));
    }

    #[test]
    fn the_first_figure_is_the_first_intervals_own() {
        let mut rate = Rate::default();
        let at = Instant::now();
        assert_eq!(rate.bps(), None, "nothing before a sample");
        rate.sample(at, 0, HALF_LIFE);
        assert_eq!(rate.bps(), None, "one sample is no speed");
        // 1 MB in 100 ms: 80 Mbps at once, not a climb from nothing.
        rate.sample(at + ms(100), 1_000_000, HALF_LIFE);
        assert_eq!(rate.bps(), Some(80_000_000));
    }

    #[test]
    fn the_wait_before_the_first_bytes_is_no_speed() {
        // A new connection's first ticks are its wait for an answer: no
        // estimate from them, or it would start at nothing and climb.
        let mut rate = Rate::default();
        let at = Instant::now();
        rate.sample(at, 0, HALF_LIFE);
        rate.sample(at + ms(250), 0, HALF_LIFE);
        rate.sample(at + ms(500), 0, HALF_LIFE);
        assert_eq!(rate.bps(), None);
        rate.sample(at + ms(600), 1_000_000, HALF_LIFE);
        assert_eq!(rate.bps(), Some(80_000_000));
    }

    #[test]
    fn bytes_in_lumps_average_out() {
        // 256 KiB every 62.5 ms, 4 MiB a second, sampled every 20 ms: most
        // ticks see nothing and one in three or four a lump. Within a tenth
        // of the true speed after a second, not set by the first lump.
        let truth = 4.0 * 8.0 * 1024.0 * 1024.0 / 1e6;
        let mut rate = Rate::default();
        let at = Instant::now();
        for tick in 0..=50u64 {
            let lumps = (tick * 20 * 16) / 1000;
            rate.sample(at + ms(tick * 20), lumps * 256 * 1024, HALF_LIFE);
        }
        assert!(
            (mbps(&rate) - truth).abs() < truth * 0.1,
            "{} for {truth}",
            mbps(&rate)
        );
    }

    #[test]
    fn ticks_of_any_length_agree() {
        let run = |tick| {
            let (mut rate, mut at, mut total) = started();
            feed(&mut rate, &mut at, &mut total, 80.0, ms(2000), tick);
            feed(&mut rate, &mut at, &mut total, 16.0, ms(2000), tick);
            mbps(&rate)
        };
        let (fine, coarse) = (run(ms(100)), run(ms(500)));
        assert!((fine - coarse).abs() < fine * 0.01, "{fine} and {coarse}");
    }

    #[test]
    fn two_samples_at_the_same_moment_change_nothing() {
        let (mut rate, mut at, mut total) = started();
        feed(&mut rate, &mut at, &mut total, 80.0, ms(1000), ms(250));
        let before = rate.bps();
        rate.sample(at, total as u64 + 1_000_000, HALF_LIFE);
        assert_eq!(rate.bps(), before);
    }
}
