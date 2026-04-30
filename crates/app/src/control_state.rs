//! Runtime control state used by both host and guest (host owns the source
//! of truth; guest mirrors what the host most recently confirmed).

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
