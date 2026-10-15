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
    /// Queue-assigned identity used by egui. Zero means not queued yet.
    id: u64,
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
        Self {
            id: 0,
            message: message.into(),
            kind,
            undoable,
            born: now,
            ttl: ttl_for(kind),
        }
    }

    pub const fn id(&self) -> u64 {
        self.id
    }
}

/// Default visible lifetime, in seconds.
pub const DEFAULT_TTL: f64 = 6.0;
/// Errors stay twice as long: many (run command, archive, find) have no
/// other surface, so the toast is the only place to read them.
pub const ERROR_TTL: f64 = 12.0;

/// Visible lifetime for a toast of `kind`.
pub const fn ttl_for(kind: ToastKind) -> f64 {
    match kind {
        ToastKind::Error => ERROR_TTL,
        ToastKind::Success | ToastKind::Info => DEFAULT_TTL,
    }
}
/// Most toasts shown at once; older ones drop off.
pub const MAX_TOASTS: usize = 3;

pub struct ToastQueue {
    toasts: Vec<Toast>,
    next_id: u64,
}

impl Default for ToastQueue {
    fn default() -> Self {
        Self {
            toasts: Vec::new(),
            next_id: 1,
        }
    }
}

impl ToastQueue {
    /// Push a toast. If any visible toast carries the same message, it is
    /// refreshed (timer, kind, Undo) and moved to the newest position under
    /// its existing id rather than stacking a duplicate. Otherwise append,
    /// capping the queue at [`MAX_TOASTS`] by dropping the oldest toast that
    /// is not an error (errors go last, oldest first). The toast just pushed
    /// is never the one evicted: the user's latest feedback always shows.
    pub fn push(&mut self, mut toast: Toast) {
        if let Some(index) = self.toasts.iter().position(|t| t.message == toast.message) {
            let mut existing = self.toasts.remove(index);
            existing.born = toast.born;
            existing.ttl = toast.ttl;
            existing.kind = toast.kind;
            existing.undoable = toast.undoable;
            self.toasts.push(existing);
            return;
        }
        toast.id = self.next_id;
        self.next_id = self
            .next_id
            .checked_add(1)
            .expect("toast ID space exhausted");
        self.toasts.push(toast);
        while self.toasts.len() > MAX_TOASTS {
            let older = self.toasts.len() - 1;
            let evict = self.toasts[..older]
                .iter()
                .position(|t| t.kind != ToastKind::Error)
                .unwrap_or(0);
            self.toasts.remove(evict);
        }
    }

    /// Drop toasts that have expired by `now`.
    pub fn prune(&mut self, now: f64) {
        self.toasts.retain(|t| t.born + t.ttl >= now);
    }

    /// Restart every toast's lifetime at `now`. Called while the stack cannot
    /// be placed without covering focus or an error, so toasts wait to be
    /// seen instead of expiring hidden.
    pub fn hold(&mut self, now: f64) {
        for toast in &mut self.toasts {
            toast.born = now;
        }
    }

    /// Drop every toast that offered an Undo (called once that Undo is spent).
    pub fn dismiss_undoable(&mut self) {
        self.toasts.retain(|t| !t.undoable);
    }

    pub fn active(&self) -> &[Toast] {
        &self.toasts
    }

    pub const fn is_empty(&self) -> bool {
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
        let id = q.active()[0].id();
        q.push(toast("Moved 8 items", 4.0)); // same message later -> reset
        assert_eq!(q.active().len(), 1);
        assert_eq!(q.active()[0].id(), id);
        assert_eq!(q.active()[0].born, 4.0);
        // A different message stacks.
        q.push(toast("Renamed 2 items", 5.0));
        assert_eq!(q.active().len(), 2);
    }

    #[test]
    fn repeated_message_refreshes_its_earlier_toast() {
        let mut q = ToastQueue::default();
        q.push(toast("A", 0.0));
        let id = q.active()[0].id();
        q.push(toast("B", 1.0));
        q.push(toast("A", 2.0));
        let msgs: Vec<&str> = q.active().iter().map(|t| t.message.as_str()).collect();
        assert_eq!(msgs, vec!["B", "A"]);
        assert_eq!(q.active()[1].id(), id);
        assert_eq!(q.active()[1].born, 2.0);
    }

    #[test]
    fn errors_outlive_successes() {
        let error = Toast::new("Failed", ToastKind::Error, false, 0.0);
        assert_eq!(error.ttl, ERROR_TTL);
        const { assert!(ERROR_TTL > DEFAULT_TTL) };
        assert_eq!(toast("Done", 0.0).ttl, DEFAULT_TTL);
        assert_eq!(
            Toast::new("Hint", ToastKind::Info, false, 0.0).ttl,
            DEFAULT_TTL
        );
    }

    #[test]
    fn cap_evicts_the_oldest_non_error_first() {
        let mut q = ToastQueue::default();
        q.push(Toast::new("err", ToastKind::Error, false, 0.0));
        for (i, m) in ["b", "c", "d"].iter().enumerate() {
            q.push(toast(m, 1.0 + i as f64));
        }
        let msgs: Vec<&str> = q.active().iter().map(|t| t.message.as_str()).collect();
        assert_eq!(msgs, vec!["err", "c", "d"]);
        // With only errors left, the oldest still goes.
        let mut q = ToastQueue::default();
        for m in ["e1", "e2", "e3", "e4"] {
            q.push(Toast::new(m, ToastKind::Error, false, 0.0));
        }
        assert_eq!(q.active()[0].message, "e2");
    }

    #[test]
    fn a_new_success_is_shown_even_when_errors_fill_the_stack() {
        let mut q = ToastQueue::default();
        for m in ["e1", "e2", "e3"] {
            q.push(Toast::new(m, ToastKind::Error, false, 0.0));
        }
        q.push(Toast::new("Moved 2 items", ToastKind::Success, true, 1.0));
        let msgs: Vec<&str> = q.active().iter().map(|t| t.message.as_str()).collect();
        assert_eq!(msgs, vec!["e2", "e3", "Moved 2 items"]);
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
    fn held_toasts_do_not_expire_while_hidden() {
        let mut q = ToastQueue::default();
        q.push(toast("error", 0.0)); // ttl 6 -> would expire at 6
        q.hold(5.0);
        q.hold(10.0);
        q.prune(15.0);
        assert_eq!(q.active().len(), 1);
        assert_eq!(remaining_fraction(&q.active()[0], 10.0), 1.0);
        q.prune(16.5);
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

    #[test]
    fn ids_are_unique_and_stable_when_older_toasts_are_pruned() {
        let mut q = ToastQueue::default();
        let mut short = toast("short", 0.0);
        short.ttl = 1.0;
        q.push(short);
        q.push(toast("persistent", 0.5));
        let persistent_id = q.active()[1].id();

        q.prune(2.0);
        assert_eq!(q.active()[0].id(), persistent_id);

        q.push(toast("new", 2.0));
        assert_ne!(q.active()[0].id(), q.active()[1].id());
        assert_eq!(q.active()[0].id(), persistent_id);
    }
}
