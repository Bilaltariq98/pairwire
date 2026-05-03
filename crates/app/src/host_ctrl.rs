//! Host-side control-state machine: consumes inbound `ControlMessage`s and tray
//! `ControlAction`s, drives `InputReceiverHandle` permission and capture cursor
//! visibility, sends `ControlGrant`/`ControlRevoke` echoes to the guest.

use std::sync::Arc;

/// Update the `AtomicPermission` to match the current control state.
///
/// Called whenever the host control state transitions so that the input
/// receiver immediately reflects the new gating without a lock.
pub(crate) fn apply_permission(
    perm: &Option<Arc<pairwire_input::AtomicPermission>>,
    state: crate::control_state::RuntimeControlState,
) {
    if let Some(p) = perm {
        p.set(state.input_permission());
    }
}

/// Host control-state machine loop.
///
/// Drives `RuntimeControlState` from inbound `ControlMessage`s (guest-initiated)
/// and tray `ControlAction`s (host-initiated). Updates `AtomicPermission` in-place
/// and sends `ControlGrant` / `ControlRevoke` back to the guest.
///
/// Exits when both the inbound broadcast channel and the tray action channel
/// are closed (i.e., after session teardown).
///
/// Extracted as a free function so tests can drive it without a full session.
pub(crate) async fn host_ctrl_state_task_fn(
    auto_grant: bool,
    inbound: &mut tokio::sync::broadcast::Receiver<pairwire_protocol::ControlMessage>,
    tray_rx: &mut tokio::sync::mpsc::Receiver<crate::control_state::ControlAction>,
    outbound_tx: &tokio::sync::mpsc::Sender<pairwire_protocol::ControlMessage>,
    perm_handle: &Option<Arc<pairwire_input::AtomicPermission>>,
    show_cursor: pairwire_capture::ShowCursorFlag,
) {
    use crate::control_state::{ControlAction, RuntimeControlState};
    use pairwire_protocol::{ControlGrant, ControlMessage, ControlRevoke};

    let mut state = RuntimeControlState::HostActive;
    // Track whether each source has been permanently closed.
    // The task exits only when the inbound control channel closes;
    // the tray channel is optional and its closure is not a terminal condition.
    let mut tray_closed = false;
    // Remember the most-recent requesting peer so tray-initiated Grant/Revoke
    // can address the right guest. Falls back to 1 for unsolicited grants where
    // the host clicks "Allow" before the guest has sent a ControlRequest.
    let mut pending_peer_id: Option<u16> = None;

    loop {
        tokio::select! {
            msg = inbound.recv() => {
                match msg {
                    Ok(ControlMessage::ControlRequest(req)) => {
                        state.apply(ControlAction::Request);
                        pending_peer_id = Some(req.peer_id);
                        tracing::info!(peer_id = req.peer_id, "guest requested control");
                        if auto_grant {
                            state.apply(ControlAction::Grant);
                            apply_permission(perm_handle, state);
                            // Hide the host cursor so the guest's overlay arrow is
                            // the only cursor visible in the streamed frames.
                            show_cursor.set(false);
                            let _ = outbound_tx
                                .send(ControlMessage::ControlGrant(ControlGrant {
                                    peer_id: req.peer_id,
                                }))
                                .await;
                            tracing::info!("auto-granted control to guest");
                        }
                    }
                    Ok(_) => {} // other control messages not handled by host state machine
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::debug!(n, "host ctrl inbound lagged — continuing");
                        continue;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
            action = tray_rx.recv(), if !tray_closed => {
                match action {
                    Some(action) => {
                        // Use the most-recently seen requester; fall back to 1 for
                        // single-guest sessions where the host grants before any request.
                        let peer_id = pending_peer_id.unwrap_or(1);
                        state.apply(action);
                        apply_permission(perm_handle, state);
                        match action {
                            ControlAction::Grant => {
                                // Hide the host cursor while the guest has control.
                                show_cursor.set(false);
                                let _ = outbound_tx
                                    .send(ControlMessage::ControlGrant(ControlGrant {
                                        peer_id,
                                    }))
                                    .await;
                                tracing::info!("host granted control via tray");
                            }
                            ControlAction::Revoke => {
                                // Restore the host cursor now that the guest no longer
                                // has control.
                                show_cursor.set(true);
                                let _ = outbound_tx
                                    .send(ControlMessage::ControlRevoke(ControlRevoke {
                                        peer_id,
                                    }))
                                    .await;
                                tracing::info!("host revoked control via tray");
                            }
                            ControlAction::Request => {} // host never requests
                        }
                    }
                    None => {
                        // Tray sender dropped — disable this arm, continue on inbound.
                        tray_closed = true;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{apply_permission, host_ctrl_state_task_fn};

    /// Integration test: when auto_grant=true, a ControlRequest from the guest side
    /// causes the host state machine to emit ControlGrant, flip AtomicPermission
    /// to FullControl, and set the ShowCursorFlag to false — driven purely
    /// in-process via broadcast channel stubs.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn host_auto_grant_on_control_request() {
        use std::sync::Arc;

        use pairwire_capture::ShowCursorFlag;
        use pairwire_input::AtomicPermission;
        use pairwire_protocol::{ControlGrant, ControlMessage, ControlRequest};
        use tokio::sync::mpsc;

        let (host_ctrl_outbound_tx, host_ctrl_outbound_rx) = mpsc::channel::<ControlMessage>(8);

        // perm handle: what we'll inspect after the task runs
        let atomic_perm = Arc::new(AtomicPermission::default());
        let perm_handle = Some(Arc::clone(&atomic_perm));

        // cursor visibility flag: starts visible, should flip to hidden on grant
        let show_cursor = ShowCursorFlag::new(true);
        let show_cursor_check = show_cursor.clone();

        let auto_grant = true;
        let (tray_action_tx, mut tray_action_rx) =
            mpsc::channel::<crate::control_state::ControlAction>(8);
        drop(tray_action_tx); // no tray actions in this test

        // Simulate the guest sending a ControlRequest to the host inbound.
        // Pre-load the ControlRequest then drop the sender so the task exits
        // when the channel becomes empty and closed.
        let (bcast_tx, mut host_inbound_rx) = tokio::sync::broadcast::channel::<ControlMessage>(8);
        let _ = bcast_tx.send(ControlMessage::ControlRequest(ControlRequest {
            peer_id: 1,
        }));
        drop(bcast_tx);

        host_ctrl_state_task_fn(
            auto_grant,
            &mut host_inbound_rx,
            &mut tray_action_rx,
            &host_ctrl_outbound_tx,
            &perm_handle,
            show_cursor,
        )
        .await;

        // The permission should have been flipped to FullControl.
        assert_eq!(
            atomic_perm.load_permission(),
            pairwire_input::InputPermission::FullControl,
            "permission should be FullControl after auto-grant"
        );

        // The cursor should have been hidden when control was granted.
        assert!(
            !show_cursor_check.get(),
            "show_cursor flag should be false after guest is granted control"
        );

        // The host should have sent a ControlGrant on the outbound channel.
        drop(host_ctrl_outbound_tx);
        let mut received_grant = false;
        let mut rx = host_ctrl_outbound_rx;
        while let Some(msg) = rx.recv().await {
            if matches!(
                msg,
                ControlMessage::ControlGrant(ControlGrant { peer_id: 1 })
            ) {
                received_grant = true;
            }
        }
        assert!(received_grant, "host should have sent ControlGrant");
    }

    #[test]
    fn apply_permission_sets_full_control() {
        use std::sync::Arc;

        use pairwire_input::{AtomicPermission, InputPermission};

        use crate::control_state::RuntimeControlState;

        let perm = Arc::new(AtomicPermission::default());
        let handle = Some(Arc::clone(&perm));
        apply_permission(&handle, RuntimeControlState::GuestActive);
        assert_eq!(perm.load_permission(), InputPermission::FullControl);
    }

    #[test]
    fn apply_permission_noop_when_none() {
        // Should not panic when no permission handle is provided.
        use crate::control_state::RuntimeControlState;
        apply_permission(&None, RuntimeControlState::GuestActive);
    }
}
