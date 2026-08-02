//! Rate limiting for password attempts.

use std::time::{Duration, Instant};

/// A simple fixed-window rate limiter.
#[derive(Debug)]
pub struct RateLimiter {
    limit: u32,
    window: Duration,
    attempts: Vec<Instant>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateDecision {
    Allowed,
    Denied { retry_after: Duration },
}

impl RateLimiter {
    pub fn new(limit: u32, window: Duration) -> Self {
        Self {
            limit,
            window,
            attempts: Vec::new(),
        }
    }

    /// Record an attempt and return whether it is permitted.
    pub fn record(&mut self) -> RateDecision {
        let now = Instant::now();
        self.attempts
            .retain(|t| now.duration_since(*t) < self.window);
        if self.attempts.len() >= self.limit as usize {
            let oldest = self.attempts.first().copied().unwrap_or(now);
            let retry_after = self.window.saturating_sub(now.duration_since(oldest));
            return RateDecision::Denied { retry_after };
        }
        self.attempts.push(now);
        RateDecision::Allowed
    }

    /// Check without recording.
    pub fn check(&self) -> RateDecision {
        let now = Instant::now();
        let active = self
            .attempts
            .iter()
            .filter(|t| now.duration_since(**t) < self.window)
            .count();
        if active >= self.limit as usize {
            let oldest = self.attempts.first().copied().unwrap_or(now);
            let retry_after = self.window.saturating_sub(now.duration_since(oldest));
            RateDecision::Denied { retry_after }
        } else {
            RateDecision::Allowed
        }
    }
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new(5, Duration::from_secs(60))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_attempts() {
        let mut limiter = RateLimiter::new(2, Duration::from_secs(60));
        assert_eq!(limiter.record(), RateDecision::Allowed);
        assert_eq!(limiter.record(), RateDecision::Allowed);
        assert!(matches!(limiter.record(), RateDecision::Denied { .. }));
    }

    #[test]
    fn window_resets() {
        let mut limiter = RateLimiter::new(1, Duration::from_millis(50));
        assert_eq!(limiter.record(), RateDecision::Allowed);
        std::thread::sleep(Duration::from_millis(60));
        assert_eq!(limiter.record(), RateDecision::Allowed);
    }
}
