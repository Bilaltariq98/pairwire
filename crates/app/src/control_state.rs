//! Runtime control state used by both host and guest (host owns the source
//! of truth; guest mirrors what the host most recently confirmed).

use pairwire_input::InputPermission;

/// Tracks who currently owns keyboard and mouse input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeControlState {
    /// Default. Host owns the keyboard/mouse; input from guest is dropped.
    HostActive,
    /// Guest has sent a `ControlRequest`; host has not yet decided.
    GuestRequested,
    /// Guest's input is forwarded to the host OS.
    GuestActive,
}

/// Actions that drive transitions between control states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlAction {
    /// Guest requests control (moves host to `GuestRequested`).
    Request,
    /// Host grants control (moves to `GuestActive`).
    Grant,
    /// Host revokes control (returns to `HostActive`).
    Revoke,
}

impl RuntimeControlState {
    /// Apply a control action, transitioning to the next state in-place.
    ///
    /// Invalid / no-op transitions leave the state unchanged.
    pub fn apply(&mut self, action: ControlAction) {
        *self = match (*self, action) {
            (Self::HostActive, ControlAction::Request) => Self::GuestRequested,
            (Self::HostActive | Self::GuestRequested, ControlAction::Grant) => Self::GuestActive,
            (Self::GuestActive | Self::GuestRequested, ControlAction::Revoke) => Self::HostActive,
            // All other combinations are no-ops.
            (state, _) => state,
        };
    }

    /// Returns the input permission corresponding to the current state.
    ///
    /// Only `GuestActive` yields `FullControl`; all other states yield `Denied`.
    #[must_use]
    pub fn input_permission(self) -> InputPermission {
        match self {
            Self::GuestActive => InputPermission::FullControl,
            _ => InputPermission::Denied,
        }
    }
}

impl Default for RuntimeControlState {
    fn default() -> Self {
        Self::HostActive
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_active_to_guest_active_via_grant() {
        let mut state = RuntimeControlState::HostActive;
        state.apply(ControlAction::Grant);
        assert_eq!(state, RuntimeControlState::GuestActive);
    }

    #[test]
    fn guest_active_back_to_host_via_revoke() {
        let mut state = RuntimeControlState::GuestActive;
        state.apply(ControlAction::Revoke);
        assert_eq!(state, RuntimeControlState::HostActive);
    }

    #[test]
    fn request_moves_host_to_guest_requested() {
        let mut state = RuntimeControlState::HostActive;
        state.apply(ControlAction::Request);
        assert_eq!(state, RuntimeControlState::GuestRequested);
    }

    #[test]
    fn revoke_when_already_host_is_noop() {
        let mut state = RuntimeControlState::HostActive;
        state.apply(ControlAction::Revoke);
        assert_eq!(state, RuntimeControlState::HostActive);
    }

    #[test]
    fn permission_for_state() {
        assert_eq!(
            RuntimeControlState::HostActive.input_permission(),
            pairwire_input::InputPermission::Denied
        );
        assert_eq!(
            RuntimeControlState::GuestActive.input_permission(),
            pairwire_input::InputPermission::FullControl
        );
        assert_eq!(
            RuntimeControlState::GuestRequested.input_permission(),
            pairwire_input::InputPermission::Denied
        );
    }
}
