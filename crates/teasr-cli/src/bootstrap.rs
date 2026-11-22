//! `teasr bootstrap`: make this install usable by agents.
//!
//! The usage skill is compiled into the binary, so the skill and the binary
//! always describe the same version. `bootstrap` writes it to a directory
//! teasr owns, then asks agentspec to install it from there and to register
//! the MCP server. It never writes into `~/.agents` or a tool's config itself.

use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Command;

pub const SKILL: &str = include_str!("../skills/teasr/SKILL.md");
const NAME: &str = "teasr";

#[derive(Debug, Serialize, PartialEq, Eq, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub enum SkillState {
    /// agentspec installed or refreshed the bundled skill.
    Installed,
    /// Another skill named `teasr` is installed and was left alone.
    Kept,
    /// agentspec is not on PATH; the skill file was written but not installed.
    Written,
}

#[derive(Debug, Serialize)]
pub struct Step {
    pub command: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub detail: String,
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub dry_run: bool,
    /// The copy of the skill teasr owns. agentspec installs from here.
    pub skill_source: String,
    pub skill: SkillState,
    pub agentspec: bool,
    pub steps: Vec<Step>,
    /// The server definition, for a tool agentspec does not manage.
    pub mcp_server: Value,
}

impl Report {
    pub fn failed(&self) -> bool {
        self.steps.iter().any(|s| !s.ok)
    }
}

pub fn home() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .context("cannot find the home directory (HOME is not set)")
}

/// Where teasr keeps its own copy of the skill: `$XDG_DATA_HOME/teasr` or
/// `~/.local/share/teasr`, then `skills/teasr`.
pub fn skill_source_dir() -> Result<PathBuf> {
    let data = match std::env::var_os("XDG_DATA_HOME") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => home()?.join(".local/share"),
    };
    Ok(data.join(NAME).join("skills").join(NAME))
}

/// Where agentspec keeps an installed skill.
pub fn installed_skill() -> Result<PathBuf> {
    Ok(home()?.join(".agents/skills").join(NAME).join("SKILL.md"))
}

/// True when the installed skill is the one in this binary.
pub fn installed_skill_is_current() -> bool {
    installed_skill()
        .ok()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .is_some_and(|s| s == SKILL)
}

pub fn mcp_server() -> Value {
    json!({ "mcpServers": { NAME: { "command": NAME, "args": ["mcp", "serve"] } } })
}

fn has_agentspec() -> bool {
    Command::new("agentspec")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

fn run(steps: &mut Vec<Step>, dry_run: bool, args: &[&str]) {
    // Shown to a person, who may paste it: quote an argument with a space.
    let shown: Vec<String> = args
        .iter()
        .map(|a| {
            if a.contains(' ') {
                format!("\"{a}\"")
            } else {
                a.to_string()
            }
        })
        .collect();
    let command = format!("agentspec {}", shown.join(" "));
    if dry_run {
        steps.push(Step {
            command,
            ok: true,
            detail: "not run (dry run)".into(),
        });
        return;
    }
    let (ok, detail) = match Command::new("agentspec").args(args).output() {
        Ok(out) if out.status.success() => (true, String::new()),
        Ok(out) => (false, last_line(&out.stderr)),
        Err(e) => (false, e.to_string()),
    };
    steps.push(Step {
        command,
        ok,
        detail,
    });
}

fn last_line(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim()
        .to_string()
}

fn write_skill(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("failed to create {}", dir.display()))?;
    let path = dir.join("SKILL.md");
    std::fs::write(&path, SKILL).with_context(|| format!("failed to write {}", path.display()))
}

/// Install the bundled skill and register the MCP server.
///
/// `force` replaces a different skill that already has the name `teasr`.
/// Without it, that skill is kept: agentspec's rule is that the first
/// installed skill wins.
pub fn bootstrap(force: bool, dry_run: bool) -> Result<Report> {
    let source = skill_source_dir()?;
    let source_arg = source.to_string_lossy().into_owned();
    let agentspec = has_agentspec();
    let mut steps = Vec::new();

    if !dry_run {
        write_skill(&source)?;
    }

    let skill = if !agentspec {
        SkillState::Written
    } else {
        if force {
            run(&mut steps, dry_run, &["manage", "remove", NAME]);
            // Nothing to remove on a fresh machine is not a failure.
            if let Some(step) = steps.last_mut() {
                step.ok = true;
            }
        }
        // `add` installs the first time and skips when the name exists.
        // `update` then refreshes a copy that came from this source.
        run(
            &mut steps,
            dry_run,
            &["manage", "add", &source_arg, "--all-tools"],
        );
        run(&mut steps, dry_run, &["manage", "update", NAME]);
        run(
            &mut steps,
            dry_run,
            &["mcp", "add", NAME, "--command", NAME, "--args", "mcp serve"],
        );
        if dry_run || installed_skill_is_current() {
            SkillState::Installed
        } else {
            SkillState::Kept
        }
    };

    Ok(Report {
        dry_run,
        skill_source: source.join("SKILL.md").to_string_lossy().into_owned(),
        skill,
        agentspec,
        steps,
        mcp_server: mcp_server(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bundled_skill_is_the_teasr_skill() {
        assert!(SKILL.starts_with("---\nname: teasr\n"));
    }

    #[test]
    fn the_server_definition_starts_mcp_serve() {
        assert_eq!(
            mcp_server()["mcpServers"]["teasr"],
            json!({ "command": "teasr", "args": ["mcp", "serve"] })
        );
    }

    #[test]
    fn last_line_skips_trailing_blank_lines() {
        assert_eq!(
            last_line(b"first\n  error: bad thing  \n\n"),
            "error: bad thing"
        );
        assert_eq!(last_line(b""), "");
    }
}
