//! `teasr mcp`: a Model Context Protocol server on stdio with one tool, `capture`.
//!
//! The transport is newline-delimited JSON-RPC 2.0: one message per line on
//! stdin, one per line on stdout. Nothing else may be written to stdout.
//!
//! The tool takes scenes in the same shape as `teasr.toml`, as JSON. What a
//! caller may do is fixed when the server starts, not per call:
//!
//! - only the scene types named by `--allow` run (default: `web`),
//! - a local file a web scene loads must be inside `--root`,
//! - output goes to a new directory under `--output` for every call.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use base64::Engine;
use serde::Deserialize;
use serde_json::{json, Value};
use teasr_core::types::{
    ColorScheme, GifConfig, OutputConfig, OutputFormat, PngConfig, RunReport, SceneConfig,
    TeaseConfig, ViewportConfig,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// Protocol revisions this server speaks, newest first.
const PROTOCOL_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

/// PNG outputs are returned inline so the caller's model can look at them.
/// Larger or further images are reported by path only.
const MAX_INLINE_IMAGES: usize = 6;
const MAX_INLINE_IMAGE_BYTES: u64 = 2 * 1024 * 1024;

const SCENE_TYPES: [&str; 3] = ["web", "terminal", "screen"];

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;

pub struct Options {
    /// Scene types a caller may run.
    pub allow: Vec<String>,
    /// Local files a web scene loads must be inside this directory.
    pub root: PathBuf,
    /// Every call writes to a new directory under this one.
    pub output: PathBuf,
    /// Wall-clock limit for one `capture` call.
    pub timeout: Duration,
}

pub struct Server {
    allow: HashSet<String>,
    root: PathBuf,
    output: PathBuf,
    timeout: Duration,
    calls: AtomicU64,
}

/// Arguments of the `capture` tool. Scenes stay raw JSON until they pass the
/// checks in [`Server::scenes`].
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CaptureArgs {
    scenes: Vec<Value>,
    formats: Option<Vec<String>>,
    color_scheme: Option<ColorScheme>,
    viewport: Option<ViewportConfig>,
    #[serde(default = "default_true")]
    inline_images: bool,
}

fn default_true() -> bool {
    true
}

impl Server {
    pub fn new(opts: Options) -> Result<Self> {
        let allow = parse_allow(opts.allow)?;
        let root = std::fs::canonicalize(&opts.root)
            .with_context(|| format!("--root is not a directory: {}", opts.root.display()))?;
        std::fs::create_dir_all(&opts.output)
            .with_context(|| format!("failed to create --output: {}", opts.output.display()))?;
        let output = std::fs::canonicalize(&opts.output)?;
        Ok(Self {
            allow,
            root,
            output,
            timeout: opts.timeout,
            calls: AtomicU64::new(0),
        })
    }

    /// Serve until stdin closes.
    pub async fn serve(&self) -> Result<()> {
        let mut lines = BufReader::new(tokio::io::stdin()).lines();
        let mut stdout = tokio::io::stdout();
        while let Some(line) = lines.next_line().await? {
            if line.trim().is_empty() {
                continue;
            }
            if let Some(reply) = self.handle_line(&line).await {
                stdout.write_all(reply.to_string().as_bytes()).await?;
                stdout.write_all(b"\n").await?;
                stdout.flush().await?;
            }
        }
        Ok(())
    }

    /// Answer one JSON-RPC message. `None` for a notification, which gets no reply.
    pub async fn handle_line(&self, line: &str) -> Option<Value> {
        let msg: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => {
                return Some(error(
                    Value::Null,
                    PARSE_ERROR,
                    &format!("parse error: {e}"),
                ))
            }
        };
        let id = msg.get("id").cloned();
        let Some(method) = msg.get("method").and_then(Value::as_str) else {
            // A response to a request this server never sent, or junk.
            return id.map(|id| error(id, INVALID_REQUEST, "message has no method"));
        };
        // No id means a notification: act on nothing and stay silent.
        let id = id?;
        let params = msg.get("params").cloned().unwrap_or(Value::Null);

        Some(match method {
            "initialize" => result(id, self.initialize(&params)),
            "ping" => result(id, json!({})),
            "tools/list" => result(id, json!({ "tools": [self.tool()] })),
            "tools/call" => match params.get("name").and_then(Value::as_str) {
                Some("capture") => {
                    let args = params.get("arguments").cloned().unwrap_or(json!({}));
                    result(id, self.capture(args).await)
                }
                other => error(
                    id,
                    INVALID_PARAMS,
                    &format!("unknown tool: {}", other.unwrap_or("(none)")),
                ),
            },
            other => error(id, METHOD_NOT_FOUND, &format!("method not found: {other}")),
        })
    }

    fn initialize(&self, params: &Value) -> Value {
        let asked = params.get("protocolVersion").and_then(Value::as_str);
        let version = asked
            .filter(|v| PROTOCOL_VERSIONS.contains(v))
            .unwrap_or(PROTOCOL_VERSIONS[0]);
        json!({
            "protocolVersion": version,
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "teasr", "version": env!("CARGO_PKG_VERSION") },
        })
    }

    fn allowed(&self) -> Vec<&str> {
        SCENE_TYPES
            .into_iter()
            .filter(|t| self.allow.contains(*t))
            .collect()
    }

    /// The `capture` tool definition. The scene `type` enum lists only the
    /// allowed types, so the schema (and any hash pinned on it) changes with
    /// `--allow`.
    fn tool(&self) -> Value {
        tool_definition(&self.allowed())
    }
}

/// Check the `--allow` values.
fn parse_allow(allow: Vec<String>) -> Result<HashSet<String>> {
    let mut set = HashSet::new();
    for kind in allow {
        if !SCENE_TYPES.contains(&kind.as_str()) {
            anyhow::bail!(
                "unknown scene type '{kind}' for --allow (use {})",
                SCENE_TYPES.join(", ")
            );
        }
        set.insert(kind);
    }
    Ok(set)
}

/// What `tools/list` returns for these `--allow` values. `teasr mcp tools`
/// prints it, so the schema can be read and pinned without a client.
pub fn tools_list(allow: Vec<String>) -> Result<Value> {
    let allow = parse_allow(allow)?;
    let allowed: Vec<&str> = SCENE_TYPES
        .into_iter()
        .filter(|t| allow.contains(*t))
        .collect();
    Ok(json!({ "tools": [tool_definition(&allowed)] }))
}

fn tool_definition(allowed: &[&str]) -> Value {
    {
        // Built once and repeated inline: some clients reject `$ref` in a
        // tool schema.
        let formats = json!({
            "type": "array",
            "minItems": 1,
            "items": { "type": "string", "enum": ["png", "gif"] },
            "description": "Output file types. Default: png. A scene's own value wins over the call's."
        });
        let color_scheme = json!({
            "type": "string",
            "enum": ["light", "dark"],
            "description": "Emulated prefers-color-scheme for web scenes. Unset, pages follow the server's machine. A scene's own value wins over the call's."
        });
        let viewport = json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "width": { "type": "integer", "minimum": 1, "default": 1280 },
                "height": { "type": "integer", "minimum": 1, "default": 720 }
            },
            "description": "Web scenes: browser viewport in CSS pixels."
        });
        json!({
            "name": "capture",
            "title": "Capture screenshots and GIFs",
            "description": format!(
                "Capture PNG screenshots or animated GIFs from scripted scenes and return the \
                 files written. A web scene loads an http(s) URL, or a local HTML, SVG, PDF or \
                 Markdown file, in headless Chrome and replays its interactions. Add a \
                 `snapshot` interaction for every frame to keep; a scene with no snapshot gets \
                 one frame at the end. Allowed scene types: {}. Local files must be inside the \
                 server's root directory. PNG results are also returned as images.",
                allowed.join(", ")
            ),
            // It writes new files under --output and loads URLs. It changes
            // nothing else, and a second call writes a second directory.
            "annotations": {
                "readOnlyHint": false,
                "destructiveHint": false,
                "idempotentHint": false,
                "openWorldHint": true
            },
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "required": ["scenes"],
                "properties": {
                    "scenes": {
                        "type": "array",
                        "minItems": 1,
                        "description": "Scenes to capture, in order. A scene that fails while it runs does not stop the others.",
                        "items": {
                            "type": "object",
                            "required": ["type", "name"],
                            "properties": {
                                "type": { "type": "string", "enum": allowed },
                                "name": {
                                    "type": "string",
                                    "pattern": "^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$",
                                    "description": "Output file name without extension. Unique in the call."
                                },
                                "uri": {
                                    "type": "string",
                                    "description": "Web scenes, required: an http(s) URL, or a path to a local .html, .svg, .pdf, .md or .markdown file."
                                },
                                "viewport": viewport,
                                "full_page": {
                                    "type": "boolean",
                                    "description": "Web scenes: capture the full page height, not only the viewport."
                                },
                                "color_scheme": color_scheme,
                                "page": {
                                    "type": "integer",
                                    "minimum": 1,
                                    "description": "Web scenes on a PDF: page to capture, from 1."
                                },
                                "theme": {
                                    "type": "string",
                                    "description": "Markdown: light or dark. Terminal and screen: dracula or monokai."
                                },
                                "formats": formats,
                                "frame_duration": {
                                    "type": "integer",
                                    "minimum": 1,
                                    "description": "Milliseconds each frame shows in a GIF."
                                },
                                "command": {
                                    "type": "string",
                                    "description": "Terminal scenes: run this command in the terminal instead of a shell."
                                },
                                "cols": { "type": "integer", "minimum": 1, "description": "Terminal scenes: width in columns." },
                                "rows": { "type": "integer", "minimum": 1, "description": "Terminal scenes: height in rows." },
                                "interactions": {
                                    "type": "array",
                                    "description": "Steps replayed in order.",
                                    "items": {
                                        "type": "object",
                                        "required": ["type"],
                                        "properties": {
                                            "type": {
                                                "type": "string",
                                                "enum": ["snapshot", "click", "hover", "scroll-to", "wait", "type", "key"]
                                            },
                                            "selector": { "type": "string", "description": "click, hover, scroll-to: CSS selector." },
                                            "duration": { "type": "integer", "minimum": 0, "description": "wait: milliseconds. Default 1000." },
                                            "text": { "type": "string", "description": "type: the text to type." },
                                            "speed": { "type": "integer", "minimum": 0, "description": "type: milliseconds per character." },
                                            "key": { "type": "string", "description": "key: the key to press, for example enter." },
                                            "hidden": { "type": "boolean", "description": "Run the step but keep its frames out of the output." }
                                        }
                                    }
                                }
                            }
                        }
                    },
                    "formats": formats,
                    "color_scheme": color_scheme,
                    "viewport": viewport,
                    "inline_images": {
                        "type": "boolean",
                        "default": true,
                        "description": format!(
                            "Return PNG outputs as images too: at most {MAX_INLINE_IMAGES}, each at most {} MiB.",
                            MAX_INLINE_IMAGE_BYTES / (1024 * 1024)
                        )
                    }
                }
            }
        })
    }
}

impl Server {
    /// Run the tool. Every failure a caller can cause comes back as a tool
    /// result with `isError`, so the model can read it and try again.
    async fn capture(&self, args: Value) -> Value {
        match self.run_capture(args).await {
            Ok((report, inline)) => {
                let mut content = vec![json!({
                    "type": "text",
                    "text": serde_json::to_string_pretty(&report).unwrap_or_default(),
                })];
                if inline {
                    content.extend(inline_images(&report));
                }
                json!({
                    "content": content,
                    "structuredContent": report,
                    "isError": report.all_failed(),
                })
            }
            Err(e) => json!({
                "content": [{ "type": "text", "text": format!("{e:#}") }],
                "isError": true,
            }),
        }
    }

    async fn run_capture(&self, args: Value) -> Result<(RunReport, bool)> {
        let args: CaptureArgs =
            serde_json::from_value(args).context("invalid arguments for capture")?;
        let scenes = self.scenes(args.scenes)?;
        let formats = match args.formats {
            Some(formats) => parse_formats(&formats)?,
            None => vec![OutputFormat::Png(PngConfig::default())],
        };

        let config = TeaseConfig {
            scenes,
            server: None,
            viewport: args.viewport,
            color_scheme: args.color_scheme,
            output: Some(OutputConfig {
                dir: self.call_dir().to_string_lossy().into_owned(),
                formats,
            }),
            font: None,
            fps: None,
            seconds: None,
            scene_timeout: None,
            outro_hold_ms: None,
            redact: None,
        }
        .resolve();

        let report =
            tokio::time::timeout(self.timeout, teasr_core::orchestrator::run_report(&config))
                .await
                .map_err(|_| {
                    anyhow::anyhow!("capture timed out after {}ms", self.timeout.as_millis())
                })??;
        Ok((report, args.inline_images))
    }

    /// A directory under `--output` that no earlier call used.
    fn call_dir(&self) -> PathBuf {
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or_default();
        let n = self.calls.fetch_add(1, Ordering::Relaxed);
        self.output.join(format!("capture-{millis}-{n}"))
    }

    /// Check raw scenes against what this server allows, then parse them.
    /// Checked on the raw JSON so that a scene of a type that is not allowed
    /// is refused before any of its fields are read.
    fn scenes(&self, raw: Vec<Value>) -> Result<Vec<SceneConfig>> {
        if raw.is_empty() {
            anyhow::bail!("scenes must have at least one scene");
        }
        let mut names = HashSet::new();
        let mut scenes = Vec::with_capacity(raw.len());
        for (i, mut scene) in raw.into_iter().enumerate() {
            let at = format!("scenes[{i}]");
            let kind = scene
                .get("type")
                .and_then(Value::as_str)
                .with_context(|| format!("{at}: type is required"))?
                .to_string();
            if !self.allow.contains(&kind) {
                anyhow::bail!(
                    "{at}: scene type '{kind}' is not allowed on this server (allowed: {})",
                    self.allowed().join(", ")
                );
            }
            let name = scene
                .get("name")
                .and_then(Value::as_str)
                .with_context(|| format!("{at}: name is required"))?
                .to_string();
            if !valid_name(&name) {
                anyhow::bail!(
                    "{at}: name '{name}' must be 1 to 64 letters, digits, '.', '_' or '-', and start with a letter or digit"
                );
            }
            if !names.insert(name.clone()) {
                anyhow::bail!("{at}: name '{name}' is used by an earlier scene");
            }
            // The tool takes format names; teasr.toml takes tagged objects.
            if let Some(formats) = scene.get("formats").cloned() {
                let names: Vec<String> = serde_json::from_value(formats)
                    .with_context(|| format!("{at}: formats must be a list of format names"))?;
                let parsed = parse_formats(&names).context(at.clone())?;
                scene["formats"] = serde_json::to_value(parsed)?;
            }
            if kind == "web" {
                let uri = scene
                    .get("uri")
                    .and_then(Value::as_str)
                    .with_context(|| format!("{at}: uri is required for a web scene"))?
                    .to_string();
                if !is_remote(&uri) {
                    scene["uri"] = Value::String(self.local_path(&uri).context(at.clone())?);
                }
                for field in ["stylesheet", "template"] {
                    if let Some(path) = scene.get(field).and_then(Value::as_str) {
                        let path = path.to_string();
                        scene[field] = Value::String(self.local_path(&path).context(at.clone())?);
                    }
                }
            }
            scenes.push(
                serde_json::from_value(scene).with_context(|| format!("{at}: invalid scene"))?,
            );
        }
        Ok(scenes)
    }

    /// Resolve a local path against the root and refuse one that leaves it.
    /// Symlinks are followed first, so a link inside the root cannot point out.
    fn local_path(&self, path: &str) -> Result<String> {
        let trimmed = path.strip_prefix("file://").unwrap_or(path);
        let joined = self.root.join(trimmed);
        let resolved =
            std::fs::canonicalize(&joined).with_context(|| format!("file not found: {path}"))?;
        if !resolved.starts_with(&self.root) {
            anyhow::bail!("{path} is outside the server root {}", self.root.display());
        }
        Ok(resolved.to_string_lossy().into_owned())
    }
}

fn is_remote(uri: &str) -> bool {
    uri.starts_with("http://") || uri.starts_with("https://")
}

fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    name.len() <= 64
        && first.is_ascii_alphanumeric()
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

fn parse_formats(formats: &[String]) -> Result<Vec<OutputFormat>> {
    if formats.is_empty() {
        anyhow::bail!("formats must have at least one format");
    }
    formats
        .iter()
        .map(|f| match f.as_str() {
            "png" => Ok(OutputFormat::Png(PngConfig::default())),
            "gif" => Ok(OutputFormat::Gif(GifConfig::default())),
            other => anyhow::bail!("unknown format '{other}' (use png or gif)"),
        })
        .collect()
}

/// PNG outputs as MCP image blocks, within the inline limits.
fn inline_images(report: &RunReport) -> Vec<Value> {
    report
        .scenes
        .iter()
        .flat_map(|s| s.files.iter())
        .filter(|f| Path::new(f).extension().is_some_and(|e| e == "png"))
        .filter(|f| std::fs::metadata(f).is_ok_and(|m| m.len() <= MAX_INLINE_IMAGE_BYTES))
        .take(MAX_INLINE_IMAGES)
        .filter_map(|f| std::fs::read(f).ok())
        .map(|bytes| {
            json!({
                "type": "image",
                "data": base64::engine::general_purpose::STANDARD.encode(bytes),
                "mimeType": "image/png",
            })
        })
        .collect()
}

fn result(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server(dir: &Path, allow: &[&str]) -> Server {
        Server::new(Options {
            allow: allow.iter().map(|s| s.to_string()).collect(),
            root: dir.join("root"),
            output: dir.join("out"),
            timeout: Duration::from_secs(5),
        })
        .unwrap()
    }

    /// A temp directory with `root/page.html` inside and `secret.html` beside it.
    fn fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("root")).unwrap();
        std::fs::write(dir.path().join("root/page.html"), "<p>hi</p>").unwrap();
        std::fs::write(dir.path().join("secret.html"), "<p>no</p>").unwrap();
        dir
    }

    async fn call(server: &Server, msg: Value) -> Value {
        server.handle_line(&msg.to_string()).await.unwrap()
    }

    /// The text of a `capture` call that must fail before any browser starts.
    async fn refused(server: &Server, arguments: Value) -> String {
        let reply = call(
            server,
            json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                    "params": { "name": "capture", "arguments": arguments } }),
        )
        .await;
        assert_eq!(reply["result"]["isError"], true, "{reply}");
        reply["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .to_string()
    }

    #[tokio::test]
    async fn initialize_echoes_a_known_version_and_offers_the_newest_otherwise() {
        let dir = fixture();
        let s = server(dir.path(), &["web"]);
        for (asked, got) in [("2024-11-05", "2024-11-05"), ("2099-01-01", "2025-06-18")] {
            let reply = call(
                &s,
                json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize",
                        "params": { "protocolVersion": asked } }),
            )
            .await;
            assert_eq!(reply["result"]["protocolVersion"], got);
            assert_eq!(reply["result"]["serverInfo"]["name"], "teasr");
        }
    }

    #[tokio::test]
    async fn notifications_get_no_reply_and_unknown_methods_get_an_error() {
        let dir = fixture();
        let s = server(dir.path(), &["web"]);
        let silent = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
        assert!(s.handle_line(&silent.to_string()).await.is_none());

        let reply = call(&s, json!({ "jsonrpc": "2.0", "id": 7, "method": "nope" })).await;
        assert_eq!(reply["id"], 7);
        assert_eq!(reply["error"]["code"], METHOD_NOT_FOUND);

        let reply = s.handle_line("{not json").await.unwrap();
        assert_eq!(reply["error"]["code"], PARSE_ERROR);
        assert_eq!(reply["id"], Value::Null);
    }

    #[tokio::test]
    async fn tool_schema_lists_only_allowed_scene_types() {
        let dir = fixture();
        for allow in [vec!["web"], vec!["terminal", "web"]] {
            let s = server(dir.path(), &allow);
            let reply = call(
                &s,
                json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }),
            )
            .await;
            let tools = reply["result"]["tools"].as_array().unwrap();
            assert_eq!(tools.len(), 1);
            assert_eq!(tools[0]["name"], "capture");
            let kinds = &tools[0]["inputSchema"]["properties"]["scenes"]["items"]["properties"]
                ["type"]["enum"];
            // Always in web, terminal, screen order, whatever order --allow gave.
            let want: Vec<&str> = SCENE_TYPES
                .into_iter()
                .filter(|t| allow.contains(t))
                .collect();
            assert_eq!(kinds, &json!(want));
        }
    }

    /// Clients pin a tool by the hash of its schema, so a schema change must
    /// be a deliberate, visible diff. To accept one, run:
    /// `teasr mcp tools > crates/teasr-cli/tests/golden/mcp-tools.json`
    #[test]
    fn the_default_tool_list_matches_the_golden_file() {
        let golden: Value =
            serde_json::from_str(include_str!("../tests/golden/mcp-tools.json")).unwrap();
        let now = tools_list(vec!["web".to_string()]).unwrap();
        assert_eq!(
            now, golden,
            "the capture tool changed; regenerate the golden file if that is intended"
        );
    }

    #[test]
    fn every_tool_has_the_four_annotations() {
        let list = tools_list(vec!["web".to_string(), "terminal".to_string()]).unwrap();
        for tool in list["tools"].as_array().unwrap() {
            for hint in [
                "readOnlyHint",
                "destructiveHint",
                "idempotentHint",
                "openWorldHint",
            ] {
                assert!(
                    tool["annotations"][hint].is_boolean(),
                    "{hint} on {}",
                    tool["name"]
                );
            }
        }
    }

    #[tokio::test]
    async fn a_scene_type_that_is_not_allowed_is_refused() {
        let dir = fixture();
        let s = server(dir.path(), &["web"]);
        let text = refused(
            &s,
            json!({ "scenes": [{ "type": "terminal", "name": "x", "command": "id" }] }),
        )
        .await;
        assert_eq!(
            text,
            "scenes[0]: scene type 'terminal' is not allowed on this server (allowed: web)"
        );
    }

    #[tokio::test]
    async fn a_local_file_outside_the_root_is_refused() {
        let dir = fixture();
        let s = server(dir.path(), &["web"]);
        for uri in ["../secret.html", "file:///etc/hosts"] {
            let text = refused(
                &s,
                json!({ "scenes": [{ "type": "web", "name": "x", "uri": uri }] }),
            )
            .await;
            assert!(text.contains("is outside the server root"), "{uri}: {text}");
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_symlink_out_of_the_root_is_refused() {
        let dir = fixture();
        std::os::unix::fs::symlink(
            dir.path().join("secret.html"),
            dir.path().join("root/link.html"),
        )
        .unwrap();
        let s = server(dir.path(), &["web"]);
        let text = refused(
            &s,
            json!({ "scenes": [{ "type": "web", "name": "x", "uri": "link.html" }] }),
        )
        .await;
        assert!(text.contains("is outside the server root"), "{text}");
    }

    #[tokio::test]
    async fn names_cannot_leave_the_output_directory_or_repeat() {
        let dir = fixture();
        let s = server(dir.path(), &["web"]);
        for name in ["../escape", "a/b", ".hidden", ""] {
            let text = refused(
                &s,
                json!({ "scenes": [{ "type": "web", "name": name, "uri": "page.html" }] }),
            )
            .await;
            assert!(text.contains("must be 1 to 64 letters"), "{name}: {text}");
        }
        let text = refused(
            &s,
            json!({ "scenes": [
                { "type": "web", "name": "same", "uri": "page.html" },
                { "type": "web", "name": "same", "uri": "page.html" },
            ] }),
        )
        .await;
        assert_eq!(text, "scenes[1]: name 'same' is used by an earlier scene");
    }

    #[tokio::test]
    async fn bad_arguments_are_tool_errors_not_protocol_errors() {
        let dir = fixture();
        let s = server(dir.path(), &["web"]);
        let page = json!([{ "type": "web", "name": "x", "uri": "page.html" }]);
        for (args, want) in [
            (json!({}), "missing field `scenes`"),
            (json!({ "scenes": [] }), "at least one scene"),
            (
                json!({ "scenes": page, "output_dir": "/tmp" }),
                "unknown field `output_dir`",
            ),
            (
                json!({ "scenes": page, "formats": ["mp4"] }),
                "unknown format 'mp4'",
            ),
            (
                json!({ "scenes": [{ "type": "web", "name": "x" }] }),
                "uri is required",
            ),
            (
                json!({ "scenes": [{ "type": "web", "name": "x", "uri": "page.html", "formats": ["mp4"] }] }),
                "unknown format 'mp4'",
            ),
            (
                json!({ "scenes": [{ "type": "web", "name": "x", "uri": "missing.html" }] }),
                "file not found: missing.html",
            ),
        ] {
            let text = refused(&s, args).await;
            assert!(text.contains(want), "want {want:?} in {text:?}");
        }
    }

    #[test]
    fn scene_formats_are_taken_as_names() {
        let dir = fixture();
        let s = server(dir.path(), &["web"]);
        let scenes = s
            .scenes(vec![json!({
                "type": "web", "name": "x", "uri": "page.html", "formats": ["png", "gif"]
            })])
            .unwrap();
        let formats = scenes[0].formats().as_ref().unwrap();
        assert!(matches!(
            formats.as_slice(),
            [OutputFormat::Png(_), OutputFormat::Gif(_)]
        ));
    }

    #[test]
    fn local_paths_resolve_against_the_root_not_the_working_directory() {
        let dir = fixture();
        let s = server(dir.path(), &["web"]);
        let got = s.local_path("page.html").unwrap();
        assert_eq!(PathBuf::from(got), s.root.join("page.html"));
    }

    #[test]
    fn unknown_allow_value_fails_at_startup() {
        let dir = fixture();
        let err = Server::new(Options {
            allow: vec!["browser".to_string()],
            root: dir.path().join("root"),
            output: dir.path().join("out"),
            timeout: Duration::from_secs(1),
        })
        .err()
        .unwrap();
        assert_eq!(
            err.to_string(),
            "unknown scene type 'browser' for --allow (use web, terminal, screen)"
        );
    }
}
