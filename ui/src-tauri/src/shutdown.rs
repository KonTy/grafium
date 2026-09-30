use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Default)]
pub(crate) struct ShutdownGate(AtomicU8);

impl ShutdownGate {
    pub fn begin(&self) -> bool {
        self.0
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    pub fn complete(&self) {
        self.0.store(2, Ordering::Release);
    }

    pub fn is_complete(&self) -> bool {
        self.0.load(Ordering::Acquire) == 2
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_close_and_exit_wait_for_one_completed_shutdown() {
        let gate = ShutdownGate::default();
        assert!(!gate.is_complete());
        assert!(gate.begin());
        assert!(!gate.begin());
        assert!(!gate.is_complete());
        gate.complete();
        assert!(gate.is_complete());
        assert!(!gate.begin());
        gate.complete();
        assert!(gate.is_complete());
    }
}
