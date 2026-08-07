//! Being a polite guest on someone else's server.
//!
//! archive.org serves this collection for free and has no obligation to serve it to us at
//! all. 38,377 PDFs is a lot to ask for, and the difference between asking politely over two
//! days and hammering the endpoint for an afternoon costs us nothing and costs them real
//! money. This module is the part that makes the asking polite.
//!
//! ## Why the timing logic is pure
//!
//! [`Limiter::due_at`] computes *when* the next request may go out; [`Limiter::wait`] is the
//! thin wrapper that actually sleeps. Splitting them means the interesting behaviour —
//! back-off growth, `Retry-After`, the cap — is tested by arithmetic in microseconds rather
//! than by a test suite that sleeps for real.

use std::time::{Duration, Instant};

/// Default minimum gap between requests.
///
/// One second. Not tuned to the fastest rate archive.org will tolerate, because that is not
/// the question — the question is the slowest rate that still finishes, and at one request a
/// second the whole corpus takes about eleven hours.
pub const DEFAULT_INTERVAL: Duration = Duration::from_secs(1);

/// Never wait longer than this between retries, however many have failed.
///
/// Five minutes. Past this, the server is not briefly busy, it is down, and the run should be
/// making no progress loudly rather than quietly.
pub const MAX_BACKOFF: Duration = Duration::from_secs(300);

/// Paces requests and backs off when the server asks it to.
#[derive(Debug, Clone)]
pub struct Limiter {
    /// Minimum gap between the start of one request and the start of the next.
    interval: Duration,
    /// Extra delay imposed by back-off, reset by any success.
    penalty: Duration,
    last: Option<Instant>,
}

impl Limiter {
    pub fn new(interval: Duration) -> Self {
        Limiter {
            interval,
            penalty: Duration::ZERO,
            last: None,
        }
    }

    /// The earliest instant the next request may be made.
    ///
    /// Pure, so back-off behaviour can be tested without sleeping.
    pub fn due_at(&self, _now: Instant) -> Option<Instant> {
        self.last.map(|last| last + self.interval + self.penalty)
    }

    /// How long to sleep before the next request.
    pub fn due_in(&self, now: Instant) -> Duration {
        match self.due_at(now) {
            Some(due) if due > now => due - now,
            _ => Duration::ZERO,
        }
    }

    /// Sleep until the next request is due, then mark it as starting now.
    pub fn wait(&mut self) {
        let now = Instant::now();
        let delay = self.due_in(now);
        if !delay.is_zero() {
            std::thread::sleep(delay);
        }
        self.last = Some(Instant::now());
    }

    /// The request succeeded: forget any accumulated back-off.
    pub fn succeeded(&mut self) {
        self.penalty = Duration::ZERO;
    }

    /// The request failed. Grow the back-off, or take the server's own figure if it gave one.
    ///
    /// A `Retry-After` is obeyed as given rather than merely used as a floor: it is the
    /// server stating its terms, and the whole point of this module is to accept them.
    pub fn failed(&mut self, retry_after: Option<Duration>) {
        self.penalty = match retry_after {
            Some(d) => d.min(MAX_BACKOFF),
            // Double, starting from the base interval. Geometric rather than linear because
            // the case worth handling quickly is "briefly busy", and the case worth handling
            // gently is "still down after ten tries".
            None => {
                let doubled = if self.penalty.is_zero() {
                    self.interval
                } else {
                    self.penalty * 2
                };
                doubled.min(MAX_BACKOFF)
            }
        };
    }

    /// The back-off currently in force. For reporting.
    pub fn penalty(&self) -> Duration {
        self.penalty
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limiter() -> Limiter {
        Limiter::new(Duration::from_secs(1))
    }

    #[test]
    fn the_first_request_waits_for_nothing() {
        let l = limiter();
        assert_eq!(l.due_in(Instant::now()), Duration::ZERO);
    }

    #[test]
    fn a_second_request_waits_the_interval() {
        let mut l = limiter();
        let t0 = Instant::now();
        l.last = Some(t0);
        assert_eq!(l.due_in(t0), Duration::from_secs(1));
        // Half of it already spent.
        assert_eq!(l.due_in(t0 + Duration::from_millis(500)), Duration::from_millis(500));
        // All of it spent.
        assert_eq!(l.due_in(t0 + Duration::from_secs(2)), Duration::ZERO);
    }

    #[test]
    fn failures_back_off_geometrically() {
        let mut l = limiter();
        let t0 = Instant::now();
        l.last = Some(t0);

        l.failed(None);
        assert_eq!(l.due_in(t0), Duration::from_secs(2)); // interval + 1s penalty
        l.failed(None);
        assert_eq!(l.due_in(t0), Duration::from_secs(3)); // interval + 2s
        l.failed(None);
        assert_eq!(l.due_in(t0), Duration::from_secs(5)); // interval + 4s
    }

    /// Otherwise one bad patch would slow the rest of an eleven-hour run to a crawl.
    #[test]
    fn a_success_clears_the_back_off() {
        let mut l = limiter();
        let t0 = Instant::now();
        l.last = Some(t0);
        l.failed(None);
        l.failed(None);
        assert!(l.penalty() > Duration::ZERO);

        l.succeeded();
        assert_eq!(l.penalty(), Duration::ZERO);
        assert_eq!(l.due_in(t0), Duration::from_secs(1));
    }

    /// The server stating its terms. Obeyed as given.
    #[test]
    fn retry_after_is_taken_from_the_server() {
        let mut l = limiter();
        let t0 = Instant::now();
        l.last = Some(t0);
        l.failed(Some(Duration::from_secs(30)));
        assert_eq!(l.due_in(t0), Duration::from_secs(31));
    }

    #[test]
    fn back_off_is_capped_however_bad_it_gets() {
        let mut l = limiter();
        for _ in 0..40 {
            l.failed(None);
        }
        assert_eq!(l.penalty(), MAX_BACKOFF);

        // Including an absurd Retry-After, which some servers do send.
        l.failed(Some(Duration::from_secs(86_400)));
        assert_eq!(l.penalty(), MAX_BACKOFF);
    }
}
