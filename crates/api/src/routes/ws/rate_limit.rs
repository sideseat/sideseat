//! Per-connection rolling rate limiter for inbound frames.
//!
//! Token-bucket-ish: counts arrivals within the rolling window; resets when
//! the window has elapsed since `window_start`.

use std::time::{Duration, Instant};

pub struct RateLimiter {
    capacity: u32,
    window: Duration,
    count: u32,
    window_start: Instant,
}

impl RateLimiter {
    pub fn new(capacity: u32, window_secs: u64) -> Self {
        Self {
            capacity,
            window: Duration::from_secs(window_secs),
            count: 0,
            window_start: Instant::now(),
        }
    }

    /// Returns `true` if allowed; `false` if the limit is exceeded.
    pub fn allow(&mut self) -> bool {
        let now = Instant::now();
        if now.duration_since(self.window_start) >= self.window {
            self.count = 0;
            self.window_start = now;
        }
        if self.count >= self.capacity {
            false
        } else {
            self.count += 1;
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_up_to_capacity_then_blocks() {
        let mut rl = RateLimiter::new(3, 60);
        assert!(rl.allow());
        assert!(rl.allow());
        assert!(rl.allow());
        assert!(!rl.allow());
    }
}
