//! Wire protocol message types for pairwire.
//!
//! All types are serialized with `postcard` (compact binary, not self-describing).
//! Both peers must agree on the exact type layout. Never reorder fields — only append.
//!
//! Protocol version is checked during handshake. Incompatible versions are rejected.

pub mod control;
pub mod cursor;
pub mod drawing;
pub mod input;
pub mod reaction;

#[cfg(any(test, feature = "test-support"))]
pub mod test_utils;

#[cfg(test)]
mod boundary_tests;

/// Current protocol version. Increment when making breaking wire format changes.
pub const PROTOCOL_VERSION: u16 = 1;

pub use control::{
    AudioCodec, Capability, ControlGrant, ControlHandoff, ControlMessage, ControlRequest,
    ControlRevoke, ControlState, Handshake, Heartbeat, Permissions, VideoCodec,
};
pub use cursor::CursorUpdate;
pub use drawing::{DrawClear, DrawMode, DrawPoint, DrawStroke};
pub use input::{ClipboardFormat, ClipboardSync, KeyEvent, MouseClick, MouseScroll};
pub use reaction::{EMOJI_SET, Reaction};
