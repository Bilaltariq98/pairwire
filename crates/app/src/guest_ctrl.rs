//! Guest-side control-state machine: observes inbound `ControlGrant` /
//! `ControlRevoke` from the host and updates the local
//! [`RuntimeControlState`](crate::control_state::RuntimeControlState) watch so
//! the viewer can reflect the current control state without polling.

/// Guest control-state machine loop.
///
/// Mirrors `ControlGrant` / `ControlRevoke` messages sent by the host and
/// forwards the new [`RuntimeControlState`](crate::control_state::RuntimeControlState)
/// to a watch channel so the viewer can reflect the current control state
/// without polling.
///
/// Exits when the inbound broadcast channel closes (i.e., after session teardown).
///
/// Extracted as a free function so tests can drive it without a full session.
pub(crate) async fn guest_ctrl_state_task_fn(
    mut inbound: tokio::sync::broadcast::Receiver<pairwire_protocol::ControlMessage>,
    state_tx: tokio::sync::watch::Sender<crate::control_state::RuntimeControlState>,
) {
    use crate::control_state::{ControlAction, RuntimeControlState};
    use pairwire_protocol::ControlMessage;

    let mut state = RuntimeControlState::HostActive;
    loop {
        match inbound.recv().await {
            Ok(ControlMessage::ControlGrant(_)) => {
                state.apply(ControlAction::Grant);
                let _ = state_tx.send(state);
                tracing::debug!("control granted by host");
            }
            Ok(ControlMessage::ControlRevoke(_)) => {
                state.apply(ControlAction::Revoke);
                let _ = state_tx.send(state);
                tracing::debug!("control revoked by host");
            }
            Ok(_) => {} // other messages not relevant to guest control state
            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                tracing::debug!(n, "guest ctrl inbound lagged — continuing");
                continue;
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::guest_ctrl_state_task_fn;

    /// Guest state machine: ControlGrant → GuestActive (watch emits); ControlRevoke → HostActive.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn guest_ctrl_state_task_grant_then_revoke() {
        use crate::control_state::RuntimeControlState;
        use pairwire_protocol::{ControlGrant, ControlMessage, ControlRevoke};

        let (bcast_tx, inbound_rx) = tokio::sync::broadcast::channel::<ControlMessage>(8);
        let (state_tx, state_rx) = tokio::sync::watch::channel(RuntimeControlState::HostActive);

        // Pre-load Grant then Revoke; drop the broadcast sender so the task exits
        // when it drains the channel and then receives the Closed error.
        let _ = bcast_tx.send(ControlMessage::ControlGrant(ControlGrant { peer_id: 1 }));
        let _ = bcast_tx.send(ControlMessage::ControlRevoke(ControlRevoke { peer_id: 1 }));
        drop(bcast_tx);

        guest_ctrl_state_task_fn(inbound_rx, state_tx).await;

        assert_eq!(
            *state_rx.borrow(),
            RuntimeControlState::HostActive,
            "state should return to HostActive after revoke"
        );

        // Separately verify the intermediate GuestActive transition via a fresh run.
        let (bcast_tx2, inbound_rx2) = tokio::sync::broadcast::channel::<ControlMessage>(8);
        let (state_tx2, state_rx2) = tokio::sync::watch::channel(RuntimeControlState::HostActive);
        let _ = bcast_tx2.send(ControlMessage::ControlGrant(ControlGrant { peer_id: 1 }));
        drop(bcast_tx2);
        guest_ctrl_state_task_fn(inbound_rx2, state_tx2).await;
        assert_eq!(
            *state_rx2.borrow(),
            RuntimeControlState::GuestActive,
            "state should be GuestActive after lone Grant"
        );
    }
}
