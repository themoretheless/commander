//! Shared poison-tolerant mutex access for UI/background-thread boundaries.

use std::sync::{Mutex, MutexGuard};

/// Keep the application usable after a worker panics while holding a mutex.
/// The protected value may be partially updated, so callers still validate
/// their own domain invariants after acquiring the recovered guard.
pub fn recover<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn recovers_the_value_from_a_poisoned_mutex() {
        let value = Arc::new(Mutex::new(0));
        let worker_value = Arc::clone(&value);
        let _ = std::thread::spawn(move || {
            let mut guard = worker_value.lock().unwrap();
            *guard = 7;
            panic!("poison the test mutex");
        })
        .join();

        assert_eq!(*recover(&value), 7);
    }
}
