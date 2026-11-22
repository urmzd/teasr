---
name: teasr
description: >
  Capture screenshots and GIFs from web apps, terminals, and screens using teasr.
  Use this skill when you need to generate project showcase assets, capture terminal
  output as styled PNGs, take web page screenshots via Chrome DevTools Protocol, or
  configure teasr.toml for automated capture pipelines.
metadata:
  argument-hint: "[--formats png,gif] [--output dir]"
---

# teasr — Project Showcase Capture

Capture screenshots and GIFs from web apps, terminals, and screens.

## Steps

1. Ensure `teasr.toml` exists in the project root (or specify with `--config`). `teasr init` writes a starter one; `teasr init --list` shows the templates (`terminal`, `web`, `markdown`). Or write it by hand:
   ```toml
   [output]
   dir = "./showcase"
   formats = [{ output_type = "png" }]

   [[scenes]]
   type = "terminal"
   name = "cli-help"

   [[scenes.interactions]]
   type = "type"
   text = "my-cli --help"

   [[scenes.interactions]]
   type = "key"
   key = "enter"

   [[scenes.interactions]]
   type = "wait"
   duration = 2000
   ```

2. Run: `teasr run $ARGUMENTS`

3. Output files are written to the configured output directory.

## Scene Types

### Terminal
Scripts an interactive PTY session, captures frames at each interaction, and renders them as animated GIFs or PNGs with terminal chrome.

```toml
[[scenes]]
type = "terminal"
name = "test-output"
theme = "dracula"     # or "monokai"
cols = 100
rows = 24

[[scenes.interactions]]
type = "type"
text = "cargo test 2>&1"
speed = 50

[[scenes.interactions]]
type = "key"
key = "enter"

[[scenes.interactions]]
type = "wait"
duration = 2000
```

### Web
Loads a remote URL, local file (HTML/SVG/PDF), or Markdown file via Chrome DevTools Protocol. Requires Chrome/Chromium. Type is auto-detected from `uri`.

```toml
# Remote URL (or /path if [server] is configured)
[[scenes]]
type = "web"
uri = "/dashboard"
name = "dashboard"
viewport = { width = 1440, height = 900 }

# Local HTML
[[scenes]]
type = "web"
uri = "./docs/preview.html"

# Markdown (auto-detected by .md extension)
[[scenes]]
type = "web"
uri = "./README.md"
theme = "dark"
```

### Screen
Captures a display or region using native screen capture.

**Warning:** screen captures record everything visible on the display, which may include PII (notifications, emails, other windows). Review the output before publishing; prefer a `window` or `region` capture over a full display, and configure `[redact]` to hide known-sensitive content.

## PII Redaction

Global `[redact]` applies to all scenes; `[scenes.redact]` extends it per scene.

```toml
[redact]
patterns = ["email", "secrets"]   # built-ins; "secrets" = all API-token patterns
username = true                   # mask $USER, hostname, home-dir paths

[scenes.redact]
selectors = [".user-email"]       # web: overlay-mask matching elements

[[scenes.redact.regions]]         # any scene: hide a pixel rectangle
x = 840
y = 12
width = 220
height = 32
style = "pixelate"                # "block" (default) | "blur" | "pixelate"
```

Terminal scenes mask matched text before rendering (custom regexes via `custom`, exact strings via `literals`). Secrets typed on camera still leak in intermediate frames — type them in `hidden = true` steps and let redaction cover program output.

```toml
[[scenes]]
type = "screen"
name = "native-app"
display = 0
setup = "open MyApp.app"
delay = 2000
```

## Server Configuration

Start a dev server before capture:

```toml
[server]
command = "npm run dev"
url = "http://localhost:3000"
timeout = 10000
```

## CLI Options

| Flag | Description |
|------|-------------|
| `-c, --config <PATH>` | Path to teasr.toml |
| `-o, --output <DIR>` | Output directory |
| `--formats <FMT,...>` | Output formats: png, gif, mp4 |
| `--verbose` | Enable debug logging |
| `--timeout <MS>` | Global timeout (default: 60000) |
| `--fps <N>` | Frames per second (overrides config) |
| `--seconds <N>` | Target output duration in seconds |
| `--scene-timeout <N>` | Per-scene wall-clock timeout in seconds |
| `--color-scheme <SCHEME>` | Emulated `prefers-color-scheme` for web scenes: `light` or `dark`. A scene's own `color_scheme` wins |
| `--format <human\|json>` | Result output. `json` prints one report on stdout: output dir, per-scene status, absolute file paths, and the error of each failed scene |
| `--scene <NAME>` | Only run scenes matching these names or types. Repeatable or comma-separated; alias `--scenes`. Errors if a name matches no scene |

## Other Commands

| Command | Use it to |
|---------|-----------|
| `teasr init [TEMPLATE]` | Write a starter `teasr.toml`. It keeps an existing file unless `--force` is set |
| `teasr doctor` | Find out why a capture cannot run: Chrome, font, config, skill, MCP registration. Each failed check prints its fix |
| `teasr fonts [NAME]` | List or install a font for terminal scenes |
| `teasr bootstrap` | Install this skill and register the MCP server in every AI tool, through agentspec |
| `teasr update` | Update the binary. Run `teasr bootstrap` after it to refresh this skill |

Every command accepts `--format json` and then prints one JSON document on stdout. Read `teasr doctor --format json` before you report that teasr is broken.

## MCP Server

`teasr mcp serve` serves one tool, `capture`, over MCP on stdio. Use it when an agent must produce captures without a `teasr.toml`.

```bash
teasr mcp serve --root ./site --output ./captures
```

- The tool takes `scenes` as JSON, with the `[[scenes]]` fields. Every scene needs a `name`. `formats` is a list of names: `["png", "gif"]`.
- The result is the `--format json` report, plus PNG outputs as images.
- `--allow` sets the scene types a caller may run. The default is `web`. `terminal` runs shell commands and `screen` records the display.
- A local file must be inside `--root`. Each call writes to a new directory under `--output`.
- `teasr mcp tools` prints the tool list and input schema as JSON. Read it instead of guessing an argument.
- `teasr bootstrap` registers the server. By hand, the client config is:

```json
{ "mcpServers": { "teasr": { "command": "teasr", "args": ["mcp", "serve"] } } }
```
