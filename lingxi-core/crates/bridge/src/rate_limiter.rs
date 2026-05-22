//! Token-bucket rate limiter keyed by an arbitrary string (A4).
//!
//! Used to throttle pairing-code generation per project: the burst capacity
//! limits how many fresh codes can be issued in a row, and the refill rate
//! controls how quickly the budget recovers. The bucket map is `Mutex`-guarded
//! because pairing code generation is rare and lock contention is irrelevant.

#![allow(clippy::unwrap_used)]

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

/// Token-bucket rate limiter.
///
/// One bucket is created lazily per distinct `key`. Each [`try_acquire`]
/// call consumes one token if available, otherwise returns `false`.
///
/// [`try_acquire`]: Self::try_acquire
pub struct RateLimiter {
    buckets: Mutex<HashMap<String, Bucket>>,
    capacity: u32,
    refill_per_second: f64,
}

#[derive(Debug)]
struct Bucket {
    tokens: f64,
    last_refill: Instant,
}

impl RateLimiter {
    /// Create a new limiter with the given burst `capacity` and continuous
    /// `refill_per_second` rate.
    #[must_use]
    pub fn new(capacity: u32, refill_per_second: f64) -> Self {
        Self {
            buckets: Mutex::new(HashMap::new()),
            capacity,
            refill_per_second,
        }
    }

    /// Try to consume one token for `key`. Returns `true` when granted.
    #[allow(clippy::cast_precision_loss)]
    pub fn try_acquire(&self, key: &str) -> bool {
        let now = Instant::now();
        let mut buckets = self.buckets.lock().unwrap();
        let capacity_f = f64::from(self.capacity);
        let bucket = buckets.entry(key.to_string()).or_insert(Bucket {
            tokens: capacity_f,
            last_refill: now,
        });
        let elapsed = now.duration_since(bucket.last_refill).as_secs_f64();
        bucket.tokens = (bucket.tokens + elapsed * self.refill_per_second).min(capacity_f);
        bucket.last_refill = now;
        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_then_refills() {
        let rl = RateLimiter::new(3, 1.0); // 3 burst, 1/sec refill
        let k = "alice";
        assert!(rl.try_acquire(k));
        assert!(rl.try_acquire(k));
        assert!(rl.try_acquire(k));
        assert!(!rl.try_acquire(k));
    }
}
