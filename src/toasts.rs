//! Pure, time-driven toast queue. All timing is injected (`now` in seconds),
//! so coalescing, the cap, pruning and the countdown are unit-tested with a
//! synthetic clock. No egui types here.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ToastKind {
    Success,
    Error,
    /// Neutral notice (an instruction or a "nothing to do" result), styled
    /// muted rather than the success green so it does not read as a win.
    Info,
}

#[derive(Clone, PartialEq, Debug)]
pub struct Toast {
    pub message: String,
    pub kind: ToastKind,
    /// Whether the toast offers an inline Undo (the op is on the undo stack).
    pub undoable: bool,
    /// Birth time and lifetime, both in seconds on the injected clock.
    pub born: f64,
    pub ttl: f64,
}

impl Toast {
    pub fn new(message: impl Into<String>, kind: ToastKind, undoable: bool, now: f64) -> Self {
        Toast {
            message: message.into(),
            kind,
            undoable,
            born: now,
            ttl: DEFAULT_TTL,
        }
    }
}

/// Default visible lifetime, in seconds.
pub const DEFAULT_TTL: f64 = 6.0;
/// Most toasts shown at once; older ones drop off.
pub const MAX_TOASTS: usize = 3;

#[derive(Default)]
pub struct ToastQueue {
    toasts: Vec<Toast>,
}

impl ToastQueue {
    /// Push a toast. If the newest existing toast carries the same message,
    /// reset its timer (coalesce) rather than stacking a duplicate. Otherwise
    /// append, capping the queue at [`MAX_TOASTS`] by dropping the oldest.
    pub fn push(&mut self, toast: Toast) {
        if let Some(last) = self.toasts.last_mut()
            && last.message == toast.message
        {
            last.born = toast.born;
            last.ttl = toast.ttl;
            last.kind = toast.kind;
            last.undoable = toast.undoable;
            return;
        }
        self.toasts.push(toast);
        while self.toasts.len() > MAX_TOASTS {
            self.toasts.remove(0);
        }
    }

    /// Drop toasts that have expired by `now`.
    pub fn prune(&mut self, now: f64) {
        self.toasts.retain(|t| t.born + t.ttl >= now);
    }

    /// Drop every toast that offered an Undo (called once that Undo is spent).
    pub fn dismiss_undoable(&mut self) {
        self.toasts.retain(|t| !t.undoable);
    }

    pub fn active(&self) -> &[Toast] {
        &self.toasts
    }

    pub fn is_empty(&self) -> bool {
        self.toasts.is_empty()
    }
}

/// Fraction of `toast`'s lifetime remaining at `now`: 1.0 at birth down to 0.0
/// at expiry, clamped to `[0, 1]`.
pub fn remaining_fraction(toast: &Toast, now: f64) -> f32 {
    if toast.ttl <= 0.0 {
        return 0.0;
    }
    let frac = 1.0 - (now - toast.born) / toast.ttl;
    frac.clamp(0.0, 1.0) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toast(msg: &str, now: f64) -> Toast {
        Toast::new(msg, ToastKind::Success, false, now)
    }

    #[test]
    fn identical_consecutive_message_coalesces_and_resets_timer() {
        let mut q = ToastQueue::default();
        q.push(toast("Moved 8 items", 0.0));
        q.push(toast("Moved 8 items", 4.0)); // same message later -> reset
        assert_eq!(q.active().len(), 1);
        assert_eq!(q.active()[0].born, 4.0);
        // A different message stacks.
        q.push(toast("Renamed 2 items", 5.0));
        assert_eq!(q.active().len(), 2);
    }

    #[test]
    fn queue_caps_at_three_dropping_oldest() {
        let mut q = ToastQueue::default();
        for (i, m) in ["a", "b", "c", "d"].iter().enumerate() {
            q.push(toast(m, i as f64));
        }
        let msgs: Vec<&str> = q.active().iter().map(|t| t.message.as_str()).collect();
        assert_eq!(msgs, vec!["b", "c", "d"]); // "a" dropped
    }

    #[test]
    fn prune_drops_expired_by_the_injected_clock() {
        let mut q = ToastQueue::default();
        q.push(toast("x", 0.0)); // ttl 6 -> expires at 6
        q.prune(5.0);
        assert_eq!(q.active().len(), 1);
        q.prune(6.5);
        assert!(q.is_empty());
    }

    #[test]
    fn remaining_fraction_runs_one_to_zero() {
        let t = toast("x", 10.0); // ttl 6
        assert!((remaining_fraction(&t, 10.0) - 1.0).abs() < 1e-6);
        assert!((remaining_fraction(&t, 13.0) - 0.5).abs() < 1e-6);
        assert_eq!(remaining_fraction(&t, 16.0), 0.0);
        assert_eq!(remaining_fraction(&t, 99.0), 0.0); // clamped past expiry
    }

    #[test]
    fn dismiss_undoable_keeps_only_plain_toasts() {
        let mut q = ToastQueue::default();
        q.push(Toast::new("Moved 3 items", ToastKind::Success, true, 0.0));
        q.push(Toast::new("Copied 1 item", ToastKind::Success, false, 0.0));
        q.dismiss_undoable();
        let msgs: Vec<&str> = q.active().iter().map(|t| t.message.as_str()).collect();
        assert_eq!(msgs, vec!["Copied 1 item"]);
    }
}
