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
//!
//! `max_content_width_font_multiple` (D-CONFIG phase 3) caps how wide the
//! terminal auto-fit path (only) is allowed to stretch `content_width_pt`:
//! the effective cap is `font_size_pt * multiple`. Wide panes (200+ columns)
//! otherwise stretch math and prose to widths well past comfortable reading
//! measure, no matter how large the font — a textbook page doesn't get wider
//! just because the desk it sits on does. The default, 28, holds a 15pt
//! render to 420pt, matching a B5 textbook's printed text width (roughly
//! 397-425pt / 140-150mm) regardless of font size. An explicit
//! `--content-width` CLI value is never capped — it states an exact pixel
//! width, not a fitting preference — so this key only ever narrows the
//! auto-fit result.

use std::env;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde::Serialize;
use tmath_render::CjkFont;

use crate::layout::{MAX_FONT_SIZE_PT, MIN_FONT_SIZE_PT};

/// Environment variable checked between the CLI flag and the config file.
const FONT_SIZE_ENV_VAR: &str = "TMATH_FONT_SIZE_PT";

/// Valid range for `max_content_width_font_multiple`: from a pocket-book-like
/// measure (10x a 15pt font is 150pt, narrower than a paperback's ~227pt but
/// still a deliberately narrow column) up to a large-format technical book
/// (60x a 15pt font is 900pt). The default (28) sits in the middle of this
/// range, at B5 textbook width.
pub(crate) const MIN_CONTENT_WIDTH_FONT_MULTIPLE: f64 = 10.0;
pub(crate) const MAX_CONTENT_WIDTH_FONT_MULTIPLE: f64 = 60.0;
/// B5 textbook text width (~397-425pt / 140-150mm) at the 15pt font size the
/// terminal auto-fit path typically resolves to.
pub(crate) const DEFAULT_CONTENT_WIDTH_FONT_MULTIPLE: f64 = 28.0;

/// Default agent viewer split width (percent).
pub(crate) const DEFAULT_AGENT_VIEWER_PERCENT: u32 = 35;
/// Default answer settle debounce (ms).
pub(crate) const DEFAULT_AGENT_WAIT_MS: u64 = 600;
/// Default source-pane poll interval (ms).
pub(crate) const DEFAULT_AGENT_POLL_MS: u64 = 250;
/// Default scrollback capture depth (lines).
pub(crate) const DEFAULT_AGENT_HISTORY_LINES: u32 = 500;

const LEGACY_ALLOWLIST_FILE: &str = "agent-allowlist";
const TMUX_TRANSPORT_ENV_VAR: &str = "TMATH_TMUX_TRANSPORT";
const VIEWER_LOG_ENV_VAR: &str = "TMATH_VIEWER_LOG";
const DPR_ENV_VAR: &str = "TMATH_DPR";

/// Parsed, validated `[agent]` settings. Unset fields fall back to the
/// documented defaults at resolution time.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct AgentConfig {
    pub viewer_percent: Option<u32>,
    pub wait_ms: Option<u64>,
    pub poll_ms: Option<u64>,
    pub history_lines: Option<u32>,
    pub device_pixel_ratio: Option<u32>,
    pub viewer_log: Option<bool>,
    pub tmux_transport: Option<String>,
    pub allowlist: Vec<PathBuf>,
}

/// Parsed, validated configuration. Every field is optional: an absent file,
/// an absent key, or a value that fails validation all resolve to `None`
/// here (fail closed to "no override" rather than propagating an error),
/// with the appropriate warning event already logged by [`load`].
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Config {
    pub(crate) font_size_pt: Option<f64>,
    pub(crate) cjk_font: Option<CjkFont>,
    pub(crate) max_content_width_font_multiple: Option<f64>,
    pub(crate) agent: AgentConfig,
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

/// Resolves the platform config directory (`$XDG_CONFIG_HOME/tmath` or
/// `$HOME/.config/tmath`).
pub(crate) fn config_dir() -> Option<PathBuf> {
    let base = match env::var_os("XDG_CONFIG_HOME") {
        Some(value) => PathBuf::from(value),
        None => PathBuf::from(env::var_os("HOME")?).join(".config"),
    };
    Some(base.join("tmath"))
}

/// Resolves `$XDG_CONFIG_HOME/tmath/config.toml`, falling back to
/// `$HOME/.config/tmath/config.toml` — identical resolution order to the
/// historical allowlist path's parent directory.
pub(crate) fn config_path() -> Option<PathBuf> {
    Some(config_dir()?.join("config.toml"))
}

/// Historical allowlist file kept for one-time migration into `config.toml`.
pub(crate) fn legacy_allowlist_path() -> Option<PathBuf> {
    Some(config_dir()?.join(LEGACY_ALLOWLIST_FILE))
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
            "max_content_width_font_multiple" => match value
                .as_float()
                .or_else(|| value.as_integer().map(|v| v as f64))
            {
                Some(raw)
                    if (MIN_CONTENT_WIDTH_FONT_MULTIPLE..=MAX_CONTENT_WIDTH_FONT_MULTIPLE)
                        .contains(&raw) =>
                {
                    config.max_content_width_font_multiple = Some(raw);
                }
                _ => log_warning("config_value_invalid", Some(key)),
            },
            "agent" => parse_agent_table(value, &mut config.agent),
            _ => log_warning("config_key_unknown", Some(key)),
        }
    }
    config
}

fn parse_agent_table(value: &toml::Value, agent: &mut AgentConfig) {
    let Some(table) = value.as_table() else {
        log_warning("config_value_invalid", Some("agent"));
        return;
    };
    for (key, value) in table {
        match key.as_str() {
            "viewer_percent" => match value.as_integer().and_then(|v| u32::try_from(v).ok()) {
                Some(raw) if (1..=99).contains(&raw) => agent.viewer_percent = Some(raw),
                _ => log_warning("config_value_invalid", Some("agent.viewer_percent")),
            },
            "wait_ms" => match value.as_integer().and_then(|v| u64::try_from(v).ok()) {
                Some(raw) if raw > 0 => agent.wait_ms = Some(raw),
                _ => log_warning("config_value_invalid", Some("agent.wait_ms")),
            },
            "poll_ms" => match value.as_integer().and_then(|v| u64::try_from(v).ok()) {
                Some(raw) if raw > 0 => agent.poll_ms = Some(raw),
                _ => log_warning("config_value_invalid", Some("agent.poll_ms")),
            },
            "history_lines" => match value.as_integer().and_then(|v| u32::try_from(v).ok()) {
                Some(raw) if raw > 0 => agent.history_lines = Some(raw),
                _ => log_warning("config_value_invalid", Some("agent.history_lines")),
            },
            "device_pixel_ratio" => match value.as_integer().and_then(|v| u32::try_from(v).ok()) {
                Some(raw) if (1..=4).contains(&raw) => agent.device_pixel_ratio = Some(raw),
                _ => log_warning("config_value_invalid", Some("agent.device_pixel_ratio")),
            },
            "viewer_log" => match value.as_bool() {
                Some(raw) => agent.viewer_log = Some(raw),
                None => log_warning("config_value_invalid", Some("agent.viewer_log")),
            },
            "tmux_transport" => match value.as_str() {
                Some("client-tty") | Some("passthrough") => {
                    agent.tmux_transport = Some(value.as_str().unwrap().to_string());
                }
                _ => log_warning("config_value_invalid", Some("agent.tmux_transport")),
            },
            "allowlist" => match value.as_array() {
                Some(items) => {
                    agent.allowlist = items
                        .iter()
                        .filter_map(|item| {
                            item.as_str()
                                .map(|raw| normalize_allowlist_path(Path::new(raw)))
                        })
                        .collect();
                }
                None => log_warning("config_value_invalid", Some("agent.allowlist")),
            },
            _ => log_warning("config_key_unknown", Some(key)),
        }
    }
}

/// Loads the active config file, migrating a legacy `agent-allowlist` file into
/// `[agent].allowlist` when present.
pub(crate) fn load_active() -> Config {
    let Some(path) = config_path() else {
        return Config::default();
    };
    let mut config = load(&path);
    if config.agent.allowlist.is_empty() {
        if let Some(legacy_path) = legacy_allowlist_path() {
            if let Ok(entries) = read_allowlist_lines(&legacy_path) {
                if !entries.is_empty() {
                    config.agent.allowlist = entries;
                    if save(&path, &config).is_ok() {
                        let _ = fs::remove_file(&legacy_path);
                    }
                }
            }
        }
    }
    if normalize_allowlist_entries(&mut config) {
        let _ = save(&path, &config);
    }
    config
}

/// Writes `config` to `path`, creating the parent directory when needed.
pub(crate) fn save(path: &Path, config: &Config) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "config path has no parent directory".to_string())?;
    fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))?;

    let document = ConfigDocument::from_config(config);
    let text = toml::to_string_pretty(&document)
        .map_err(|error| format!("config serialize: {error}"))?;

    let mut open_options = fs::OpenOptions::new();
    open_options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        open_options.mode(0o600);
    }
    let mut file = open_options
        .open(path)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    file.write_all(text.as_bytes())
        .map_err(|error| format!("{}: {error}", path.display()))
}

fn read_allowlist_lines(path: &Path) -> Result<Vec<PathBuf>, String> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(text
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .map(PathBuf::from)
            .map(|entry| normalize_allowlist_path(&entry))
            .collect()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(format!("{}: {error}", path.display())),
    }
}

/// Adds `dir` to the active config allowlist unless already present.
pub(crate) fn enable_allowlist_dir(dir: &Path) -> Result<bool, String> {
    let path = config_path().ok_or_else(|| "config path unavailable".to_string())?;
    let mut config = load_active();
    if config.agent.allowlist.iter().any(|entry| entry == dir) {
        return Ok(false);
    }
    config.agent.allowlist.push(dir.to_path_buf());
    save(&path, &config)?;
    Ok(true)
}

/// Removes `dir` from the active config allowlist (exact match).
pub(crate) fn disable_allowlist_dir(dir: &Path) -> Result<bool, String> {
    let path = config_path().ok_or_else(|| "config path unavailable".to_string())?;
    let mut config = load_active();
    let before = config.agent.allowlist.len();
    config.agent.allowlist.retain(|entry| entry != dir);
    if config.agent.allowlist.len() == before {
        return Ok(false);
    }
    save(&path, &config)?;
    Ok(true)
}

/// Returns whether `dir` is within any configured allowlist entry.
pub(crate) fn is_dir_allowlisted(dir: &Path) -> bool {
    let dir = dir
        .canonicalize()
        .unwrap_or_else(|_| normalize_allowlist_path(dir));
    let config = load_active();
    config
        .agent
        .allowlist
        .iter()
        .any(|base| dir.starts_with(base))
}

/// Expands a leading `~` or `~/…` using `$HOME`. Other paths are returned as-is.
pub(crate) fn expand_tilde_path(path: &Path) -> PathBuf {
    let raw = path.to_string_lossy();
    if raw == "~" {
        return env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| path.to_path_buf());
    }
    if let Some(rest) = raw.strip_prefix("~/") {
        if let Some(home) = env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    path.to_path_buf()
}

/// Resolves an allowlist entry for matching: expand `~`, then canonicalize when
/// the path exists (manual `config.toml` edits often use `~/…` literals).
fn normalize_allowlist_path(path: &Path) -> PathBuf {
    let expanded = expand_tilde_path(path);
    expanded.canonicalize().unwrap_or(expanded)
}

fn normalize_allowlist_entries(config: &mut Config) -> bool {
    let normalized: Vec<PathBuf> = config
        .agent
        .allowlist
        .iter()
        .map(|entry| normalize_allowlist_path(entry))
        .collect();
    if normalized == config.agent.allowlist {
        return false;
    }
    config.agent.allowlist = normalized;
    true
}

pub(crate) fn resolve_tmux_transport(config: &AgentConfig) -> Option<String> {
    env::var(TMUX_TRANSPORT_ENV_VAR)
        .ok()
        .filter(|value| value == "client-tty" || value == "passthrough")
        .or_else(|| config.tmux_transport.clone())
}

pub(crate) fn resolve_device_pixel_ratio_config(config: &AgentConfig) -> Option<u32> {
    env::var(DPR_ENV_VAR)
        .ok()
        .and_then(|raw| raw.trim().parse().ok())
        .filter(|value| (1..=4).contains(value))
        .or(config.device_pixel_ratio)
}

pub(crate) fn resolve_viewer_log_config(config: &AgentConfig) -> Option<bool> {
    env::var(VIEWER_LOG_ENV_VAR)
        .ok()
        .map(|raw| matches!(raw.trim(), "1" | "true" | "yes" | "on"))
        .or(config.viewer_log)
}

/// Precedence: environment variable > config file > `None` (viewer auto-fit).
pub(crate) fn resolve_font_size_pt_env_or_config(config: &Config) -> Option<f64> {
    env_font_size_pt().or(config.font_size_pt)
}

#[derive(Serialize)]
struct ConfigDocument {
    #[serde(skip_serializing_if = "Option::is_none")]
    font_size_pt: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cjk_font: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_content_width_font_multiple: Option<f64>,
    agent: AgentConfigDocument,
}

#[derive(Serialize)]
struct AgentConfigDocument {
    viewer_percent: u32,
    wait_ms: u64,
    poll_ms: u64,
    history_lines: u32,
    allowlist: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    device_pixel_ratio: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    viewer_log: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tmux_transport: Option<String>,
}

impl ConfigDocument {
    fn from_config(config: &Config) -> Self {
        Self {
            font_size_pt: config.font_size_pt,
            cjk_font: config.cjk_font.map(|font| font.slug()),
            max_content_width_font_multiple: config.max_content_width_font_multiple,
            agent: AgentConfigDocument {
                viewer_percent: config
                    .agent
                    .viewer_percent
                    .unwrap_or(DEFAULT_AGENT_VIEWER_PERCENT),
                wait_ms: config.agent.wait_ms.unwrap_or(DEFAULT_AGENT_WAIT_MS),
                poll_ms: config.agent.poll_ms.unwrap_or(DEFAULT_AGENT_POLL_MS),
                history_lines: config
                    .agent
                    .history_lines
                    .unwrap_or(DEFAULT_AGENT_HISTORY_LINES),
                allowlist: config
                    .agent
                    .allowlist
                    .iter()
                    .map(|entry| entry.display().to_string())
                    .collect(),
                device_pixel_ratio: config.agent.device_pixel_ratio,
                viewer_log: config.agent.viewer_log,
                tmux_transport: config.agent.tmux_transport.clone(),
            },
        }
    }
}

/// Resolves the effective CJK font: the config file's `cjk_font` when
/// present and valid, otherwise the embedded default. No CLI/env layer — see
/// the module doc for why `font_size_pt`'s 4-level precedence does not apply
/// here yet.
pub(crate) fn resolve_cjk_font(config: &Config) -> CjkFont {
    config.cjk_font.unwrap_or_default()
}

/// Resolves the effective `max_content_width_font_multiple`: the config
/// file's value when present and valid, otherwise
/// [`DEFAULT_CONTENT_WIDTH_FONT_MULTIPLE`]. No CLI/env layer — an explicit
/// `--content-width` bypasses this cap entirely rather than needing its own
/// override (see the module doc).
pub(crate) fn resolve_max_content_width_font_multiple(config: &Config) -> f64 {
    config
        .max_content_width_font_multiple
        .unwrap_or(DEFAULT_CONTENT_WIDTH_FONT_MULTIPLE)
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
    use std::path::PathBuf;
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

    // --- D-CONFIG phase 3: max_content_width_font_multiple ---

    #[test]
    fn valid_content_width_multiple_is_applied() {
        let path = temp_config_path(Some("max_content_width_font_multiple = 20.0\n"));
        let config = load(&path);
        assert_eq!(config.max_content_width_font_multiple, Some(20.0));
    }

    #[test]
    fn integer_content_width_multiple_is_accepted_as_a_float() {
        let path = temp_config_path(Some("max_content_width_font_multiple = 20\n"));
        let config = load(&path);
        assert_eq!(config.max_content_width_font_multiple, Some(20.0));
    }

    #[test]
    fn out_of_range_content_width_multiple_is_rejected() {
        let path = temp_config_path(Some("max_content_width_font_multiple = 5.0\n"));
        assert_eq!(load(&path).max_content_width_font_multiple, None);
        let path = temp_config_path(Some("max_content_width_font_multiple = 200.0\n"));
        assert_eq!(load(&path).max_content_width_font_multiple, None);
    }

    #[test]
    fn wrong_type_content_width_multiple_is_rejected() {
        let path = temp_config_path(Some("max_content_width_font_multiple = \"wide\"\n"));
        assert_eq!(load(&path).max_content_width_font_multiple, None);
    }

    #[test]
    fn content_width_multiple_boundary_values_are_accepted() {
        let path = temp_config_path(Some("max_content_width_font_multiple = 10.0\n"));
        assert_eq!(load(&path).max_content_width_font_multiple, Some(10.0));
        let path = temp_config_path(Some("max_content_width_font_multiple = 60.0\n"));
        assert_eq!(load(&path).max_content_width_font_multiple, Some(60.0));
    }

    #[test]
    fn resolve_max_content_width_font_multiple_falls_back_to_the_b5_default_when_unset() {
        assert_eq!(
            resolve_max_content_width_font_multiple(&Config::default()),
            DEFAULT_CONTENT_WIDTH_FONT_MULTIPLE
        );
    }

    #[test]
    fn resolve_max_content_width_font_multiple_uses_the_configured_value() {
        let config = Config {
            font_size_pt: None,
            cjk_font: None,
            max_content_width_font_multiple: Some(40.0),
            agent: AgentConfig::default(),
        };
        assert_eq!(resolve_max_content_width_font_multiple(&config), 40.0);
    }

    #[test]
    fn precedence_cli_beats_everything() {
        let config = Config {
            font_size_pt: Some(18.0),
            cjk_font: None,
            max_content_width_font_multiple: None,
            agent: AgentConfig::default(),
        };
        let (value, source) =
            resolve_font_size_pt_with_source(Some(20), &config, Some(fitted(15.0)));
        assert_eq!(value, 20.0);
        assert_eq!(source, FontSizeSource::Cli);
    }

    #[test]
    fn agent_table_parses_viewer_and_allowlist_settings() {
        let path = temp_config_path(Some(
            "[agent]\nviewer_percent = 45\nwait_ms = 500\nallowlist = [\"/tmp/proj\"]\n",
        ));
        let config = load(&path);
        assert_eq!(config.agent.viewer_percent, Some(45));
        assert_eq!(config.agent.wait_ms, Some(500));
        assert_eq!(config.agent.allowlist, vec![PathBuf::from("/tmp/proj")]);
    }

    #[test]
    fn save_round_trips_allowlist_entries() {
        let path = temp_config_path(None);
        let mut config = Config::default();
        config.agent.allowlist = vec![PathBuf::from("/tmp/a")];
        save(&path, &config).unwrap();
        let loaded = load(&path);
        assert_eq!(loaded.agent.allowlist, vec![PathBuf::from("/tmp/a")]);
    }

    #[test]
    fn allowlist_expands_tilde_paths_from_config() {
        let home = env::var("HOME").expect("HOME");
        let path = temp_config_path(Some(
            "[agent]\nallowlist = [\"~/docs/obsidian\"]\n",
        ));
        let config = load(&path);
        assert_eq!(
            config.agent.allowlist,
            vec![PathBuf::from(format!("{home}/docs/obsidian"))]
        );
    }

    #[test]
    fn precedence_config_beats_auto_fit() {
        let config = Config {
            font_size_pt: Some(18.0),
            cjk_font: None,
            max_content_width_font_multiple: None,
            agent: AgentConfig::default(),
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
