---
name: teasr
description: >
  Capture screenshots and GIFs from web apps, terminals, and screens using teasr.
  Use this skill when you need to generate project showcase assets, capture terminal
  output as styled PNGs, take web page screenshots via Chrome DevTools Protocol, or
  configure teasr.toml for automated capture pipelines.
argument-hint: [--formats png,gif] [--output dir]
---

# teasr — Project Showcase Capture

Capture screenshots and GIFs from web apps, terminals, and screens.

## Steps

1. Ensure `teasr.toml` exists in the project root (or specify with `--config`):
   ```toml
   [output]
   dir = "./showcase"
   formats = ["png"]

   [[scenes]]
   type = "terminal"
   command = "my-cli --help"
   name = "cli-help"
   ```

2. Run: `teasr $ARGUMENTS`

3. Output files are written to the configured output directory.

## Scene Types

### Terminal
Runs a command in a PTY, captures ANSI output, renders to a styled PNG with terminal chrome.

```toml
[[scenes]]
type = "terminal"
command = "cargo test 2>&1"
name = "test-output"
theme = "dracula"     # or "monokai"
cols = 100
maxLines = 40
```

### Web
Navigates to a URL via Chrome DevTools Protocol. Requires Chrome/Chromium.

```toml
[[scenes]]
type = "web"
url = "/dashboard"
name = "dashboard"
viewport = { width = 1440, height = 900 }
```

### Screen
Captures a display or region using native screen capture.

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
| `--formats <FMT,...>` | Output formats: png, gif |
| `--verbose` | Enable debug logging |
| `--timeout <MS>` | Global timeout (default: 60000) |
