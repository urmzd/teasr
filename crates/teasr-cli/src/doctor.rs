//! `teasr doctor`: check that teasr can work on this machine. It reads and
//! reports. It installs nothing: `bootstrap`, `init` and `fonts` do that.

use serde::Serialize;
use std::path::Path;
use std::process::Command;

use crate::bootstrap;

#[derive(Debug, Serialize)]
pub struct Check {
    pub id: &'static str,
    /// A failed required check makes `doctor` exit 1.
    pub required: bool,
    pub ok: bool,
    pub detail: String,
    /// The command that fixes a failed check.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix: Option<String>,
}

fn check(id: &'static str, required: bool, result: Result<String, (String, &str)>) -> Check {
    match result {
        Ok(detail) => Check {
            id,
            required,
            ok: true,
            detail,
            fix: None,
        },
        Err((detail, fix)) => Check {
            id,
            required,
            ok: false,
            detail,
            fix: Some(fix.to_string()),
        },
    }
}

fn chrome() -> Check {
    check(
        "chrome",
        true,
        match teasr_core::browser::find_executable() {
            Some(path) => Ok(format!("Chrome found at {}", path.display())),
            None => Err((
                "Chrome or Chromium not found; web scenes cannot run".into(),
                "install Chrome or Chromium, or set CHROME to the path of the binary",
            )),
        },
    )
}

/// The font the config in use asks for. The generic `monospace` always
/// resolves to a system font, so only a named family can be missing.
fn font(cwd: &Path) -> Check {
    let family = teasr_core::config::discover_config(cwd)
        .and_then(|p| teasr_core::config::load_config(&p).ok())
        .map(|c| c.font.family)
        .unwrap_or_else(|| teasr_core::types::FontConfig::default().family);
    check(
        "font",
        false,
        if family == "monospace" {
            Ok("terminal scenes use the system monospace font".into())
        } else if teasr_core::setup::check_font(&family) {
            Ok(format!("font '{family}' is available"))
        } else {
            Err((
                format!(
                    "font '{family}' is not available; terminal scenes fall back to another font"
                ),
                "teasr fonts",
            ))
        },
    )
}

fn config(cwd: &Path) -> Check {
    check(
        "config",
        false,
        match teasr_core::config::discover_config(cwd) {
            None => Err(("no teasr.toml found from here up".into(), "teasr init")),
            Some(path) => match teasr_core::config::load_config(&path) {
                Ok(c) => Ok(format!(
                    "{} has {} scene(s)",
                    path.display(),
                    c.scenes.len()
                )),
                Err(e) => Err((
                    format!("{}: {e:#}", path.display()),
                    "fix the file, or teasr init --force",
                )),
            },
        },
    )
}

fn skill() -> Check {
    check(
        "skill",
        false,
        if bootstrap::installed_skill_is_current() {
            Ok("the installed skill matches this binary".into())
        } else if bootstrap::installed_skill().is_ok_and(|p| p.exists()) {
            Err((
                "the installed skill differs from this binary".into(),
                "teasr bootstrap (add --force when another skill is named teasr)",
            ))
        } else {
            Err((
                "the skill is not installed for agents".into(),
                "teasr bootstrap",
            ))
        },
    )
}

fn mcp() -> Check {
    let listed = Command::new("agentspec")
        .args(["mcp", "list", "--format", "json"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| serde_json::from_slice::<serde_json::Value>(&o.stdout).ok());
    check(
        "mcp",
        false,
        match listed {
            None => Err((
                "agentspec is not on PATH, so the MCP registration cannot be checked".into(),
                "install agentspec, or add the snippet from: teasr bootstrap --dry-run",
            )),
            Some(doc) => match doc["store"]["teasr"]["args"].as_array() {
                Some(args) if args == &["mcp", "serve"] => {
                    Ok("the MCP server is registered with agentspec".into())
                }
                Some(_) => Err((
                    "the registered MCP server does not start 'teasr mcp serve'".into(),
                    "teasr bootstrap",
                )),
                None => Err(("the MCP server is not registered".into(), "teasr bootstrap")),
            },
        },
    )
}

pub fn run(cwd: &Path) -> Vec<Check> {
    vec![chrome(), font(cwd), config(cwd), skill(), mcp()]
}

/// True when a required check failed.
pub fn failed(checks: &[Check]) -> bool {
    checks.iter().any(|c| c.required && !c.ok)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_required_failure_fails_doctor() {
        let mut checks = vec![
            check("a", true, Ok("fine".into())),
            check("b", false, Err(("missing".into(), "teasr init"))),
        ];
        assert!(!failed(&checks));
        checks.push(check("c", true, Err(("missing".into(), "install it"))));
        assert!(failed(&checks));
    }

    #[test]
    fn a_failed_check_carries_its_fix_and_a_passed_one_does_not() {
        let json = serde_json::to_value(vec![
            check("a", true, Ok("fine".into())),
            check("b", false, Err(("missing".into(), "teasr init"))),
        ])
        .unwrap();
        assert_eq!(
            json,
            serde_json::json!([
                { "id": "a", "required": true, "ok": true, "detail": "fine" },
                { "id": "b", "required": false, "ok": false, "detail": "missing", "fix": "teasr init" },
            ])
        );
    }

    #[test]
    fn config_check_reports_a_missing_file_with_init_as_the_fix() {
        let dir = tempfile::tempdir().unwrap();
        // A temp dir has no teasr.toml above it on any CI runner or laptop
        // unless one sits in the temp root, so guard the assertion.
        if teasr_core::config::discover_config(dir.path()).is_none() {
            let c = config(dir.path());
            assert!(!c.ok);
            assert_eq!(c.fix.as_deref(), Some("teasr init"));
        }
    }
}
