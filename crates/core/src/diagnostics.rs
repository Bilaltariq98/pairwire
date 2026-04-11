//! Lightweight diagnostic counters for frame drops and other metrics.
//!
//! All counters use `AtomicU64` with `Relaxed` ordering — good enough for
//! diagnostics without contention overhead.

use std::sync::atomic::{AtomicU64, Ordering};

/// Tracks how many frames/packets were dropped at a particular stage.
#[derive(Debug)]
pub struct DropCounter {
    name: &'static str,
    count: AtomicU64,
}

impl DropCounter {
    /// Create a new named counter starting at zero.
    #[must_use]
    pub const fn new(name: &'static str) -> Self {
        Self {
            name,
            count: AtomicU64::new(0),
        }
    }

    /// Increment by one and return the new total.
    pub fn increment(&self) -> u64 {
        self.count.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// Current count.
    #[must_use]
    pub fn get(&self) -> u64 {
        self.count.load(Ordering::Relaxed)
    }

    /// Counter name (for logging).
    #[must_use]
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// Reset to zero, returning the previous value.
    pub fn reset(&self) -> u64 {
        self.count.swap(0, Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counter_starts_at_zero() {
        let c = DropCounter::new("test");
        assert_eq!(c.get(), 0);
    }

    #[test]
    fn increment_returns_new_value() {
        let c = DropCounter::new("test");
        assert_eq!(c.increment(), 1);
        assert_eq!(c.increment(), 2);
        assert_eq!(c.get(), 2);
    }

    #[test]
    fn reset_returns_old_value() {
        let c = DropCounter::new("test");
        c.increment();
        c.increment();
        c.increment();
        assert_eq!(c.reset(), 3);
        assert_eq!(c.get(), 0);
    }

    #[test]
    fn name_is_accessible() {
        let c = DropCounter::new("capture_drops");
        assert_eq!(c.name(), "capture_drops");
    }

    #[test]
    fn concurrent_increments() {
        use std::sync::Arc;
        use std::thread;

        let c = Arc::new(DropCounter::new("concurrent"));
        let mut handles = Vec::new();

        for _ in 0..8 {
            let c = Arc::clone(&c);
            handles.push(thread::spawn(move || {
                for _ in 0..1000 {
                    c.increment();
                }
            }));
        }

        for h in handles {
            h.join().unwrap();
        }

        assert_eq!(c.get(), 8000);
    }
}
