//! Window-of-1 pacing policy (spec 006). Pure: the caller passes the time in.

use std::time::{Duration, Instant};

pub const MAX_DELAY_MS: i64 = 3_600_000;

pub fn clamp_delay_ms(ms: i64) -> i64 {
    ms.clamp(0, MAX_DELAY_MS)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InFlight {
    pub msg: i64,
    pub sent: Instant,
    pub acked: Option<Instant>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Next {
    /// The window is open and the pacing delay has passed.
    Send,
    /// Nothing may be sent before this instant.
    Wait(Instant),
}

#[derive(Debug, Clone)]
pub struct Pacer {
    timeout: Duration,
    in_flight: Option<InFlight>,
    released_at: Option<Instant>,
}

impl Pacer {
    pub fn new(timeout: Duration) -> Self {
        Pacer {
            timeout,
            in_flight: None,
            released_at: None,
        }
    }

    pub fn in_flight(&self) -> Option<InFlight> {
        self.in_flight
    }

    pub fn start(&mut self, msg: i64, now: Instant) {
        self.in_flight = Some(InFlight {
            msg,
            sent: now,
            acked: None,
        });
    }

    /// Record the device's ack; the in-flight timeout restarts from here.
    pub fn acked(&mut self, now: Instant) {
        if let Some(f) = &mut self.in_flight
            && f.acked.is_none()
        {
            f.acked = Some(now);
        }
    }

    /// The message left the in-flight state (terminal for pacing).
    pub fn release(&mut self, now: Instant) {
        if self.in_flight.take().is_some() {
            self.released_at = Some(now);
        }
    }

    pub fn deadline(&self) -> Option<Instant> {
        self.in_flight
            .map(|f| f.acked.unwrap_or(f.sent) + self.timeout)
    }

    pub fn poll(&mut self, now: Instant, delay: Duration) -> Next {
        if let Some(deadline) = self.deadline() {
            if now < deadline {
                return Next::Wait(deadline);
            }
            self.release(now);
        }
        match self.released_at {
            Some(r) if now < r + delay => Next::Wait(r + delay),
            _ => Next::Send,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: Duration = Duration::from_millis(1);

    #[test]
    fn ac_006_01_window_of_one_and_delay() {
        let t0 = Instant::now();
        let mut p = Pacer::new(MS * 1000);
        assert_eq!(p.poll(t0, MS * 200), Next::Send);
        p.start(1, t0);
        assert_eq!(p.poll(t0 + MS * 10, MS * 200), Next::Wait(t0 + MS * 1000));
        p.release(t0 + MS * 50);
        assert_eq!(p.poll(t0 + MS * 60, MS * 200), Next::Wait(t0 + MS * 250));
        assert_eq!(p.poll(t0 + MS * 250, MS * 200), Next::Send);
    }

    #[test]
    fn ac_006_02_inflight_timeout_moves_on() {
        let t0 = Instant::now();
        let mut p = Pacer::new(MS * 100);
        p.start(1, t0);
        p.acked(t0 + MS * 50);
        assert_eq!(p.deadline(), Some(t0 + MS * 150));
        assert_eq!(
            p.poll(t0 + MS * 120, Duration::ZERO),
            Next::Wait(t0 + MS * 150)
        );
        assert_eq!(p.poll(t0 + MS * 150, Duration::ZERO), Next::Send);
        assert_eq!(p.in_flight(), None);
    }

    #[test]
    fn second_ack_does_not_extend() {
        let t0 = Instant::now();
        let mut p = Pacer::new(MS * 100);
        p.start(1, t0);
        p.acked(t0 + MS * 10);
        p.acked(t0 + MS * 90);
        assert_eq!(p.deadline(), Some(t0 + MS * 110));
    }

    #[test]
    fn delay_is_clamped() {
        assert_eq!(clamp_delay_ms(-5), 0);
        assert_eq!(clamp_delay_ms(5000), 5000);
        assert_eq!(clamp_delay_ms(9_999_999), MAX_DELAY_MS);
    }
}
