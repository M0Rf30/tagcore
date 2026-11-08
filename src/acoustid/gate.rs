//! Caps how many files are fingerprinted at once (Picard's `fpcalc_threads`).
//!
//! The job pool is shared with network work, so fingerprinting must not take every worker: each
//! job holds a [`Permit`] while it decodes, and waits when `max` are already running.

use std::sync::{Condvar, Mutex, PoisonError};

static RUNNING: Mutex<usize> = Mutex::new(0);
static CHANGED: Condvar = Condvar::new();

/// One running fingerprint calculation; released on drop.
#[derive(Debug)]
pub struct Permit(());

/// Blocks until fewer than `max` (at least 1) permits are held, then takes one.
pub fn acquire(max: usize) -> Permit {
    let max = max.max(1);
    let mut running = RUNNING.lock().unwrap_or_else(PoisonError::into_inner);
    while *running >= max {
        running = CHANGED
            .wait(running)
            .unwrap_or_else(PoisonError::into_inner);
    }
    *running += 1;
    Permit(())
}

impl Drop for Permit {
    fn drop(&mut self) {
        let mut running = RUNNING.lock().unwrap_or_else(PoisonError::into_inner);
        *running = running.saturating_sub(1);
        CHANGED.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;
    use std::time::Duration;

    #[test]
    fn never_exceeds_the_limit() {
        static ACTIVE: AtomicUsize = AtomicUsize::new(0);
        static PEAK: AtomicUsize = AtomicUsize::new(0);
        let handles: Vec<_> = (0..6)
            .map(|_| {
                thread::spawn(|| {
                    let _permit = super::acquire(2);
                    let now = ACTIVE.fetch_add(1, Ordering::SeqCst) + 1;
                    PEAK.fetch_max(now, Ordering::SeqCst);
                    thread::sleep(Duration::from_millis(20));
                    ACTIVE.fetch_sub(1, Ordering::SeqCst);
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        assert!(PEAK.load(Ordering::SeqCst) <= 2);
    }
}
