//! Small pure rules for the one-shot focus mode.

const ARM_DELAY_SECS: f64 = 0.20;
const POINTER_DELTA_SQ: f32 = 0.25;

pub fn should_exit(started_at: f64, now: f64, pointer_delta_sq: f32, escape_pressed: bool) -> bool {
    escape_pressed || (now - started_at >= ARM_DELAY_SECS && pointer_delta_sq > POINTER_DELTA_SQ)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_always_exits_focus_mode() {
        assert!(should_exit(10.0, 10.01, 0.0, true));
    }

    #[test]
    fn pointer_move_waits_until_mode_is_armed() {
        assert!(!should_exit(10.0, 10.05, 4.0, false));
        assert!(should_exit(10.0, 10.25, 4.0, false));
    }

    #[test]
    fn tiny_pointer_noise_does_not_exit() {
        assert!(!should_exit(10.0, 11.0, 0.01, false));
    }
}
