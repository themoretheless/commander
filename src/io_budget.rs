//! Cooperative I/O priority: background workers yield briefly after UI input.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

const FOREGROUND_RESERVATION: Duration = Duration::from_millis(180);
const BACKGROUND_YIELD: Duration = Duration::from_millis(3);

static EPOCH: OnceLock<Instant> = OnceLock::new();
static FOREGROUND_UNTIL_MS: AtomicU64 = AtomicU64::new(0);

fn now_ms() -> u64 {
    EPOCH
        .get_or_init(Instant::now)
        .elapsed()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

pub fn note_foreground_activity() {
    let deadline = now_ms().saturating_add(FOREGROUND_RESERVATION.as_millis() as u64);
    FOREGROUND_UNTIL_MS.fetch_max(deadline, Ordering::Release);
}

pub fn foreground_reserved() -> bool {
    now_ms() < FOREGROUND_UNTIL_MS.load(Ordering::Acquire)
}

/// Return `false` when the caller should stop. Otherwise yield one bounded
/// slice if foreground I/O was recently requested, avoiding starvation while
/// still leaving most of the disk budget to listing and preview work.
pub fn background_checkpoint(cancelled: impl FnOnce() -> bool) -> bool {
    if cancelled() {
        return false;
    }
    if foreground_reserved() {
        std::thread::sleep(BACKGROUND_YIELD);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancellation_wins_without_waiting() {
        note_foreground_activity();
        let started = Instant::now();
        assert!(!background_checkpoint(|| true));
        assert!(started.elapsed() < BACKGROUND_YIELD);
    }

    #[test]
    fn foreground_activity_extends_the_reservation() {
        note_foreground_activity();
        assert!(foreground_reserved());
        assert!(background_checkpoint(|| false));
    }
}
