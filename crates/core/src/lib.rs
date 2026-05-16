//! Shared types, configuration, and error definitions for pairwire.
//!
//! All other crates depend on this one. Keep it lean — no network I/O,
//! no platform-specific code, no heavy dependencies.

pub mod config;
pub mod diagnostics;
pub mod error;
pub mod settings;
pub mod types;

#[cfg(any(test, feature = "test-support"))]
pub mod test_utils;

pub use config::{AudioConfig, CaptureConfig, SessionConfig};
pub use error::{Error, Result};
pub use settings::{
    AudioSettings, CaptureSettings, ClipboardPolicy, H264Profile, IdentitySettings,
    PermissionSettings, RemoteControlPolicy, Settings, TerminalAccessPolicy, TerminalSettings,
    TunnelSettings, UiSettings, default_config_path, load_or_init, save,
};
pub use types::{PeerId, SessionId};
