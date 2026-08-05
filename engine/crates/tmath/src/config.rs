//! User-facing configuration file: `font_size_pt` (D-CONFIG phase 1) and
//! `cjk_font` (D-CONFIG phase 2), loaded from `config.toml` in the platform
//! config directory.
//!
//! Follows the exact directory pattern `agent_allowlist.rs` already uses for
//! its own state file (`XDG_CONFIG_HOME`, falling back to `$HOME/.config`) —
//! no new platform-directory crate dependency, per AGENTS.md's "runtime
//! artifacts live in a platform state directory" rule and this repo's
//! existing precedent. The file only ever holds small, non-content settings
//! (never document text, formula source, or rendered bytes), matching
//! AGENTS.md's privacy invariants.
//!
//! `font_size_pt` precedence (highest first): CLI flag > environment
//! variable > config file > terminal auto-fit > fixed default. Implemented
//! once in [`resolve_font_size_pt_with_source`] and reused by every entry
//! point (`tmath render`, `tmath watch`, `tmath agent-viewer`) so they
//! cannot drift from each other.
//!
//! `cjk_font` precedence: config file > the embedded default
//! (`tmath_render::CjkFont::default()`). No CLI flag or environment
//! variable — unlike font size, which genuinely varies per run/terminal,
//! there is currently exactly one embedded CJK family to choose from, so an
//! extra override layer would have nothing to override *to*. Add one only
//! when a second embedded family exists and per-run selection becomes a
//! real use case.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use tmath_render::CjkFont;

use crate::layout::{MAX_FONT_SIZE_PT, MIN_FONT_SIZE_PT};

/// Environment variable checked between the CLI flag and the config file.
const FONT_SIZE_ENV_VAR: &str = "TMATH_FONT_SIZE_PT";

/// Parsed, validated configuration. Every field is optional: an absent file,
/// an absent key, or a value that fails validation all resolve to `None`
/// here (fail closed to "no override" rather than propagating an error),
/// with the appropriate warning event already logged by [`load`].
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct Config {
    pub(crate) font_size_pt: Option<f64>,
    pub(crate) cjk_font: Option<CjkFont>,
}

/// Where the config file resolution and precedence decisions get their
/// source label from, for the numbers-only log event
/// (`resolve_font_size_pt_with_source`'s second return value).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FontSizeSource {
    Cli,
    Env,
    Config,
    AutoFit,
    Default,
}

impl FontSizeSource {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Cli => "cli",
            Self::Env => "env",
            Self::Config => "config",
            Self::AutoFit => "auto-fit",
            Self::Default => "default",
        }
    }
}

/// Resolves `$XDG_CONFIG_HOME/tmath/config.toml`, falling back to
/// `$HOME/.config/tmath/config.toml` — identical resolution order to
/// `agent_allowlist.rs::allowlist_path` (kept as a separate function rather
/// than shared code since the two files intentionally have unrelated
/// lifecycles and error handling: a missing/unwritable config directory
/// here silently disables config, whereas the allowlist path is load-bearing
/// for `agent-enable`/`agent-allowed`).
pub(crate) fn config_path() -> Option<PathBuf> {
    let base = match env::var_os("XDG_CONFIG_HOME") {
        Some(value) => PathBuf::from(value),
        None => PathBuf::from(env::var_os("HOME")?).join(".config"),
    };
    Some(base.join("tmath").join("config.toml"))
}

/// Loads and validates the config file at `path`. Fails closed at every
/// stage per AT-3-... (D-CONFIG): a missing file returns `Config::default()`
/// silently (this is the expected common case, not a warning); a file that
/// exists but fails to read, fails to parse as TOML, or is not a table all
/// log one stable-code warning event (never file content) and return
/// defaults; a recognized key whose value is present but fails validation
/// (wrong type or out of range) logs one warning event naming just the key
/// and is skipped (other valid keys still apply); an unrecognized key logs
/// one warning event naming just the key and is otherwise ignored.
pub(crate) fn load(path: &Path) -> Config {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Config::default(),
        Err(_) => {
            log_warning("config_read_failed", None);
            return Config::default();
        }
    };
    let table: toml::Table = match toml::from_str(&text) {
        Ok(table) => table,
        Err(_) => {
            log_warning("config_parse_failed", None);
            return Config::default();
        }
    };

    let mut config = Config::default();
    for (key, value) in &table {
        match key.as_str() {
            "font_size_pt" => match value
                .as_float()
                .or_else(|| value.as_integer().map(|v| v as f64))
            {
                Some(raw) if (MIN_FONT_SIZE_PT..=MAX_FONT_SIZE_PT).contains(&raw) => {
                    config.font_size_pt = Some(raw);
                }
                _ => log_warning("config_value_invalid", Some(key)),
            },
            "cjk_font" => match value.as_str().and_then(CjkFont::from_slug) {
                Some(font) => config.cjk_font = Some(font),
                None => log_warning("config_value_invalid", Some(key)),
            },
            _ => log_warning("config_key_unknown", Some(key)),
        }
    }
    config
}

/// Resolves the effective CJK font: the config file's `cjk_font` when
/// present and valid, otherwise the embedded default. No CLI/env layer — see
/// the module doc for why `font_size_pt`'s 4-level precedence does not apply
/// here yet.
pub(crate) fn resolve_cjk_font(config: &Config) -> CjkFont {
    config.cjk_font.unwrap_or_default()
}

/// Logs one bounded, content-free warning event to stderr: an event name
/// (stable code) and, for the per-key cases, the offending key's name only
/// — never the file's path or any value from it, per AGENTS.md's logging
/// rules (event/status names, not content).
fn log_warning(event: &'static str, key: Option<&str>) {
    match key {
        Some(key) => eprintln!("tmath: config warning={event} key={key}"),
        None => eprintln!("tmath: config warning={event}"),
    }
}

/// Parses `TMATH_FONT_SIZE_PT` the same way the config file's `font_size_pt`
/// is validated (clamped to `[MIN_FONT_SIZE_PT, MAX_FONT_SIZE_PT]`); an
/// unset, non-numeric, or out-of-range value is silently `None` (falls
/// through to the next precedence level), matching `TMATH_DPR`'s existing
/// "never error, only fall through" convention in `layout.rs`.
fn env_font_size_pt() -> Option<f64> {
    let raw = env::var(FONT_SIZE_ENV_VAR).ok()?;
    let value: f64 = raw.trim().parse().ok()?;
    (MIN_FONT_SIZE_PT..=MAX_FONT_SIZE_PT)
        .contains(&value)
        .then_some(value)
}

/// Resolves the effective font size in points across the full precedence
/// chain — CLI flag > `TMATH_FONT_SIZE_PT` env var > config file >
/// terminal auto-fit > fixed default — plus which level won, for a
/// numbers-only log event at the call site. `cli` is the caller's already-
/// parsed `--font-size` value (pixels, matching the existing flag's unit);
/// `config` is the already-loaded [`Config`] (loading it is the caller's
/// job, so callers that already have one open — e.g. the agent-viewer — do
/// not load it twice); `fitted` is the terminal auto-fit result, exactly as
/// `layout::resolve_font_size_pt` already takes it.
pub(crate) fn resolve_font_size_pt_with_source(
    cli: Option<u32>,
    config: &Config,
    fitted: Option<crate::layout::TerminalFitLayout>,
) -> (f64, FontSizeSource) {
    if let Some(cli) = cli {
        return (f64::from(cli), FontSizeSource::Cli);
    }
    if let Some(env) = env_font_size_pt() {
        return (env, FontSizeSource::Env);
    }
    if let Some(config) = config.font_size_pt {
        return (config, FontSizeSource::Config);
    }
    if let Some(fitted) = fitted {
        return (fitted.font_size_pt, FontSizeSource::AutoFit);
    }
    (crate::layout::DEFAULT_FONT_SIZE_PT, FontSizeSource::Default)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::TerminalFitLayout;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_config_path(contents: Option<&str>) -> PathBuf {
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = env::temp_dir().join(format!("tmath-config-test-{}-{}", std::process::id(), id));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        if let Some(contents) = contents {
            fs::write(&path, contents).unwrap();
        }
        path
    }

    fn fitted(font_size_pt: f64) -> TerminalFitLayout {
        TerminalFitLayout {
            content_width_pt: 480.0,
            font_size_pt,
            device_pixel_ratio: 1,
            effective_cell_px: (8, 16),
        }
    }

    #[test]
    fn missing_file_loads_silently_to_defaults() {
        let path = temp_config_path(None);
        assert_eq!(load(&path), Config::default());
    }

    #[test]
    fn valid_font_size_is_applied() {
        let path = temp_config_path(Some("font_size_pt = 18.0\n"));
        let config = load(&path);
        assert_eq!(config.font_size_pt, Some(18.0));
    }

    #[test]
    fn integer_font_size_is_accepted_as_a_float() {
        let path = temp_config_path(Some("font_size_pt = 18\n"));
        let config = load(&path);
        assert_eq!(config.font_size_pt, Some(18.0));
    }

    #[test]
    fn malformed_toml_falls_back_to_defaults() {
        let path = temp_config_path(Some("font_size_pt = [not valid\n"));
        assert_eq!(load(&path), Config::default());
    }

    #[test]
    fn out_of_range_value_is_rejected_and_other_keys_still_apply() {
        // 999 is outside [10, 24]; a sibling recognized key must still load.
        let path = temp_config_path(Some("font_size_pt = 999.0\n"));
        let config = load(&path);
        assert_eq!(config.font_size_pt, None);
    }

    #[test]
    fn wrong_type_value_is_rejected() {
        let path = temp_config_path(Some("font_size_pt = \"large\"\n"));
        let config = load(&path);
        assert_eq!(config.font_size_pt, None);
    }

    #[test]
    fn unknown_key_is_ignored_and_does_not_block_known_keys() {
        let path = temp_config_path(Some("font_size_pt = 16.0\nnonexistent_key = 1\n"));
        let config = load(&path);
        assert_eq!(config.font_size_pt, Some(16.0));
    }

    #[test]
    fn boundary_values_are_accepted() {
        let path = temp_config_path(Some("font_size_pt = 10.0\n"));
        assert_eq!(load(&path).font_size_pt, Some(10.0));
        let path = temp_config_path(Some("font_size_pt = 24.0\n"));
        assert_eq!(load(&path).font_size_pt, Some(24.0));
    }

    // --- D-CONFIG phase 2: cjk_font ---

    #[test]
    fn valid_cjk_font_slug_is_applied() {
        let path = temp_config_path(Some("cjk_font = \"m-plus-2\"\n"));
        let config = load(&path);
        assert_eq!(config.cjk_font, Some(CjkFont::MPlus2));
    }

    #[test]
    fn unknown_cjk_font_slug_is_rejected_and_falls_back_to_the_embedded_default() {
        let path = temp_config_path(Some("cjk_font = \"noto-sans-jp\"\n"));
        let config = load(&path);
        assert_eq!(
            config.cjk_font, None,
            "an unrecognized slug leaves the field unset"
        );
        assert_eq!(
            resolve_cjk_font(&config),
            CjkFont::default(),
            "resolve_cjk_font falls back to the embedded default"
        );
    }

    #[test]
    fn wrong_type_cjk_font_value_is_rejected() {
        let path = temp_config_path(Some("cjk_font = 2\n"));
        let config = load(&path);
        assert_eq!(config.cjk_font, None);
    }

    #[test]
    fn cjk_font_and_font_size_pt_can_both_be_set_together() {
        let path = temp_config_path(Some("font_size_pt = 16.0\ncjk_font = \"m-plus-2\"\n"));
        let config = load(&path);
        assert_eq!(config.font_size_pt, Some(16.0));
        assert_eq!(config.cjk_font, Some(CjkFont::MPlus2));
    }

    #[test]
    fn resolve_cjk_font_uses_the_embedded_default_when_unset() {
        assert_eq!(resolve_cjk_font(&Config::default()), CjkFont::default());
    }

    #[test]
    fn precedence_cli_beats_everything() {
        let config = Config {
            font_size_pt: Some(18.0),
            cjk_font: None,
        };
        let (value, source) =
            resolve_font_size_pt_with_source(Some(20), &config, Some(fitted(15.0)));
        assert_eq!(value, 20.0);
        assert_eq!(source, FontSizeSource::Cli);
    }

    #[test]
    fn precedence_config_beats_auto_fit() {
        let config = Config {
            font_size_pt: Some(18.0),
            cjk_font: None,
        };
        let (value, source) = resolve_font_size_pt_with_source(None, &config, Some(fitted(15.0)));
        assert_eq!(value, 18.0);
        assert_eq!(source, FontSizeSource::Config);
    }

    #[test]
    fn precedence_auto_fit_beats_default() {
        let config = Config::default();
        let (value, source) = resolve_font_size_pt_with_source(None, &config, Some(fitted(15.0)));
        assert_eq!(value, 15.0);
        assert_eq!(source, FontSizeSource::AutoFit);
    }

    #[test]
    fn precedence_default_is_last_resort() {
        let config = Config::default();
        let (value, source) = resolve_font_size_pt_with_source(None, &config, None);
        assert_eq!(value, crate::layout::DEFAULT_FONT_SIZE_PT);
        assert_eq!(source, FontSizeSource::Default);
    }
}
