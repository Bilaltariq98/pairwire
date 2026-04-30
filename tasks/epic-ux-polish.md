# Epic: UX Polish — Cursor, Control, & Capture Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Polish the existing P2P session surface — wire guest→host cursor, add runtime control grant/revoke, implement the `ControlRequest`/`ControlGrant`/`ControlRevoke` round-trip, and hide the host's system cursor while the guest has control. Outcome: a session that feels like a real tool and proves the core "share + observe + take over" UX end-to-end.

**Architecture:**
- Add a single persistent bidi `ControlMessage` stream (post-handshake) that both peers use for runtime control-plane traffic. The current handshake stream is one-shot and finishes immediately, so we need a new long-lived stream for `ControlRequest`/`ControlGrant`/`ControlRevoke`.
- Guest grows a `CursorSender` symmetric to host's; host's already-spawned `CursorReceiver` becomes the source of truth for guest cursor state (today its watch is discarded — we log it and surface in HUD).
- `InputReceiverHandle::set_permission` already supports runtime toggling — we wire it to (a) tray menu actions and (b) inbound `ControlGrant`/`ControlRevoke` messages. Capture's `show_cursor` flag becomes runtime-toggleable mirroring the granted state.

**Tech Stack:** Rust workspace; iroh QUIC; `postcard` framing; `egui`/`eframe` viewer; `tray-icon`; `scap`; `tokio` async runtime + dedicated OS threads for capture/encode.

---

## File Structure

**New files:**
- `crates/transport/src/control_channel.rs` — `ControlChannel` wrapping a long-lived bidi stream that multiplexes `ControlMessage`s (send/recv halves with mpsc + watch fan-out). ~200 LoC.
- `crates/app/src/control_state.rs` — small enum `RuntimeControlState { HostActive, GuestRequested, GuestActive }` + helpers for transitioning. Also defines a `ControlAction` enum that flows from tray/HUD to the session loop. ~80 LoC.
- `crates/capture/src/cursor_visibility.rs` — `Arc<AtomicBool>` flag the capture thread reads each frame to set `show_cursor` on the next-built `Capturer` (since scap doesn't allow live toggling, we wrap by stop+rebuild on transition). ~120 LoC.

**Modified files:**
- `crates/transport/src/lib.rs` — re-export `ControlChannel`, `ControlChannelHandle`.
- `crates/transport/src/handshake.rs` — leave as-is (still one-shot); `ControlChannel` is opened separately afterwards.
- `crates/app/src/session.rs` — open `ControlChannel` after handshake on both sides; wire guest `CursorSender`; consume host `CursorReceiver` watch into a log/HUD channel; route `ControlAction`s to `InputReceiverHandle::set_permission` and to capture cursor-visibility flag.
- `crates/app/src/tray.rs` — add `Allow Remote Control` and `Revoke Control` menu items; emit `TrayAction::GrantControl` / `TrayAction::RevokeControl`.
- `crates/app/src/main.rs` — bridge tray actions → session control channel.
- `crates/app/src/cli.rs` — keep `--allow-remote-control` (now means: auto-grant on first `ControlRequest`).
- `crates/app/src/viewer.rs` — add HUD line "You have control" / "Awaiting host approval" / "View only" driven by a new `watch::Receiver<RuntimeControlState>`; add a `Request Control` button (or `Cmd+Shift+R`) that emits a `ControlRequest` outbound.
- `crates/capture/src/capture_thread.rs` — accept an `Arc<AtomicBool>` "show cursor" flag; rebuild capturer on toggle.
- `crates/input/src/lib.rs` — already exposes `InputPermission`; no change.

---

## Task 1: ControlChannel — persistent bidi multiplexer

**Files:**
- Create: `crates/transport/src/control_channel.rs`
- Modify: `crates/transport/src/lib.rs`
- Test: `crates/transport/src/control_channel.rs` (inline `#[cfg(test)] mod tests`)

- [ ] **Step 1: Write the failing test for round-tripping a single `ControlRequest`**

```rust
// crates/transport/src/control_channel.rs (inside `#[cfg(test)] mod tests`)
use pairwire_protocol::{ControlMessage, ControlRequest};
use tokio::sync::mpsc;

#[tokio::test]
async fn control_channel_round_trips_request() {
    // Set up a loopback iroh connection (helper already used elsewhere in this crate).
    let (peer_a, peer_b) = crate::test_utils::loopback_pair().await;

    let (a_send, a_recv) = peer_a.open_control_stream().await.unwrap();
    let (b_send, b_recv) = peer_b.accept_bi().await.unwrap().unwrap();

    let (a_outbound_tx, a_outbound_rx) = mpsc::channel(8);
    let a_handle = ControlChannel::spawn(a_send, a_recv, a_outbound_rx);

    let (b_outbound_tx, b_outbound_rx) = mpsc::channel(8);
    let b_handle = ControlChannel::spawn(b_send, b_recv, b_outbound_rx);

    a_outbound_tx
        .send(ControlMessage::ControlRequest(ControlRequest { peer_id: 1 }))
        .await
        .unwrap();

    let mut b_inbound = b_handle.subscribe();
    let msg = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        b_inbound.recv(),
    )
    .await
    .unwrap()
    .unwrap();

    assert!(matches!(
        msg,
        ControlMessage::ControlRequest(ControlRequest { peer_id: 1 })
    ));

    let _ = b_outbound_tx;
    a_handle.stop().await;
    b_handle.stop().await;
}
```

> Note: `crate::test_utils::loopback_pair()` is the existing helper used in `connection.rs` tests; if it lives behind a private path, expose it via `pub(crate)` for the new module.

- [ ] **Step 2: Run test — must fail because `ControlChannel` does not exist**

Run: `cargo test -p pairwire-transport control_channel_round_trips_request -- --nocapture`
Expected: compile error `cannot find type ControlChannel`.

- [ ] **Step 3: Implement `ControlChannel`**

```rust
// crates/transport/src/control_channel.rs
//! Persistent bidi control-message stream.
//!
//! Opened after the one-shot `Handshake` exchange. Carries `ControlMessage`
//! variants — `ControlRequest`, `ControlGrant`, `ControlRevoke`,
//! `ControlHandoff`, `Goodbye`, `SetSharing` — for the lifetime of the session.

use std::sync::Arc;

use iroh::endpoint::{RecvStream, SendStream};
use pairwire_core::{Error, Result};
use pairwire_protocol::ControlMessage;
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use crate::connection::{recv_message, send_message};

const INBOUND_BUFFER: usize = 32;

pub struct ControlChannel;

pub struct ControlChannelHandle {
    inbound_tx: broadcast::Sender<ControlMessage>,
    send_task: JoinHandle<()>,
    recv_task: JoinHandle<()>,
}

impl ControlChannel {
    /// Spawn send + recv tasks for the persistent control stream.
    ///
    /// `outbound_rx` — messages this peer wants to send to the other side.
    /// Use [`ControlChannelHandle::subscribe`] to read inbound messages.
    pub fn spawn(
        mut send: SendStream,
        mut recv: RecvStream,
        mut outbound_rx: mpsc::Receiver<ControlMessage>,
    ) -> ControlChannelHandle {
        let (inbound_tx, _) = broadcast::channel(INBOUND_BUFFER);
        let inbound_tx_for_recv = inbound_tx.clone();

        let send_task = tokio::spawn(async move {
            info!("control channel send task started");
            while let Some(msg) = outbound_rx.recv().await {
                if let Err(e) = send_message(&mut send, &msg).await {
                    warn!(error = %e, "control channel send failed");
                    break;
                }
                debug!(?msg, "control message sent");
            }
            info!("control channel send task stopped");
        });

        let recv_task = tokio::spawn(async move {
            info!("control channel recv task started");
            loop {
                let msg: Result<ControlMessage> = recv_message(&mut recv).await
                    .map_err(|e| Error::Transport(format!("control recv: {e}")));
                match msg {
                    Ok(m) => {
                        debug!(msg = ?m, "control message received");
                        let _ = inbound_tx_for_recv.send(m);
                    }
                    Err(e) => {
                        let s = e.to_string();
                        if s.contains("closed") || s.contains("eof") || s.contains("reset") {
                            break;
                        }
                        warn!(error = %e, "control channel recv error");
                        break;
                    }
                }
            }
            info!("control channel recv task stopped");
        });

        ControlChannelHandle {
            inbound_tx,
            send_task,
            recv_task,
        }
    }
}

impl ControlChannelHandle {
    /// Subscribe to inbound messages. Each subscriber receives every message
    /// produced after the call to `subscribe`.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<ControlMessage> {
        self.inbound_tx.subscribe()
    }

    pub async fn stop(self) {
        self.send_task.abort();
        self.recv_task.abort();
        let _ = self.send_task.await;
        let _ = self.recv_task.await;
    }
}
```

Add to `crates/transport/src/lib.rs`:

```rust
pub mod control_channel;
pub use control_channel::{ControlChannel, ControlChannelHandle};
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p pairwire-transport control_channel_round_trips_request -- --nocapture`
Expected: PASS.

- [ ] **Step 5: Add a second test for graceful shutdown**

```rust
#[tokio::test]
async fn control_channel_stops_cleanly() {
    let (peer_a, peer_b) = crate::test_utils::loopback_pair().await;
    let (a_send, a_recv) = peer_a.open_control_stream().await.unwrap();
    let (b_send, b_recv) = peer_b.accept_bi().await.unwrap().unwrap();

    let (_a_tx, a_rx) = mpsc::channel(8);
    let (_b_tx, b_rx) = mpsc::channel(8);
    let a = ControlChannel::spawn(a_send, a_recv, a_rx);
    let b = ControlChannel::spawn(b_send, b_recv, b_rx);

    // Should complete without hanging.
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        a.stop().await;
        b.stop().await;
    })
    .await
    .unwrap();
}
```

Run: `cargo test -p pairwire-transport control_channel_stops_cleanly`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/transport/src/control_channel.rs crates/transport/src/lib.rs
git commit -m "feat(transport): add persistent ControlChannel for runtime control messages"
```

---

## Task 2: Open ControlChannel from session loop on both sides

**Files:**
- Modify: `crates/app/src/session.rs:108-113` (host post-handshake), `:436-440` (guest post-handshake grace)
- Test: `tests/control_channel_session.rs` (new integration test, host+guest in-process)

- [ ] **Step 1: Write the failing test**

```rust
// tests/control_channel_session.rs
//! End-to-end: host opens session, guest opens session, both end up with a
//! live ControlChannel that survives until the session ends.

use std::time::Duration;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn control_channel_alive_after_handshake() {
    // The test harness should boot a host + guest pair using the existing
    // testing scaffold in tests/, but assert one new thing:
    // after both peers complete handshake, sending a ControlRequest from
    // guest causes the host to receive it within 1s.
    //
    // Use pairwire::testing::HostHarness / GuestHarness if they exist;
    // otherwise drive run_host / run_join in tasks.

    let outcome = pairwire::testing::run_pair_session(|ctx| async move {
        ctx.guest_send_control_request(1).await;
        let received = ctx.host_recv_control_message(Duration::from_secs(1)).await;
        assert!(
            matches!(received, Some(pairwire_protocol::ControlMessage::ControlRequest(_))),
            "host did not receive ControlRequest, got {received:?}"
        );
    })
    .await;

    outcome.expect("session pair completed");
}
```

> If `pairwire::testing::run_pair_session` does not yet exist, this task includes adding a thin harness that wraps the existing patterns from `tests/smoke.rs` (or whichever integration test currently boots a host+guest). Keep it minimal: spawn `run_host`/`run_join`, expose two `mpsc::Sender<ControlMessage>` and a way to await inbound messages on the host's `ControlChannelHandle::subscribe`.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test control_channel_session -- --nocapture`
Expected: FAIL (unresolved import or "host did not receive ControlRequest").

- [ ] **Step 3: Wire `ControlChannel` into `run_host`**

Insert after `perform_host_handshake` returns and before the `accept_bi` chain (around `crates/app/src/session.rs:113`):

```rust
// Persistent control channel for ControlRequest / ControlGrant / ControlRevoke.
let (host_ctrl_outbound_tx, host_ctrl_outbound_rx) =
    tokio::sync::mpsc::channel::<pairwire_protocol::ControlMessage>(32);
let (ctrl_send, ctrl_recv) = conn
    .accept_bi()
    .await
    .context("failed to accept control channel stream")?
    .ok_or_else(|| anyhow::anyhow!("connection closed before control channel"))?;
let host_ctrl_handle =
    pairwire_transport::ControlChannel::spawn(ctrl_send, ctrl_recv, host_ctrl_outbound_rx);
let mut host_ctrl_inbound = host_ctrl_handle.subscribe();
```

In the cleanup section (after `quality_handle.stop().await;`):

```rust
host_ctrl_handle.stop().await;
drop(host_ctrl_outbound_tx);
```

- [ ] **Step 4: Wire `ControlChannel` into `run_join`**

Insert after the 250ms sleep at line 436:

```rust
let (guest_ctrl_outbound_tx, guest_ctrl_outbound_rx) =
    tokio::sync::mpsc::channel::<pairwire_protocol::ControlMessage>(32);
let (ctrl_send, ctrl_recv) = conn
    .open_control_stream()
    .await
    .context("failed to open control channel stream")?;
let guest_ctrl_handle =
    pairwire_transport::ControlChannel::spawn(ctrl_send, ctrl_recv, guest_ctrl_outbound_rx);
let mut guest_ctrl_inbound = guest_ctrl_handle.subscribe();
```

In the cleanup section (after `guest_quality_handle.stop().await;`):

```rust
guest_ctrl_handle.stop().await;
drop(guest_ctrl_outbound_tx);
```

> Stream ordering matters. Currently the order is: host→handshake; guest opens input bidi; guest opens annotation bidi; moq starts. Insert the control channel as the **first** post-handshake bidi (host accepts before input). Update the guest side to open it before the input stream. The pairing relies on this exact ordering because `accept_bi` is FIFO.

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test --test control_channel_session -- --nocapture`
Expected: PASS.

- [ ] **Step 6: Run the rest of the tests**

Run: `just test`
Expected: all tests pass; if any pre-existing host/guest harness breaks because of the new bidi ordering, fix the ordering inside that harness.

- [ ] **Step 7: Commit**

```bash
git add crates/app/src/session.rs tests/control_channel_session.rs
git commit -m "feat(session): open persistent ControlChannel after handshake"
```

---

## Task 3: Guest cursor → host (send-only path)

**Files:**
- Modify: `crates/app/src/session.rs` — guest section adds a `CursorSender` and a polling task.
- Modify: `crates/app/src/session.rs` — host's discarded `_host_cursor_watch` becomes a logged tracing watch.
- Test: `crates/app/src/session.rs` (inline test for `scale_cursor_to_frame` already exists; add one new unit test for the new poll task only if extracted to a helper).

- [ ] **Step 1: Write the failing test (extract the poll loop into a testable helper first)**

Create a small helper in `session.rs`:

```rust
/// Poll local cursor and forward debounced updates into a sender.
///
/// Extracted for test coverage. Returns when the sender is closed or the
/// poll function returns `Err`.
async fn run_cursor_poll<F>(
    poll: F,
    update_tx: tokio::sync::mpsc::Sender<CursorUpdate>,
    frame_width: u16,
    frame_height: u16,
    peer_id: u8,
) where
    F: Fn() -> pairwire_core::Result<((u16, u16), (u16, u16))> + Send + 'static,
{
    let mut interval = tokio::time::interval(std::time::Duration::from_millis(16));
    let mut last_sent: Option<CursorUpdate> = None;
    loop {
        interval.tick().await;
        match poll() {
            Ok(((x, y), (dw, dh))) => {
                let (sx, sy) = scale_cursor_to_frame(x, y, dw, dh, frame_width, frame_height);
                let update = CursorUpdate {
                    peer_id,
                    x: sx,
                    y: sy,
                    visible: true,
                    click_animation: 0,
                };
                if last_sent.as_ref() == Some(&update) {
                    continue;
                }
                last_sent = Some(update.clone());
                if update_tx.send(update).await.is_err() {
                    break;
                }
            }
            Err(_) => break,
        }
    }
}
```

Test:

```rust
#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn cursor_poll_uses_provided_peer_id() {
    let (tx, mut rx) = tokio::sync::mpsc::channel(4);
    let task = tokio::spawn(run_cursor_poll(
        || Ok(((100, 200), (1000, 1000))),
        tx,
        1000,
        1000,
        7,
    ));

    tokio::time::advance(std::time::Duration::from_millis(20)).await;
    let update = rx.recv().await.unwrap();
    assert_eq!(update.peer_id, 7);
    assert_eq!((update.x, update.y), (100, 200));

    drop(rx);
    let _ = task.await;
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p pairwire cursor_poll_uses_provided_peer_id`
Expected: compile error (helper not yet defined) OR fail.

- [ ] **Step 3: Refactor host poll loop to use the helper**

Replace the existing `cursor_poll_task` block (`session.rs:203-242`) with:

```rust
let cursor_poll_task = {
    let tx = cursor_update_tx.clone();
    tokio::spawn(run_cursor_poll(
        pairwire_input::current_cursor_state,
        tx,
        cursor_frame_width,
        cursor_frame_height,
        0, // host peer_id
    ))
};
drop(cursor_update_tx);
```

- [ ] **Step 4: Add the symmetric block in `run_join`**

Insert after `cursor_recv_handle, mut cursor_watch_rx) = CursorReceiver::spawn(cursor_chan_rx);` (around line 547):

```rust
// Guest cursor → host. Use the negotiated frame size from the handshake so
// coordinates land in the same coordinate space the host's overlay expects.
let guest_cursor_send = pairwire_transport::CursorSender::new(Arc::clone(&conn));
let (guest_cursor_update_tx, guest_cursor_update_rx) = tokio::sync::mpsc::channel(1);
let guest_cursor_send_handle = guest_cursor_send.spawn(guest_cursor_update_rx);

#[allow(clippy::cast_possible_truncation)]
let frame_w = session.display_width.max(1);
#[allow(clippy::cast_possible_truncation)]
let frame_h = session.display_height.max(1);

let guest_cursor_poll_task = tokio::spawn(run_cursor_poll(
    pairwire_input::current_cursor_state,
    guest_cursor_update_tx,
    frame_w,
    frame_h,
    1, // guest peer_id
));
```

> `NegotiatedSession` exposes `display_width`/`display_height` populated from the host's handshake; if the field is currently named differently, use whatever holds the host's resolution (search `display_width` in `crates/transport/src/handshake.rs`).

In cleanup:

```rust
guest_cursor_poll_task.abort();
let _ = guest_cursor_poll_task.await;
guest_cursor_send_handle.stop().await;
```

- [ ] **Step 5: Replace host's discarded watch with a logging task**

Replace `let (_host_cursor_handle, _host_cursor_watch) = CursorReceiver::spawn(cursor_chan_rx);` (`session.rs:244`) with:

```rust
let (host_cursor_recv_handle, mut host_cursor_watch_rx) =
    CursorReceiver::spawn(cursor_chan_rx);
let host_cursor_log_task = tokio::spawn(async move {
    loop {
        if host_cursor_watch_rx.changed().await.is_err() {
            break;
        }
        if let Some(update) = host_cursor_watch_rx.borrow().clone() {
            tracing::info!(
                peer_id = update.peer_id,
                x = update.x,
                y = update.y,
                "guest cursor"
            );
        }
    }
});
```

In cleanup:

```rust
host_cursor_log_task.abort();
let _ = host_cursor_log_task.await;
host_cursor_recv_handle.stop().await;
```

- [ ] **Step 6: Run tests**

Run: `cargo test -p pairwire cursor_poll_uses_provided_peer_id`
Expected: PASS.

Run: `just check && just test`
Expected: workspace builds; existing tests pass.

- [ ] **Step 7: Smoke-test manually**

```bash
cargo run --features encode,capture,audio,gui,tray -- host --allow-remote-control &
HOST=$!
# In another shell paste the printed invite into:
cargo run --features encode,capture,audio,gui,tray -- join "<invite>"
```

Expected: with `RUST_LOG=info`, the host log shows `guest cursor peer_id=1 x=… y=…` updates as the guest moves their mouse over the viewer window.

- [ ] **Step 8: Commit**

```bash
git add crates/app/src/session.rs
git commit -m "feat(session): wire guest cursor → host with peer_id=1, log on host"
```

---

## Task 4: RuntimeControlState + tray grant/revoke menu items

**Files:**
- Create: `crates/app/src/control_state.rs`
- Modify: `crates/app/src/main.rs` (mod declaration, plumbing)
- Modify: `crates/app/src/tray.rs` — new menu items + `TrayAction` variants
- Test: `crates/app/src/control_state.rs` inline tests

- [ ] **Step 1: Write failing tests**

```rust
// crates/app/src/control_state.rs
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
```

- [ ] **Step 2: Run tests — must fail (module does not exist)**

Run: `cargo test -p pairwire control_state`
Expected: FAIL ("module `control_state` not found").

- [ ] **Step 3: Implement the module**

```rust
// crates/app/src/control_state.rs
//! Runtime control state used by both host and guest (host owns the source
//! of truth; guest mirrors what the host most recently confirmed).

use pairwire_input::InputPermission;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeControlState {
    /// Default. Host owns the keyboard/mouse; input from guest is dropped.
    HostActive,
    /// Guest has sent a `ControlRequest`; host has not yet decided.
    GuestRequested,
    /// Guest's input is forwarded to the host OS.
    GuestActive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlAction {
    /// Guest requests control (sets host to GuestRequested).
    Request,
    /// Host grants control (moves to GuestActive).
    Grant,
    /// Host revokes control (back to HostActive).
    Revoke,
}

impl RuntimeControlState {
    pub fn apply(&mut self, action: ControlAction) {
        *self = match (*self, action) {
            (Self::HostActive, ControlAction::Request) => Self::GuestRequested,
            (Self::HostActive | Self::GuestRequested, ControlAction::Grant) => Self::GuestActive,
            (Self::GuestActive | Self::GuestRequested, ControlAction::Revoke) => Self::HostActive,
            (state, _) => state,
        };
    }

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
```

Add to `crates/app/src/main.rs`:

```rust
mod control_state;
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p pairwire control_state`
Expected: PASS (5 tests).

- [ ] **Step 5: Add tray actions**

In `crates/app/src/tray.rs` inside `inner` mod, extend `TrayAction`:

```rust
pub enum TrayAction {
    CopyInvite(String),
    Disconnect,
    Quit,
    /// Grant remote control to the connected guest.
    GrantControl,
    /// Revoke remote control.
    RevokeControl,
}
```

Add fields on `TrayHandle`:

```rust
grant_item: MenuItem,
grant_id: tray_icon::menu::MenuId,
revoke_item: MenuItem,
revoke_id: tray_icon::menu::MenuId,
```

In `TrayHandle::new`, build the items before the `disconnect_item`:

```rust
let grant_item = MenuItem::new("Allow Remote Control", true, None);
let revoke_item = MenuItem::new("Revoke Control", false, None); // disabled until granted
let grant_id = grant_item.id().clone();
let revoke_id = revoke_item.id().clone();

menu.append_items(&[
    &copy_item,
    &grant_item,
    &revoke_item,
    &separator,
    &disconnect_item,
    &separator, // existing separator
    &quit_item,
])?;
```

Add a method:

```rust
pub fn set_control_granted(&mut self, granted: bool) {
    self.grant_item.set_enabled(!granted);
    self.revoke_item.set_enabled(granted);
}
```

Extend `poll`:

```rust
} else if id == &self.grant_id {
    debug!("tray: GrantControl");
    Some(TrayAction::GrantControl)
} else if id == &self.revoke_id {
    debug!("tray: RevokeControl");
    Some(TrayAction::RevokeControl)
}
```

- [ ] **Step 6: Verify build**

Run: `just check`
Expected: clean.

- [ ] **Step 7: Commit**

```bash
git add crates/app/src/control_state.rs crates/app/src/tray.rs crates/app/src/main.rs
git commit -m "feat(app): add RuntimeControlState and tray Grant/Revoke menu items"
```

---

## Task 5: Wire ControlRequest/Grant/Revoke through ControlChannel

**Files:**
- Modify: `crates/app/src/session.rs` — host loop consumes inbound `ControlMessage`s and tray `ControlAction`s; guest loop sends `ControlRequest` and consumes inbound `ControlGrant`/`ControlRevoke`.
- Modify: `crates/app/src/main.rs` — bridges tray actions into a `mpsc::Sender<ControlAction>` consumed by the session.
- Modify: `crates/app/src/cli.rs` — `--allow-remote-control` becomes "auto-grant on first request" (no upfront grant).

- [ ] **Step 1: Write the failing integration test**

```rust
// tests/control_handoff.rs
use std::time::Duration;
use pairwire_protocol::{ControlGrant, ControlMessage, ControlRequest};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn guest_request_grants_control_when_host_auto_grants() {
    let outcome = pairwire::testing::run_pair_session_with(
        pairwire::testing::HostOpts { auto_grant: true, ..Default::default() },
        |ctx| async move {
            ctx.guest_send_control_message(
                ControlMessage::ControlRequest(ControlRequest { peer_id: 1 }),
            )
            .await;

            let grant = tokio::time::timeout(
                Duration::from_secs(2),
                ctx.guest_wait_for(|m| matches!(m, ControlMessage::ControlGrant(_))),
            )
            .await
            .expect("grant not received in time");
            assert!(matches!(
                grant,
                ControlMessage::ControlGrant(ControlGrant { peer_id: 1 })
            ));

            // Host's input permission should now be FullControl.
            assert_eq!(
                ctx.host_input_permission().await,
                pairwire_input::InputPermission::FullControl
            );
        },
    )
    .await;
    outcome.expect("session pair completed");
}
```

> If the test harness from Task 2 doesn't support `auto_grant` / `host_input_permission`, this task includes extending the harness (small struct fields exposed to the test closure).

- [ ] **Step 2: Run test — must fail**

Run: `cargo test --test control_handoff -- --nocapture`
Expected: FAIL.

- [ ] **Step 3: Implement host handling**

Inside `run_host`, after `host_ctrl_handle` is created (Task 2), add a state machine task:

```rust
let auto_grant = args.allow_remote_control;
let input_recv_handle_for_ctrl = input_recv_handle.as_ref().map(|h| h.clone_handle());
// NOTE: InputReceiverHandle currently consumes self in stop(); add a clone()
// or wrap it in Arc earlier in this function. The cleanest path is to wrap
// `Option<InputReceiverHandle>` in `Arc<...>` before this point — see step 3a.
```

**Step 3a (sub-step):** Refactor `input_recv_handle: Option<InputReceiverHandle>` to be shared via `Arc`. Either:
- Change the field of `InputReceiverHandle` so `set_permission` takes `&self` (it already does — see `crates/input/src/receiver.rs:52`) and wrap the whole handle in `Arc<InputReceiverHandle>`, or
- Add a `permission_handle()` method that returns just the `Arc<AtomicPermission>` for sharing.

Use the second approach — cleaner:

```rust
// crates/input/src/receiver.rs — add a method
impl InputReceiverHandle {
    /// Cloneable handle that can flip permission state from another task.
    #[must_use]
    pub fn permission_handle(&self) -> Arc<AtomicPermission> {
        Arc::clone(&self.permission)
    }
}
```

Make `AtomicPermission` `pub` (currently private). Add a public method `AtomicPermission::set` that takes `InputPermission`. Add unit tests for the new public surface.

**Step 3b:** Now in `run_host`, after `input_recv_handle` is bound:

```rust
let host_perm_handle = input_recv_handle
    .as_ref()
    .map(pairwire_input::InputReceiverHandle::permission_handle);

let host_ctrl_outbound_for_task = host_ctrl_outbound_tx.clone();
let host_capture_show_cursor = Arc::clone(&capture_show_cursor); // see Task 6
let mut host_ctrl_inbound_for_task = host_ctrl_handle.subscribe();

let (tray_action_tx, mut tray_action_rx) =
    tokio::sync::mpsc::channel::<crate::control_state::ControlAction>(8);

let host_ctrl_state_task = tokio::spawn(async move {
    use crate::control_state::{ControlAction, RuntimeControlState};
    use pairwire_protocol::{ControlGrant, ControlMessage, ControlRevoke};

    let mut state = RuntimeControlState::HostActive;
    loop {
        tokio::select! {
            msg = host_ctrl_inbound_for_task.recv() => {
                match msg {
                    Ok(ControlMessage::ControlRequest(req)) => {
                        state.apply(ControlAction::Request);
                        info!(peer_id = req.peer_id, "guest requested control");
                        if auto_grant {
                            state.apply(ControlAction::Grant);
                            apply_permission(&host_perm_handle, state);
                            host_capture_show_cursor.store(false, Ordering::Relaxed);
                            let _ = host_ctrl_outbound_for_task
                                .send(ControlMessage::ControlGrant(ControlGrant {
                                    peer_id: req.peer_id,
                                }))
                                .await;
                            info!("auto-granted control to guest");
                        }
                    }
                    Ok(_) => {} // ignore other messages here
                    Err(_) => break,
                }
            }
            tray_action = tray_action_rx.recv() => {
                let Some(action) = tray_action else { break; };
                state.apply(action);
                apply_permission(&host_perm_handle, state);
                match action {
                    ControlAction::Grant => {
                        host_capture_show_cursor.store(false, Ordering::Relaxed);
                        let _ = host_ctrl_outbound_for_task
                            .send(ControlMessage::ControlGrant(ControlGrant { peer_id: 1 }))
                            .await;
                    }
                    ControlAction::Revoke => {
                        host_capture_show_cursor.store(true, Ordering::Relaxed);
                        let _ = host_ctrl_outbound_for_task
                            .send(ControlMessage::ControlRevoke(ControlRevoke { peer_id: 1 }))
                            .await;
                    }
                    ControlAction::Request => {} // host never requests
                }
            }
        }
    }
});

fn apply_permission(
    perm: &Option<Arc<pairwire_input::AtomicPermission>>,
    state: crate::control_state::RuntimeControlState,
) {
    if let Some(p) = perm {
        p.set(state.input_permission());
    }
}
```

In cleanup:

```rust
host_ctrl_state_task.abort();
let _ = host_ctrl_state_task.await;
```

- [ ] **Step 4: Implement guest handling**

Inside `run_join`, after `guest_ctrl_handle` is created (Task 2):

```rust
let viewer_ctrl_state_tx = control_state_tx.clone();
let mut guest_ctrl_inbound_for_task = guest_ctrl_handle.subscribe();
let guest_ctrl_state_task = tokio::spawn(async move {
    use crate::control_state::{ControlAction, RuntimeControlState};
    use pairwire_protocol::ControlMessage;
    let mut state = RuntimeControlState::HostActive;
    while let Ok(msg) = guest_ctrl_inbound_for_task.recv().await {
        match msg {
            ControlMessage::ControlGrant(_) => {
                state.apply(ControlAction::Grant);
                let _ = viewer_ctrl_state_tx.send(state);
                info!("control granted by host");
            }
            ControlMessage::ControlRevoke(_) => {
                state.apply(ControlAction::Revoke);
                let _ = viewer_ctrl_state_tx.send(state);
                info!("control revoked by host");
            }
            _ => {}
        }
    }
});
```

Where `control_state_tx: watch::Sender<RuntimeControlState>` is a new viewer channel passed into `run_join` from `main.rs` (mirroring the pattern of `mute_rx`).

The viewer's `Request Control` button (`Cmd+Shift+R`) emits:

```rust
let _ = guest_ctrl_outbound_tx
    .send(pairwire_protocol::ControlMessage::ControlRequest(
        pairwire_protocol::ControlRequest { peer_id: 1 },
    ))
    .await;
```

- [ ] **Step 5: Wire tray → session in main.rs**

In `crates/app/src/main.rs`, after `TrayHandle::new`:

```rust
loop {
    if let Some(action) = tray_handle.poll() {
        match action {
            TrayAction::CopyInvite(url) => { /* existing */ }
            TrayAction::Disconnect => { /* existing */ }
            TrayAction::Quit => { /* existing */ }
            TrayAction::GrantControl => {
                let _ = tray_action_tx.try_send(ControlAction::Grant);
                tray_handle.set_control_granted(true);
            }
            TrayAction::RevokeControl => {
                let _ = tray_action_tx.try_send(ControlAction::Revoke);
                tray_handle.set_control_granted(false);
            }
        }
    }
    std::thread::sleep(std::time::Duration::from_millis(50));
}
```

- [ ] **Step 6: Update CLI semantics**

`crates/app/src/cli.rs`: keep the field `allow_remote_control: bool`. Update the doc comment:

```rust
/// When set, automatically grants `FullControl` the moment the guest sends a
/// `ControlRequest`. When unset, the host must click "Allow Remote Control" in
/// the tray menu to grant.
pub allow_remote_control: bool,
```

In `session.rs` (Task 5 / step 3b above), the host no longer pre-grants on session start.

- [ ] **Step 7: Run all tests**

Run: `just test`
Expected: PASS, including the new `tests/control_handoff.rs`.

- [ ] **Step 8: Manual smoke test**

Without auto-grant:
```bash
cargo run --features encode,capture,audio,gui,tray -- host
# in another shell after copying invite:
cargo run --features encode,capture,audio,gui,tray -- join "<invite>"
```

Steps:
1. Verify guest viewer cannot move host's mouse initially.
2. Click `Request Control` in viewer (or press `Cmd+Shift+R`).
3. Host log shows `guest requested control`.
4. Click `Allow Remote Control` in macOS menu bar.
5. Guest can now move host's mouse; viewer HUD shows `You have control`.
6. Click `Revoke Control` in tray. Guest viewer stops moving host's mouse; HUD shows `View only`.

- [ ] **Step 9: Commit**

```bash
git add crates/app/src/session.rs crates/app/src/main.rs crates/app/src/cli.rs crates/input/src/receiver.rs crates/input/src/lib.rs tests/control_handoff.rs
git commit -m "feat(control): implement ControlRequest/Grant/Revoke runtime handoff"
```

---

## Task 6: Hide host system cursor while guest has control

**Files:**
- Create: `crates/capture/src/cursor_visibility.rs`
- Modify: `crates/capture/src/capture_thread.rs`
- Modify: `crates/capture/src/lib.rs`
- Modify: `crates/app/src/session.rs`

- [ ] **Step 1: Write the failing test**

```rust
// crates/capture/src/cursor_visibility.rs
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

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
        assert_eq!(cloned.inner().load(Ordering::Relaxed), false);
    }
}
```

- [ ] **Step 2: Run test — must fail**

Run: `cargo test -p pairwire-capture cursor_visibility`
Expected: compile error.

- [ ] **Step 3: Implement `ShowCursorFlag`**

```rust
// crates/capture/src/cursor_visibility.rs
//! Cloneable flag that controls whether the system cursor is rendered into
//! captured frames. Toggling at runtime requires the capture thread to
//! observe the flag and (on platforms without a live API) tear down + rebuild
//! the underlying capturer. The capture thread checks the flag on a debounce
//! tick rather than every frame.

use std::sync::{Arc, atomic::{AtomicBool, Ordering}};

#[derive(Clone, Debug)]
pub struct ShowCursorFlag(Arc<AtomicBool>);

impl ShowCursorFlag {
    #[must_use]
    pub fn new(initial: bool) -> Self {
        Self(Arc::new(AtomicBool::new(initial)))
    }

    pub fn set(&self, v: bool) {
        self.0.store(v, Ordering::Relaxed);
    }

    #[must_use]
    pub fn get(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }

    #[must_use]
    pub fn inner(&self) -> &AtomicBool {
        &self.0
    }
}
```

Add to `crates/capture/src/lib.rs`:

```rust
pub mod cursor_visibility;
pub use cursor_visibility::ShowCursorFlag;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p pairwire-capture cursor_visibility`
Expected: PASS.

- [ ] **Step 5: Wire flag into `CaptureThread::start`**

Modify `CaptureThread::start` to accept the flag and observe it:

```rust
pub fn start(
    config: CaptureConfig,
    show_cursor: ShowCursorFlag,
) -> Result<(Self, mpsc::Receiver<RawFrame>)> {
    // ... existing setup ...
    let show_cursor_thread = show_cursor.clone();
    let handle = thread::Builder::new()
        .name("pairwire-capture".into())
        .spawn(move || {
            let mut current_show_cursor = show_cursor_thread.get();
            let options = build_options(&config, current_show_cursor);
            let mut capturer = match Capturer::build(options) { /* ... */ };
            capturer.start_capture();

            // Check the flag every N frames; rebuild the capturer on transition.
            let mut tick = 0u32;

            while !stop_flag_thread.load(Ordering::Relaxed) {
                tick = tick.wrapping_add(1);
                if tick.is_multiple_of(15) {
                    let want = show_cursor_thread.get();
                    if want != current_show_cursor {
                        debug!(show_cursor = want, "rebuilding capturer for cursor toggle");
                        capturer.stop_capture();
                        let opts = build_options(&config, want);
                        match Capturer::build(opts) {
                            Ok(c) => {
                                capturer = c;
                                capturer.start_capture();
                                current_show_cursor = want;
                            }
                            Err(e) => {
                                warn!(error = %e, "failed to rebuild capturer; keeping old");
                            }
                        }
                    }
                }

                match capturer.get_next_frame() {
                    /* unchanged */
                }
            }
            // ... unchanged ...
        })
        .map_err(/* unchanged */)?;
    /* unchanged */
}

fn build_options(config: &CaptureConfig, show_cursor: bool) -> Options {
    Options {
        fps: config.fps,
        show_cursor,
        show_highlight: false,
        target: None,
        output_type: FrameType::BGRAFrame,
        output_resolution: Resolution::Captured,
        ..Default::default()
    }
}
```

Apply the same change to `start_fake` (it has no cursor — accept the flag and ignore for now).

- [ ] **Step 6: Update callers in `session.rs`**

```rust
let capture_show_cursor = pairwire_capture::ShowCursorFlag::new(true);
// pass into capture
let (capture_thread, frame_rx) =
    start_capture(capture_config.clone(), capture_show_cursor.clone())?;
```

Update the `start_capture` helper signature accordingly. Pass `capture_show_cursor` into the host control state task (Task 5 step 3b already references it).

- [ ] **Step 7: Run tests**

Run: `just test`
Expected: PASS.

- [ ] **Step 8: Manual smoke test**

Repeat Task 5's smoke test. After granting control, the cursor in the guest's view should disappear (no double-cursor) — only the guest's own overlay arrow remains. After revoking, the host system cursor reappears in the captured stream.

- [ ] **Step 9: Commit**

```bash
git add crates/capture/src/cursor_visibility.rs crates/capture/src/capture_thread.rs crates/capture/src/lib.rs crates/app/src/session.rs
git commit -m "feat(capture): hide host system cursor while guest has control"
```

---

## Task 7: HUD reflects RuntimeControlState

**Files:**
- Modify: `crates/app/src/viewer.rs` — add control-state line to the bottom-right badge stack and a "Request Control" button.
- Modify: `crates/app/src/main.rs` — pass `watch::Receiver<RuntimeControlState>` and the guest control outbound `mpsc::Sender<ControlMessage>` into the viewer.

- [ ] **Step 1: Extend `ViewerApp` fields**

```rust
pub struct ViewerApp {
    // ... existing ...
    control_rx: watch::Receiver<crate::control_state::RuntimeControlState>,
    control_tx: tokio::sync::mpsc::Sender<pairwire_protocol::ControlMessage>,
}
```

Update its constructor and the call sites in `main.rs` / `run_join` to thread the channels through.

- [ ] **Step 2: Render HUD line**

In the existing HUD block (search for the FPS counter, around `viewer.rs:200-260`):

```rust
let state = *self.control_rx.borrow();
let (label, color) = match state {
    RuntimeControlState::HostActive => ("View only", egui::Color32::GRAY),
    RuntimeControlState::GuestRequested => {
        ("Awaiting host approval…", egui::Color32::YELLOW)
    }
    RuntimeControlState::GuestActive => ("You have control", egui::Color32::LIGHT_GREEN),
};
ui.colored_label(color, label);

if state == RuntimeControlState::HostActive
    && ui.button("Request Control").clicked()
{
    let tx = self.control_tx.clone();
    tokio::spawn(async move {
        let _ = tx
            .send(pairwire_protocol::ControlMessage::ControlRequest(
                pairwire_protocol::ControlRequest { peer_id: 1 },
            ))
            .await;
    });
}
```

Also bind `Cmd+Shift+R` (or `Ctrl+Shift+R` on non-mac):

```rust
if ctx.input(|i| i.modifiers.command && i.modifiers.shift && i.key_pressed(egui::Key::R)) {
    // same send as the button
}
```

- [ ] **Step 3: Verify build**

Run: `just check && just lint`
Expected: clean.

- [ ] **Step 4: Manual smoke test**

Repeat Task 5's smoke test and visually confirm the HUD label transitions:
- Initial: `View only` (gray)
- After click `Request Control`: `Awaiting host approval…` (yellow)
- After host grants: `You have control` (green)
- After host revokes: back to `View only`

- [ ] **Step 5: Commit**

```bash
git add crates/app/src/viewer.rs crates/app/src/main.rs
git commit -m "feat(viewer): HUD control state + Request Control button (Cmd+Shift+R)"
```

---

## Task 8: End-to-end smoke test + epic close-out

**Files:**
- Modify: `SMOKE_TEST.md` — add a section for the new flows.
- Modify: `LEARNINGS.md` — capture any quirks discovered (scap rebuild on cursor toggle latency, broadcast lag, etc.).
- Modify: `tasks/README.md` — mark this epic complete.

- [ ] **Step 1: Update SMOKE_TEST.md**

Append a new section:

```markdown
## UX Polish Epic — Manual Verification

Run host (without auto-grant):
```
cargo run --features encode,capture,audio,gui,tray -- host
```

Run guest (separate machine or process):
```
cargo run --features encode,capture,audio,gui,tray -- join "<invite>"
```

Verify:
- [ ] Guest cursor logged on host (`RUST_LOG=info` shows `guest cursor peer_id=1 …`).
- [ ] Tray menu has `Allow Remote Control` (enabled) and `Revoke Control` (disabled).
- [ ] Guest viewer HUD shows `View only` initially.
- [ ] `Cmd+Shift+R` on viewer → host log shows `guest requested control`; HUD shows `Awaiting host approval…`.
- [ ] Click tray `Allow Remote Control` → HUD switches to `You have control`; guest can drive host mouse.
- [ ] Captured stream shows no system cursor while granted (only guest's own overlay arrow).
- [ ] Click tray `Revoke Control` → HUD back to `View only`; system cursor reappears in stream.
- [ ] `--allow-remote-control` flag still works as auto-grant on first request.
```

- [ ] **Step 2: Run the full validation suite**

Run: `just all`
Expected: `cargo check`, `cargo clippy -- -D warnings`, `cargo test`, `cargo fmt -- --check` all pass.

- [ ] **Step 3: Update tasks/README.md**

Add row:

```markdown
| `epic-ux-polish.md` | UX Polish — cursor + control + capture polish | ✅ Complete |
```

- [ ] **Step 4: Update LEARNINGS.md**

Add at least one entry covering whatever non-obvious gotcha came up (e.g., "scap requires capturer rebuild on `show_cursor` toggle — ~80ms hitch on transition", or "broadcast channel `recv` returns `Lagged` if HUD is slow — match on it").

- [ ] **Step 5: Final commit**

```bash
git add SMOKE_TEST.md LEARNINGS.md tasks/README.md tasks/epic-ux-polish.md
git commit -m "docs: close out UX polish epic with smoke test + learnings"
```

---

## Validation Checklist

Run before declaring the epic complete:

```bash
just all
```

- [ ] `cargo check --workspace` clean
- [ ] `cargo clippy --workspace -- -D warnings` clean
- [ ] `cargo test --workspace` all green
- [ ] `cargo fmt --all -- --check` clean
- [ ] All eight task commits landed in order
- [ ] Manual smoke test in `SMOKE_TEST.md` "UX Polish Epic" section runs end-to-end
- [ ] HUD label transitions match the four expected states
- [ ] System cursor hides on grant, reappears on revoke
- [ ] Tray menu items enable/disable in lockstep with control state
- [ ] `LEARNINGS.md` updated with at least one new entry
