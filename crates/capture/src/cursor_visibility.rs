//! Cloneable flag that controls whether the system cursor is rendered into
//! captured frames. Toggling at runtime requires the capture thread to
//! observe the flag and (on platforms without a live API) tear down + rebuild
//! the underlying capturer. The capture thread checks the flag on a debounce
//! tick rather than every frame.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

/// A cheaply cloneable atomic boolean that signals whether the system cursor
/// should appear in captured frames.
///
/// All clones share the same underlying `AtomicBool` so any writer (e.g. the
/// control-state task) is immediately visible to the capture thread reader.
#[derive(Clone, Debug)]
pub struct ShowCursorFlag(Arc<AtomicBool>);

impl ShowCursorFlag {
    /// Create a new flag with the given initial value.
    #[must_use]
    pub fn new(initial: bool) -> Self {
        Self(Arc::new(AtomicBool::new(initial)))
    }

    /// Set the flag value. All clones will observe the change.
    ///
    /// `Relaxed` ordering is intentional: a stale read on the capture thread
    /// only delays the cursor toggle by at most one debounce tick (~33–500 ms
    /// depending on capture FPS), which is invisible to the user.
    pub fn set(&self, v: bool) {
        self.0.store(v, Ordering::Relaxed);
    }

    /// Read the current flag value.
    #[must_use]
    pub fn get(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flag_default_is_visible() {
        let flag = ShowCursorFlag::new(true);
        assert!(flag.get());
    }

    #[test]
    fn flag_round_trips() {
        let flag = ShowCursorFlag::new(true);
        flag.set(false);
        assert!(!flag.get());
        flag.set(true);
        assert!(flag.get());
    }

    #[test]
    fn arc_clones_share_state() {
        let flag = ShowCursorFlag::new(true);
        let cloned = flag.clone();
        flag.set(false);
        assert!(!cloned.get());
    }
}
