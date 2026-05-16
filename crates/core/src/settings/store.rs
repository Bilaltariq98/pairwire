//! File-backed I/O for [`super::Settings`].
//!
//! Provides:
//! - [`default_config_path`] — OS-correct location (`directories` crate).
//! - [`load_or_init`] — read the file; if absent, write a commented
//!   template and return defaults.
//! - [`save`] — atomic write (tmp + rename) so a crash mid-save cannot
//!   leave a half-written `config.toml` on disk.
//!
//! Errors from this module use [`crate::Error::ConfigIo`] for parse and
//! directory-discovery failures, and [`crate::Error::Io`] for raw
//! filesystem errors via `#[from]`.

use crate::{Error, Result, Settings};
use std::fs;
use std::path::{Path, PathBuf};

/// Filename used inside the project config directory.
pub const CONFIG_FILENAME: &str = "config.toml";

/// Top-of-file comment block prepended to the rendered TOML on first
/// run. Kept short — orientation only; the field names are self-documenting.
const FIRST_RUN_HEADER: &str = "\
# pairwire config — edit freely. Unknown fields are ignored.
# Generated on first run. Re-run with `pairwire config edit` to open in $EDITOR.
#
# Safe defaults: remote control denied, clipboard text-only, terminals
# read-only, database ports (3306/5432/6379/27017) blocked.
#
";

/// Resolve the OS-correct path to `config.toml`.
///
/// - macOS:    `~/Library/Application Support/pairwire/config.toml`
/// - Linux:    `~/.config/pairwire/config.toml`
/// - Windows:  `%APPDATA%\pairwire\config.toml`
///
/// Returns [`Error::ConfigIo`] on platforms where a config directory
/// cannot be determined (e.g. no `$HOME`).
pub fn default_config_path() -> Result<PathBuf> {
    let dirs = directories::ProjectDirs::from("", "", "pairwire").ok_or_else(|| {
        Error::ConfigIo("could not determine OS config directory (no $HOME?)".into())
    })?;
    Ok(dirs.config_dir().join(CONFIG_FILENAME))
}

/// Load settings from `path`. If the file does not exist, write a
/// commented template at that path and return `Settings::default()`.
///
/// Parent directories are created as needed. Parse errors are returned
/// rather than silently masked by defaults — a corrupted config file is
/// almost certainly a user mistake worth surfacing.
pub fn load_or_init(path: &Path) -> Result<Settings> {
    match fs::read_to_string(path) {
        Ok(text) => {
            let settings: Settings = toml::from_str(&text)?;
            Ok(settings)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let settings = Settings::default();
            write_template(path, &settings)?;
            Ok(settings)
        }
        Err(e) => Err(Error::Io(e)),
    }
}

/// Atomically write `settings` to `path` (TOML, no header comment).
///
/// Implementation: write to `<path>.tmp` in the same directory, then
/// rename over the target. Rename on the same filesystem is atomic on
/// macOS, Linux, and Windows.
pub fn save(path: &Path, settings: &Settings) -> Result<()> {
    let body = toml::to_string_pretty(settings)?;
    write_atomic(path, body.as_bytes())
}

/// First-run template: TOML body prefixed with the orientation header.
fn write_template(path: &Path, settings: &Settings) -> Result<()> {
    let body = toml::to_string_pretty(settings)?;
    let mut full = String::with_capacity(FIRST_RUN_HEADER.len() + body.len());
    full.push_str(FIRST_RUN_HEADER);
    full.push_str(&body);
    write_atomic(path, full.as_bytes())
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }
    let tmp = tmp_path_for(path);
    fs::write(&tmp, bytes)?;
    // `fs::rename` replaces the destination atomically on all our targets.
    match fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            // Best-effort cleanup — don't shadow the real error.
            let _ = fs::remove_file(&tmp);
            Err(Error::Io(e))
        }
    }
}

/// Build the sibling temp path: `config.toml` → `config.toml.tmp`.
fn tmp_path_for(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(".tmp");
    PathBuf::from(s)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{ClipboardPolicy, RemoteControlPolicy};

    fn tmp_dir() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    #[test]
    fn default_config_path_ends_with_pairwire_config_toml() {
        let path = default_config_path().expect("config path");
        let s = path.to_string_lossy();
        assert!(s.ends_with("config.toml"), "path = {s}");
        assert!(s.contains("pairwire"), "path = {s}");
    }

    #[test]
    fn load_or_init_creates_file_when_missing_and_returns_defaults() {
        let dir = tmp_dir();
        let path = dir.path().join("config.toml");
        assert!(!path.exists());

        let settings = load_or_init(&path).expect("load_or_init");
        assert_eq!(settings, Settings::default());
        assert!(path.exists(), "first run should create the file");
    }

    #[test]
    fn generated_template_round_trips_back_to_same_settings() {
        let dir = tmp_dir();
        let path = dir.path().join("config.toml");

        let first = load_or_init(&path).expect("first load");
        let second = load_or_init(&path).expect("second load");
        assert_eq!(first, second, "template must reload identically");
    }

    #[test]
    fn generated_template_contains_header_comment() {
        let dir = tmp_dir();
        let path = dir.path().join("config.toml");
        load_or_init(&path).expect("load_or_init");
        let contents = fs::read_to_string(&path).expect("read");
        assert!(
            contents.starts_with("# pairwire config"),
            "first-run header missing:\n{contents}"
        );
    }

    #[test]
    fn load_or_init_creates_parent_dir() {
        let dir = tmp_dir();
        let nested = dir.path().join("deeply").join("nested").join("config.toml");
        assert!(!nested.parent().unwrap().exists());

        load_or_init(&nested).expect("load_or_init");
        assert!(nested.exists());
    }

    #[test]
    fn load_or_init_returns_parsed_contents_when_file_exists() {
        let dir = tmp_dir();
        let path = dir.path().join("config.toml");

        let mut settings = Settings::default();
        settings.permissions.remote_control = RemoteControlPolicy::AutoGrant;
        settings.permissions.clipboard_sync = ClipboardPolicy::Disabled;
        save(&path, &settings).expect("save");

        let loaded = load_or_init(&path).expect("load_or_init");
        assert_eq!(loaded, settings);
    }

    #[test]
    fn load_or_init_rejects_malformed_toml() {
        let dir = tmp_dir();
        let path = dir.path().join("config.toml");
        fs::write(&path, "this is = = not toml [[[").expect("write");

        let err = load_or_init(&path).unwrap_err();
        match err {
            Error::ConfigIo(_) => {}
            other => panic!("expected ConfigIo, got {other:?}"),
        }
    }

    #[test]
    fn save_overwrites_existing_file() {
        let dir = tmp_dir();
        let path = dir.path().join("config.toml");

        save(&path, &Settings::default()).expect("save 1");

        let mut modified = Settings::default();
        modified.audio.bitrate_kbps = 96;
        save(&path, &modified).expect("save 2");

        let loaded = load_or_init(&path).expect("load");
        assert_eq!(loaded.audio.bitrate_kbps, 96);
    }

    #[test]
    fn save_does_not_leave_tmp_file_behind() {
        let dir = tmp_dir();
        let path = dir.path().join("config.toml");
        save(&path, &Settings::default()).expect("save");

        let tmp = path.with_extension("toml.tmp");
        assert!(!tmp.exists(), "tmp file leaked: {tmp:?}");
    }

    #[test]
    fn tmp_path_for_appends_dot_tmp() {
        let p = Path::new("/foo/bar/config.toml");
        assert_eq!(tmp_path_for(p), PathBuf::from("/foo/bar/config.toml.tmp"));
    }
}
