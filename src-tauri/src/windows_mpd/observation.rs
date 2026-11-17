//! A serial for already existing worker replies, never a media/queue authority.
use super::protocol::Result;
use std::cell::Cell;
#[derive(Default)]
pub(super) struct Clock(Cell<u64>);
impl Clock {
    pub(super) fn advance(&self, shutdown: bool) -> Result<()> {
        match self.0.get().checked_add(1) {
            Some(value) => {
                self.0.set(value);
                Ok(())
            }
            // Even exhaustion must not prevent exact owned shutdown/cleanup.
            None if shutdown => Ok(()),
            None => Err("worker_failed"),
        }
    }
    pub(super) fn current(&self) -> String {
        self.0.get().to_string()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn replies_keep_worker_order_and_use_lossless_decimal_strings() {
        let clock = Clock::default();
        clock.advance(false).unwrap();
        assert_eq!(clock.current(), "1");
        clock.advance(false).unwrap();
        assert_eq!(clock.current(), "2");
        let clock = Clock(Cell::new(9_007_199_254_740_991));
        clock.advance(false).unwrap();
        assert_eq!(clock.current(), "9007199254740992");
        clock.advance(false).unwrap();
        assert_eq!(clock.current(), "9007199254740993");
    }
    #[test]
    fn exhausted_counter_refuses_further_operations_but_preserves_shutdown() {
        let clock = Clock(Cell::new(u64::MAX - 1));
        clock.advance(false).unwrap();
        assert_eq!(clock.current(), u64::MAX.to_string());
        assert_eq!(clock.advance(false), Err("worker_failed"));
        assert_eq!(clock.current(), u64::MAX.to_string());
        assert_eq!(clock.advance(true), Ok(()));
        assert_eq!(clock.current(), u64::MAX.to_string());
    }
}
