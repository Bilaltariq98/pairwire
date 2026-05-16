//! `pairwire config {path, get, set, edit}` — manage `config.toml`
//! without hand-editing TOML.
//!
//! - `path` prints the resolved location.
//! - `get` reads one dotted-key value via `toml::Value`.
//! - `set` mutates the file via `toml_edit::DocumentMut`, which
//!   preserves comments and formatting. After the in-memory mutation,
//!   the result is parsed back through `Settings` to validate — if the
//!   user typed nonsense, the file is **not** rewritten.
//! - `edit` execs `$EDITOR` (falls back to `vi`) on the resolved path.

use anyhow::{Context, Result, anyhow, bail};
use pairwire_core::Settings;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::cli::ConfigArgs;

/// Entry point used by `main.rs` after parsing.
pub fn run(args: ConfigArgs) -> Result<()> {
    let path = pairwire_core::default_config_path()
        .map_err(|e| anyhow!("could not determine config path: {e}"))?;
    match args {
        ConfigArgs::Path => run_path(&path),
        ConfigArgs::Get { key } => run_get(&path, &key),
        ConfigArgs::Set { key, value } => run_set(&path, &key, &value),
        ConfigArgs::Edit => run_edit(&path),
    }
}

fn run_path(path: &Path) -> Result<()> {
    println!("{}", path.display());
    Ok(())
}

fn run_get(path: &Path, key: &str) -> Result<()> {
    let settings = ensure_loaded(path)?;
    let text = toml::to_string(&settings).context("re-serialize loaded settings")?;
    let doc: toml::Value = toml::from_str(&text).context("parse loaded settings as toml::Value")?;
    let v = lookup(&doc, key).ok_or_else(|| {
        anyhow!("key '{key}' not found in config (try `pairwire config path` to inspect)")
    })?;
    println!("{}", render_scalar(v));
    Ok(())
}

fn run_set(path: &Path, key: &str, value: &str) -> Result<()> {
    // Ensure the file exists with defaults so `set` can run on a clean
    // machine — same first-run semantics as `host`.
    ensure_loaded(path)?;

    // Read the file as a comment-preserving document.
    let text = std::fs::read_to_string(path).context("read config")?;
    let mut doc: toml_edit::DocumentMut =
        text.parse().context("parse config as toml_edit document")?;

    // Insert the value, parsing it as TOML so quoted strings, numbers,
    // booleans, and arrays all work: `set capture.max_fps 60` and
    // `set permissions.remote_control "auto_grant"` both parse here.
    let item = parse_value(value)?;
    set_at_path(&mut doc, key, item)?;

    // Validate: parse the post-mutation document back through Settings.
    // If the user typed something the schema rejects, we abort without
    // writing — the on-disk file stays intact.
    let serialized = doc.to_string();
    let _validated: Settings = toml::from_str(&serialized)
        .with_context(|| format!("'{value}' is not a valid value for key '{key}'"))?;

    // Atomic write via the same helper as first-run save.
    atomic_write(path, &serialized)?;
    Ok(())
}

fn run_edit(path: &Path) -> Result<()> {
    // Ensure the file exists with defaults so $EDITOR has something to
    // open — same first-run semantics as `host`.
    ensure_loaded(path)?;

    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());
    let status = Command::new(&editor)
        .arg(path)
        .status()
        .with_context(|| format!("failed to launch $EDITOR ({editor})"))?;
    if !status.success() {
        bail!("editor exited with status {status}");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Load settings via the same first-run path as the host/join flows.
fn ensure_loaded(path: &Path) -> Result<Settings> {
    pairwire_core::load_or_init(path)
        .with_context(|| format!("loading config at {}", path.display()))
}

/// Walk a dotted key path through a `toml::Value`. Returns `None` if
/// any segment is missing or the path traverses through a non-table.
fn lookup<'a>(root: &'a toml::Value, key: &str) -> Option<&'a toml::Value> {
    let mut cur = root;
    for segment in key.split('.') {
        cur = cur.as_table()?.get(segment)?;
    }
    Some(cur)
}

/// Render a TOML scalar in the natural CLI shape — bare for strings,
/// `true`/`false` for bools, the numeric form for ints/floats. Tables
/// and arrays render as TOML.
fn render_scalar(v: &toml::Value) -> String {
    match v {
        toml::Value::String(s) => s.clone(),
        toml::Value::Integer(i) => i.to_string(),
        toml::Value::Float(f) => f.to_string(),
        toml::Value::Boolean(b) => b.to_string(),
        other => other.to_string(),
    }
}

/// Parse a raw CLI value into a `toml_edit::Item`. Strategy: wrap as
/// `x = <value>` and parse — this gives us TOML's natural type
/// inference (numbers stay numbers, `true`/`false` stay bools, quoted
/// strings stay strings, arrays parse as arrays).
fn parse_value(raw: &str) -> Result<toml_edit::Item> {
    let snippet = format!("x = {raw}");
    let doc: toml_edit::DocumentMut = snippet
        .parse()
        .with_context(|| format!("invalid value '{raw}' — examples: 60, true, \"auto_grant\""))?;
    let item = doc
        .get("x")
        .ok_or_else(|| anyhow!("internal: parsed snippet missing 'x'"))?
        .clone();
    Ok(item)
}

/// Insert/overwrite the value at a dotted key path. Intermediate
/// tables are created if missing.
fn set_at_path(doc: &mut toml_edit::DocumentMut, key: &str, item: toml_edit::Item) -> Result<()> {
    let segments: Vec<&str> = key.split('.').collect();
    if segments.is_empty() || segments.iter().any(|s| s.is_empty()) {
        bail!("invalid key '{key}' — use dotted form like capture.max_fps");
    }
    let (last, parents) = segments
        .split_last()
        .expect("non-empty segments after validation");
    let mut cursor: &mut toml_edit::Item = doc.as_item_mut();
    for seg in parents {
        if cursor.is_none() {
            *cursor = toml_edit::Item::Table(toml_edit::Table::new());
        }
        let table = cursor
            .as_table_like_mut()
            .ok_or_else(|| anyhow!("'{seg}' is not a table — cannot descend into it"))?;
        if !table.contains_key(seg) {
            table.insert(seg, toml_edit::Item::Table(toml_edit::Table::new()));
        }
        cursor = table
            .get_mut(seg)
            .expect("just inserted or already present");
    }
    let table = cursor
        .as_table_like_mut()
        .ok_or_else(|| anyhow!("parent of '{last}' is not a table"))?;
    table.insert(last, item);
    Ok(())
}

/// Atomic write — same pattern as `pairwire_core::settings::store::save`,
/// reimplemented here against a raw string so we don't have to go
/// through `Settings::save` and lose comments.
fn atomic_write(path: &Path, contents: &str) -> Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let mut tmp = PathBuf::from(path);
    let mut name = tmp.file_name().unwrap_or_default().to_owned();
    name.push(".tmp");
    tmp.set_file_name(name);
    std::fs::write(&tmp, contents.as_bytes())
        .with_context(|| format!("write {}", tmp.display()))?;
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e).with_context(|| format!("rename {} -> {}", tmp.display(), path.display()));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp_path() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        (dir, path)
    }

    // ─── get ──────────────────────────────────────────────────────────

    #[test]
    fn get_reads_a_default_field() {
        let (_dir, path) = tmp_path();
        // Initialize the file with defaults.
        pairwire_core::load_or_init(&path).unwrap();

        let text = fs::read_to_string(&path).unwrap();
        let doc: toml::Value = toml::from_str(&text).unwrap();
        let v = lookup(&doc, "capture.max_fps").expect("found");
        assert_eq!(render_scalar(v), "30");
    }

    #[test]
    fn get_returns_none_for_missing_key() {
        let (_dir, path) = tmp_path();
        pairwire_core::load_or_init(&path).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        let doc: toml::Value = toml::from_str(&text).unwrap();
        assert!(lookup(&doc, "capture.does_not_exist").is_none());
        assert!(lookup(&doc, "no_such_section.x").is_none());
    }

    #[test]
    fn get_renders_string_without_quotes() {
        let (_dir, path) = tmp_path();
        pairwire_core::load_or_init(&path).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        let doc: toml::Value = toml::from_str(&text).unwrap();
        let v = lookup(&doc, "permissions.remote_control").expect("found");
        // The on-disk value is "denied" — must print without TOML quotes.
        assert_eq!(render_scalar(v), "denied");
    }

    // ─── set ──────────────────────────────────────────────────────────

    #[test]
    fn set_persists_through_load_or_init() {
        let (_dir, path) = tmp_path();
        run_set(&path, "capture.max_fps", "60").expect("set");
        let s = pairwire_core::load_or_init(&path).expect("reload");
        assert_eq!(s.capture.max_fps, 60);
    }

    #[test]
    fn set_preserves_first_run_header_comment() {
        let (_dir, path) = tmp_path();
        // First-run template includes the header comment.
        pairwire_core::load_or_init(&path).unwrap();
        let before = fs::read_to_string(&path).unwrap();
        assert!(before.starts_with("# pairwire config"));

        run_set(&path, "capture.max_fps", "45").expect("set");

        let after = fs::read_to_string(&path).unwrap();
        assert!(
            after.starts_with("# pairwire config"),
            "header comment was wiped:\n{after}"
        );
        // And the value is actually there.
        assert!(after.contains("max_fps = 45"));
    }

    #[test]
    fn set_string_value_round_trips() {
        let (_dir, path) = tmp_path();
        run_set(&path, "permissions.remote_control", "\"auto_grant\"").expect("set string");
        let s = pairwire_core::load_or_init(&path).expect("reload");
        assert_eq!(
            s.permissions.remote_control,
            pairwire_core::RemoteControlPolicy::AutoGrant
        );
    }

    #[test]
    fn set_rejects_invalid_value_without_writing() {
        let (_dir, path) = tmp_path();
        pairwire_core::load_or_init(&path).unwrap();
        let before = fs::read_to_string(&path).unwrap();

        // "garbage" is not a valid RemoteControlPolicy variant.
        let err =
            run_set(&path, "permissions.remote_control", "\"garbage\"").expect_err("must reject");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("garbage") || msg.contains("remote_control"),
            "error should reference the bad value: {msg}"
        );

        // File is unchanged.
        let after = fs::read_to_string(&path).unwrap();
        assert_eq!(
            before, after,
            "file should not be written on validation failure"
        );
    }

    #[test]
    fn set_rejects_invalid_type_without_writing() {
        let (_dir, path) = tmp_path();
        pairwire_core::load_or_init(&path).unwrap();
        let before = fs::read_to_string(&path).unwrap();

        // max_fps is a u32; setting it to a string should fail validation.
        assert!(run_set(&path, "capture.max_fps", "\"sixty\"").is_err());

        let after = fs::read_to_string(&path).unwrap();
        assert_eq!(before, after);
    }

    #[test]
    fn set_creates_file_on_first_run() {
        // `pairwire config set` on a clean machine should bootstrap the
        // file (same as `host`/`edit`) instead of erroring on "no file".
        let (_dir, path) = tmp_path();
        assert!(!path.exists());
        run_set(&path, "capture.max_fps", "24").expect("set");
        assert!(path.exists());
        let s = pairwire_core::load_or_init(&path).unwrap();
        assert_eq!(s.capture.max_fps, 24);
    }

    #[test]
    fn set_rejects_empty_segment() {
        let (_dir, path) = tmp_path();
        pairwire_core::load_or_init(&path).unwrap();
        // `capture..max_fps` has an empty segment between the dots.
        assert!(run_set(&path, "capture..max_fps", "60").is_err());
    }

    // ─── path ──────────────────────────────────────────────────────────

    #[test]
    fn default_config_path_ends_with_pairwire_config_toml() {
        let p = pairwire_core::default_config_path().expect("path");
        let s = p.to_string_lossy();
        assert!(s.contains("pairwire") && s.ends_with("config.toml"), "{s}");
    }

    // ─── value parser ─────────────────────────────────────────────────

    #[test]
    fn parse_value_handles_naked_int() {
        let item = parse_value("60").unwrap();
        assert_eq!(item.as_integer(), Some(60));
    }

    #[test]
    fn parse_value_handles_quoted_string() {
        let item = parse_value("\"hello\"").unwrap();
        assert_eq!(item.as_str(), Some("hello"));
    }

    #[test]
    fn parse_value_handles_bool() {
        let item = parse_value("true").unwrap();
        assert_eq!(item.as_bool(), Some(true));
    }

    #[test]
    fn parse_value_rejects_garbage() {
        assert!(parse_value("== oh no ==").is_err());
    }
}
