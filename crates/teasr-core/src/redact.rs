//! PII redaction: pattern-based text masking for terminal grids, pixel-region
//! masking for captured frames, and CSS-selector masking for web scenes.
//!
//! Text redaction operates on the pre-render cell grid, so matched characters
//! are replaced in place (layout and styling preserved). Region redaction
//! decodes PNG frame bytes, applies the configured style, and re-encodes.
//! Selector masking is applied in the web backend by injecting overlay
//! elements before each screenshot (see `capture/web.rs`).

use anyhow::{Context, Result};
use image::RgbaImage;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::io::Cursor;

use crate::term_render::CellGrid;
use crate::types::Region;

/// Visual style used to hide a masked region or selector.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RedactStyle {
    /// Solid fill (near-black). The safest option: nothing recoverable.
    #[default]
    Block,
    /// Gaussian blur. Readable shapes remain; avoid for secrets.
    Blur,
    /// Mosaic pixelation.
    Pixelate,
}

/// A pixel rectangle to hide in every captured frame, with an optional
/// per-region style override.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RedactRegion {
    #[serde(flatten)]
    pub rect: Region,
    pub style: Option<RedactStyle>,
}

/// Redaction settings. Appears globally as `[redact]` and per scene as
/// `[scenes.redact]`; scene lists extend the global lists, scene scalars
/// override global ones.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RedactConfig {
    /// Built-in pattern names (see `builtin_pattern` for the full list).
    #[serde(default)]
    pub patterns: Vec<String>,
    /// Custom regexes applied to terminal text.
    #[serde(default)]
    pub custom: Vec<String>,
    /// Exact strings to mask wherever they appear in terminal text.
    #[serde(default)]
    pub literals: Vec<String>,
    /// Mask the current OS username (`$USER`) and home-directory paths.
    #[serde(default)]
    pub username: bool,
    /// Replacement character for text redaction.
    pub mask: Option<char>,
    /// Default style for `regions` and `selectors`.
    pub style: Option<RedactStyle>,
    /// Pixel rectangles hidden in every frame (all scene types).
    #[serde(default)]
    pub regions: Vec<RedactRegion>,
    /// CSS selectors masked with page overlays (web scenes only).
    #[serde(default)]
    pub selectors: Vec<String>,
}

impl RedactConfig {
    /// Merge a scene-level config over a global one: lists concatenate
    /// (global first), scalars take the scene value when set.
    pub fn merged(global: Option<&RedactConfig>, scene: Option<&RedactConfig>) -> Option<RedactConfig> {
        match (global, scene) {
            (None, None) => None,
            (Some(g), None) => Some(g.clone()),
            (None, Some(s)) => Some(s.clone()),
            (Some(g), Some(s)) => {
                let mut merged = g.clone();
                merged.patterns.extend(s.patterns.iter().cloned());
                merged.custom.extend(s.custom.iter().cloned());
                merged.literals.extend(s.literals.iter().cloned());
                merged.regions.extend(s.regions.iter().cloned());
                merged.selectors.extend(s.selectors.iter().cloned());
                merged.username = merged.username || s.username;
                if s.mask.is_some() {
                    merged.mask = s.mask;
                }
                if s.style.is_some() {
                    merged.style = s.style;
                }
                Some(merged)
            }
        }
    }
}

/// Regex source for a built-in pattern name, or `None` if unknown.
/// The `secrets` group name expands separately in `compile`.
fn builtin_pattern(name: &str) -> Option<&'static str> {
    Some(match name {
        "email" => r"[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}",
        "ipv4" => r"\b(?:\d{1,3}\.){3}\d{1,3}\b",
        "ipv6" => r"\b(?:[0-9A-Fa-f]{1,4}:){2,7}[0-9A-Fa-f]{1,4}\b",
        "ssn" => r"\b\d{3}-\d{2}-\d{4}\b",
        "credit-card" => r"\b\d{4}[ -]?\d{4}[ -]?\d{4}[ -]?\d{4}\b",
        "phone" => r"\+?\d{1,3}[ .-]?\(?\d{3}\)?[ .-]?\d{3}[ .-]?\d{4}\b",
        "aws-key" => r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b",
        "github-token" => r"\b(?:ghp|gho|ghu|ghs|ghr)_[A-Za-z0-9]{36}\b|\bgithub_pat_[A-Za-z0-9_]{22,}\b",
        "slack-token" => r"\bxox[baprs]-[A-Za-z0-9-]{10,}\b",
        "google-api-key" => r"\bAIza[0-9A-Za-z_-]{35}\b",
        "stripe-key" => r"\b[sr]k_live_[A-Za-z0-9]{20,}\b",
        "openai-key" => r"\bsk-[A-Za-z0-9_-]{20,}\b",
        "anthropic-key" => r"\bsk-ant-[A-Za-z0-9_-]{20,}\b",
        "npm-token" => r"\bnpm_[A-Za-z0-9]{36}\b",
        "jwt" => r"\beyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]+\b",
        "private-key" => r"-----BEGIN [A-Z ]*PRIVATE KEY-----",
        "bearer-token" => r"(?i)\bbearer +[A-Za-z0-9._~+/=-]{16,}",
        "home-path" => r"(?:/Users/|/home/|C:\\Users\\)[A-Za-z0-9._-]+",
        _ => return None,
    })
}

/// Members of the `secrets` convenience group.
const SECRETS_GROUP: &[&str] = &[
    "aws-key",
    "github-token",
    "slack-token",
    "google-api-key",
    "stripe-key",
    "openai-key",
    "anthropic-key",
    "npm-token",
    "jwt",
    "private-key",
    "bearer-token",
];

const DEFAULT_MASK: char = '•';

/// A compiled, ready-to-apply redaction set for one scene.
#[derive(Debug, Clone)]
pub struct CompiledRedactions {
    regexes: Vec<Regex>,
    pub mask: char,
    pub style: RedactStyle,
    pub regions: Vec<RedactRegion>,
    pub selectors: Vec<String>,
}

impl CompiledRedactions {
    pub fn compile(config: &RedactConfig) -> Result<Self> {
        let mut sources: Vec<String> = Vec::new();

        for name in &config.patterns {
            if name == "secrets" {
                sources.extend(
                    SECRETS_GROUP
                        .iter()
                        .map(|n| builtin_pattern(n).unwrap().to_string()),
                );
            } else {
                let src = builtin_pattern(name).with_context(|| {
                    format!("unknown redact pattern '{name}' (see docs for built-in names)")
                })?;
                sources.push(src.to_string());
            }
        }
        sources.extend(config.custom.iter().cloned());
        for lit in &config.literals {
            sources.push(regex::escape(lit));
        }
        if config.username {
            if let Ok(user) = std::env::var("USER").or_else(|_| std::env::var("USERNAME")) {
                if !user.is_empty() {
                    sources.push(regex::escape(&user));
                }
            }
            // The machine hostname often embeds the user's real name
            // (e.g. "Janes-MacBook-Pro"). Prompts usually show the short
            // name while gethostname may return "Janes-MacBook-Pro.local",
            // so mask the first label as well as the full name.
            if let Some(host) = hostname() {
                if let Some(short) = host.split('.').next() {
                    if short.len() >= 4 {
                        sources.push(regex::escape(short));
                    }
                }
            }
            sources.push(builtin_pattern("home-path").unwrap().to_string());
        }

        let regexes = sources
            .iter()
            .map(|src| {
                Regex::new(src).with_context(|| format!("invalid redact regex: {src}"))
            })
            .collect::<Result<Vec<_>>>()?;

        Ok(Self {
            regexes,
            mask: config.mask.unwrap_or(DEFAULT_MASK),
            style: config.style.unwrap_or_default(),
            regions: config.regions.clone(),
            selectors: config.selectors.clone(),
        })
    }

    pub fn has_text_rules(&self) -> bool {
        !self.regexes.is_empty()
    }

    pub fn has_regions(&self) -> bool {
        !self.regions.is_empty()
    }

    /// Replace matched characters in the grid with the mask character.
    /// The grid is matched as one concatenated string so tokens that wrap
    /// across rows are still caught (non-wrapped rows end in padding spaces,
    /// which break patterns at the row boundary naturally).
    pub fn redact_grid(&self, grid: &mut CellGrid) {
        if self.regexes.is_empty() {
            return;
        }
        let mut text = String::new();
        let mut positions: Vec<(usize, usize)> = Vec::new();
        for (ri, row) in grid.rows.iter().enumerate() {
            for (ci, cell) in row.iter().enumerate() {
                text.push(cell.ch);
                positions.push((ri, ci));
            }
        }
        let mut masked = vec![false; positions.len()];
        for re in &self.regexes {
            for m in re.find_iter(&text) {
                let start = text[..m.start()].chars().count();
                let len = m.as_str().chars().count();
                for flag in masked.iter_mut().skip(start).take(len) {
                    *flag = true;
                }
            }
        }
        for (i, hit) in masked.into_iter().enumerate() {
            if hit {
                let (ri, ci) = positions[i];
                grid.rows[ri][ci].ch = self.mask;
            }
        }
    }

    /// Apply region redactions to PNG bytes, re-encoding only when needed.
    pub fn apply_regions_png(&self, png: &[u8]) -> Result<Vec<u8>> {
        if self.regions.is_empty() {
            return Ok(png.to_vec());
        }
        let mut img = image::load_from_memory(png)
            .context("failed to decode frame for redaction")?
            .to_rgba8();
        self.apply_regions_image(&mut img);
        let mut buf = Vec::new();
        img.write_to(&mut Cursor::new(&mut buf), image::ImageFormat::Png)
            .context("failed to re-encode redacted frame")?;
        Ok(buf)
    }

    /// Apply region redactions directly to a decoded image.
    pub fn apply_regions_image(&self, img: &mut RgbaImage) {
        for region in &self.regions {
            let style = region.style.unwrap_or(self.style);
            apply_style(img, &region.rect, style);
        }
    }
}

#[cfg(unix)]
fn hostname() -> Option<String> {
    let mut buf = [0u8; 256];
    let ret = unsafe { libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) };
    if ret != 0 {
        return None;
    }
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8(buf[..end].to_vec()).ok()
}

#[cfg(not(unix))]
fn hostname() -> Option<String> {
    std::env::var("COMPUTERNAME").ok()
}

/// Clamp a region to the image bounds; returns `None` when fully outside.
fn clamp_region(img: &RgbaImage, r: &Region) -> Option<(u32, u32, u32, u32)> {
    let (iw, ih) = img.dimensions();
    if r.x >= iw || r.y >= ih {
        return None;
    }
    let w = r.width.min(iw - r.x);
    let h = r.height.min(ih - r.y);
    if w == 0 || h == 0 {
        None
    } else {
        Some((r.x, r.y, w, h))
    }
}

fn apply_style(img: &mut RgbaImage, rect: &Region, style: RedactStyle) {
    let Some((x, y, w, h)) = clamp_region(img, rect) else {
        return;
    };
    match style {
        RedactStyle::Block => {
            let fill = image::Rgba([15u8, 15, 15, 255]);
            for py in y..y + h {
                for px in x..x + w {
                    img.put_pixel(px, py, fill);
                }
            }
        }
        RedactStyle::Blur => {
            let sub = image::imageops::crop_imm(img, x, y, w, h).to_image();
            // Sigma scales with region size so small regions still smear.
            let sigma = (w.min(h) as f32 / 6.0).max(4.0);
            let blurred = image::imageops::blur(&sub, sigma);
            image::imageops::replace(img, &blurred, x as i64, y as i64);
        }
        RedactStyle::Pixelate => {
            let sub = image::imageops::crop_imm(img, x, y, w, h).to_image();
            let block = 12u32;
            let (dw, dh) = ((w / block).max(1), (h / block).max(1));
            let small = image::imageops::resize(&sub, dw, dh, image::imageops::FilterType::Triangle);
            let mosaic = image::imageops::resize(&small, w, h, image::imageops::FilterType::Nearest);
            image::imageops::replace(img, &mosaic, x as i64, y as i64);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::term_render::Cell;

    fn config_with(patterns: &[&str]) -> RedactConfig {
        RedactConfig {
            patterns: patterns.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    fn grid_from(text: &str, cols: usize) -> CellGrid {
        let mut row: Vec<Cell> = text
            .chars()
            .map(|ch| Cell {
                ch,
                ..Default::default()
            })
            .collect();
        row.resize(cols, Cell::default());
        CellGrid {
            cols,
            rows: vec![row],
        }
    }

    fn row_text(grid: &CellGrid) -> String {
        grid.rows[0].iter().map(|c| c.ch).collect()
    }

    #[test]
    fn redacts_email_preserving_layout() {
        let compiled = CompiledRedactions::compile(&config_with(&["email"])).unwrap();
        let mut grid = grid_from("mail me at jane@example.com today", 40);
        compiled.redact_grid(&mut grid);
        let text = row_text(&grid);
        assert!(text.starts_with("mail me at "));
        assert!(text.contains("••••••••••••••••"));
        assert!(!text.contains("jane@example.com"));
        assert_eq!(grid.rows[0].len(), 40);
    }

    #[test]
    fn redacts_secrets_group() {
        let compiled = CompiledRedactions::compile(&config_with(&["secrets"])).unwrap();
        let mut grid = grid_from("export AWS_KEY=AKIAIOSFODNN7EXAMPLE", 45);
        compiled.redact_grid(&mut grid);
        assert!(!row_text(&grid).contains("AKIAIOSFODNN7EXAMPLE"));
    }

    #[test]
    fn custom_regex_and_literal() {
        let config = RedactConfig {
            custom: vec![r"acme-[0-9a-f]{8}".to_string()],
            literals: vec!["s3cret-host".to_string()],
            mask: Some('█'),
            ..Default::default()
        };
        let compiled = CompiledRedactions::compile(&config).unwrap();
        let mut grid = grid_from("acme-deadbeef on s3cret-host", 30);
        compiled.redact_grid(&mut grid);
        let text = row_text(&grid);
        assert!(!text.contains("deadbeef"));
        assert!(!text.contains("s3cret-host"));
        assert!(text.contains('█'));
    }

    #[test]
    fn redacts_token_wrapped_across_rows() {
        let compiled = CompiledRedactions::compile(&config_with(&["aws-key"])).unwrap();
        // 10-col grid: the key spans rows with no padding at the wrap point
        let key = "AKIAIOSFODNN7EXAMPLE";
        let cols = 10;
        let rows: Vec<Vec<Cell>> = key
            .as_bytes()
            .chunks(cols)
            .map(|chunk| {
                let mut row: Vec<Cell> = chunk
                    .iter()
                    .map(|&b| Cell {
                        ch: b as char,
                        ..Default::default()
                    })
                    .collect();
                row.resize(cols, Cell::default());
                row
            })
            .collect();
        let mut grid = CellGrid { cols, rows };
        compiled.redact_grid(&mut grid);
        let all: String = grid
            .rows
            .iter()
            .flat_map(|r| r.iter().map(|c| c.ch))
            .collect();
        assert!(!all.contains("AKIA"));
        assert!(!all.contains("EXAMPLE"));
    }

    #[test]
    fn unknown_pattern_errors() {
        assert!(CompiledRedactions::compile(&config_with(&["nope"])).is_err());
    }

    #[test]
    fn invalid_custom_regex_errors() {
        let config = RedactConfig {
            custom: vec!["([".to_string()],
            ..Default::default()
        };
        assert!(CompiledRedactions::compile(&config).is_err());
    }

    #[test]
    fn merge_extends_lists_and_overrides_scalars() {
        let global = RedactConfig {
            patterns: vec!["email".to_string()],
            mask: Some('•'),
            ..Default::default()
        };
        let scene = RedactConfig {
            patterns: vec!["ipv4".to_string()],
            mask: Some('█'),
            selectors: vec![".secret".to_string()],
            ..Default::default()
        };
        let merged = RedactConfig::merged(Some(&global), Some(&scene)).unwrap();
        assert_eq!(merged.patterns, vec!["email", "ipv4"]);
        assert_eq!(merged.mask, Some('█'));
        assert_eq!(merged.selectors, vec![".secret"]);
    }

    #[test]
    fn block_region_fills_pixels() {
        let compiled = CompiledRedactions::compile(&RedactConfig {
            regions: vec![RedactRegion {
                rect: Region {
                    x: 2,
                    y: 2,
                    width: 4,
                    height: 4,
                },
                style: Some(RedactStyle::Block),
            }],
            ..Default::default()
        })
        .unwrap();
        let mut img = RgbaImage::from_pixel(10, 10, image::Rgba([255, 255, 255, 255]));
        compiled.apply_regions_image(&mut img);
        assert_eq!(img.get_pixel(3, 3), &image::Rgba([15, 15, 15, 255]));
        assert_eq!(img.get_pixel(0, 0), &image::Rgba([255, 255, 255, 255]));
    }

    #[test]
    fn pixelate_region_changes_content_and_region_is_clamped() {
        let compiled = CompiledRedactions::compile(&RedactConfig {
            regions: vec![RedactRegion {
                rect: Region {
                    x: 5,
                    y: 5,
                    width: 100,
                    height: 100,
                },
                style: Some(RedactStyle::Pixelate),
            }],
            ..Default::default()
        })
        .unwrap();
        // Checkerboard so pixelation visibly averages content
        let mut img = RgbaImage::from_fn(20, 20, |x, y| {
            if (x + y) % 2 == 0 {
                image::Rgba([0, 0, 0, 255])
            } else {
                image::Rgba([255, 255, 255, 255])
            }
        });
        let before = img.get_pixel(10, 10).0;
        compiled.apply_regions_image(&mut img);
        assert_ne!(img.get_pixel(10, 10).0, before);
    }

    #[test]
    fn apply_regions_png_roundtrip() {
        let compiled = CompiledRedactions::compile(&RedactConfig {
            regions: vec![RedactRegion {
                rect: Region {
                    x: 0,
                    y: 0,
                    width: 2,
                    height: 2,
                },
                style: None,
            }],
            ..Default::default()
        })
        .unwrap();
        let img = RgbaImage::from_pixel(4, 4, image::Rgba([200, 200, 200, 255]));
        let mut png = Vec::new();
        img.write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let out = compiled.apply_regions_png(&png).unwrap();
        let decoded = image::load_from_memory(&out).unwrap().to_rgba8();
        assert_eq!(decoded.get_pixel(1, 1), &image::Rgba([15, 15, 15, 255]));
    }

    #[test]
    fn no_rules_is_noop() {
        let compiled = CompiledRedactions::compile(&RedactConfig::default()).unwrap();
        let mut grid = grid_from("hello jane@example.com", 25);
        compiled.redact_grid(&mut grid);
        assert!(row_text(&grid).contains("jane@example.com"));
        assert!(!compiled.has_text_rules());
        assert!(!compiled.has_regions());
    }
}
