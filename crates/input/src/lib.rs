//! Keyboard, mouse, and clipboard input forwarding.
//!
//! Two roles:
//! - `InputSender` (guest side): captures local input events, serializes, sends on QUIC stream
//! - `InputReceiver` (host side): receives events, injects into OS via platform APIs
//!
//! Permission model: host must explicitly grant control via `ControlHandoff` before
//! any input events are injected. Events received without permission are silently dropped.
//!
//! Platform injection:
//! - macOS: CGEvent posting (requires Accessibility permission)
//! - Windows: SendInput API (Phase 3)
//! - Linux: uinput / XTest (Phase 3)
//!
//! Implemented in P2.3.
