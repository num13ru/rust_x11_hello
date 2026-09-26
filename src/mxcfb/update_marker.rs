//! Process-local, nonzero HWTCON update markers.
//! This does not reserve markers with the kernel or submit an update.

/// The first marker is the process ID (or 1 if the seed is zero).
/// Later markers increment, skipping zero after `u32::MAX`.
/// Values are distinct only within one sequence before a full wrap. Other
/// sequences, including ones in the same process, can issue the same marker.
pub(super) struct UpdateMarkerSequence {
    next: u32,
}

impl UpdateMarkerSequence {
    pub(super) fn for_process() -> Self {
        Self::from_seed(std::process::id())
    }

    fn from_seed(seed: u32) -> Self {
        Self { next: seed.max(1) }
    }

    pub(super) fn next_marker(&mut self) -> u32 {
        let marker = self.next;
        self.next = self.next.wrapping_add(1).max(1);
        marker
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_at_seed_and_advances() {
        let mut markers = UpdateMarkerSequence::from_seed(42);
        assert_eq!(markers.next_marker(), 42);
        assert_eq!(markers.next_marker(), 43);
    }

    #[test]
    fn zero_seed_never_issues_zero() {
        let mut markers = UpdateMarkerSequence::from_seed(0);
        assert_eq!(markers.next_marker(), 1);
        assert_eq!(markers.next_marker(), 2);
    }

    #[test]
    fn wrap_skips_zero() {
        let mut markers = UpdateMarkerSequence::from_seed(u32::MAX);
        assert_eq!(markers.next_marker(), u32::MAX);
        assert_eq!(markers.next_marker(), 1);
        assert_eq!(markers.next_marker(), 2);
    }

    #[test]
    fn process_seed_is_nonzero() {
        let mut markers = UpdateMarkerSequence::for_process();
        assert_ne!(markers.next_marker(), 0);
    }
}
