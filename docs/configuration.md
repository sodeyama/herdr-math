# Configuration

Terminal Math stores user settings in a single TOML file. The installer creates
it on first run; afterwards you edit it directly or use CLI commands that write
back to the same file.

## Location

```text
$XDG_CONFIG_HOME/tmath/config.toml
```

When `XDG_CONFIG_HOME` is unset, the path is `~/.config/tmath/config.toml`.

Install:

```sh
bash scripts/install.sh
# writes config.toml from config/config.toml.default when the file is missing
```

## Example

See [`config/config.toml.default`](../config/config.toml.default) for the full
commented template shipped with the repository.

```toml
font_size_pt = 16.0
cjk_font = "m-plus-2"
max_content_width_font_multiple = 28.0

[agent]
viewer_percent = 35
wait_ms = 600
poll_ms = 250
history_lines = 500
allowlist = []
```

## Keys

### Render and layout

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `font_size_pt` | number | auto-fit / 14 | Typeset font size (10–24 pt) |
| `cjk_font` | string | `"m-plus-2"` | Embedded CJK font slug |
| `max_content_width_font_multiple` | number | `28` | Caps auto-fit width as `font_size_pt × multiple` (10–60) |

**Font size precedence** (highest first): `tmath render --font-size` / `tmath watch
--font-size` → `TMATH_FONT_SIZE_PT` → `config.toml` → terminal auto-fit → fixed
default.

### Agent viewer and auto-watch

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `agent.viewer_percent` | integer | `35` | Viewer pane width (1–99) |
| `agent.wait_ms` | integer | `600` | Answer settle debounce (ms) |
| `agent.poll_ms` | integer | `250` | Source-pane poll interval (ms) |
| `agent.history_lines` | integer | `500` | Scrollback capture depth |
| `agent.allowlist` | string array | `[]` | Directories for shell auto-watch |
| `agent.device_pixel_ratio` | integer | unset | Optional tmux DPR override (1–4) |
| `agent.viewer_log` | boolean | unset | Show viewer diagnostics in the pane |
| `agent.tmux_transport` | string | unset | `client-tty` or `passthrough` |

**Agent CLI flags** (`--percent`, `--wait-ms`, `--poll-ms`, `--history`) override
the corresponding config defaults for that run.

**Environment variables** (`TMATH_FONT_SIZE_PT`, `TMATH_DPR`, `TMATH_VIEWER_LOG`,
`TMATH_TMUX_TRANSPORT`) override config when set.

### Allowlist

Shell auto-watch consults `[agent].allowlist`:

```sh
tmath agent-enable ~/projects/my-app   # appends canonical path to config.toml
tmath agent-disable ~/projects/my-app  # removes it
tmath agent-allowed                    # exit 0 when cwd is allowlisted
```

You may also edit `allowlist` by hand. A legacy `agent-allowlist` file in the
same directory is migrated into `config.toml` automatically on first load, then
removed.

## Privacy

The config file holds only small numeric and path settings. It never stores
document text, formulas, rendered bytes, or transcripts.
