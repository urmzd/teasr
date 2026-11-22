//! `teasr init`: write a starter `teasr.toml`.

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

pub struct Template {
    pub name: &'static str,
    pub about: &'static str,
    pub body: &'static str,
}

/// The first template is the default: it needs no browser and no server, so
/// `teasr init && teasr run` works on a fresh machine.
pub const TEMPLATES: &[Template] = &[
    Template {
        name: "terminal",
        about: "A terminal recording. Needs no browser and no server",
        body: include_str!("../templates/terminal.toml"),
    },
    Template {
        name: "web",
        about: "Screenshots of a web app, with a dev server teasr starts",
        body: include_str!("../templates/web.toml"),
    },
    Template {
        name: "markdown",
        about: "A Markdown file rendered as an image",
        body: include_str!("../templates/markdown.toml"),
    },
];

pub const CONFIG_FILE: &str = "teasr.toml";

pub fn find(name: &str) -> Result<&'static Template> {
    TEMPLATES.iter().find(|t| t.name == name).with_context(|| {
        let names: Vec<&str> = TEMPLATES.iter().map(|t| t.name).collect();
        format!("unknown template '{name}' (use {})", names.join(", "))
    })
}

/// Write the template as `teasr.toml` in `dir`. An existing file is kept
/// unless `force` is set.
pub fn write(dir: &Path, template: &Template, force: bool) -> Result<PathBuf> {
    let path = dir.join(CONFIG_FILE);
    if path.exists() && !force {
        bail!(
            "{} already exists. Use --force to overwrite it",
            path.display()
        );
    }
    std::fs::write(&path, template.body)
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_template_is_a_config_teasr_accepts() {
        for template in TEMPLATES {
            let dir = tempfile::tempdir().unwrap();
            let path = write(dir.path(), template, false).unwrap();
            let config = teasr_core::config::load_config(&path)
                .unwrap_or_else(|e| panic!("template {}: {e:#}", template.name));
            assert!(!config.scenes.is_empty(), "template {}", template.name);
        }
    }

    #[test]
    fn an_existing_config_is_kept_unless_forced() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(CONFIG_FILE);
        std::fs::write(&path, "# mine\n").unwrap();

        let err = write(dir.path(), &TEMPLATES[0], false).unwrap_err();
        assert!(err.to_string().ends_with("Use --force to overwrite it"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "# mine\n");

        write(dir.path(), &TEMPLATES[0], true).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), TEMPLATES[0].body);
    }

    #[test]
    fn an_unknown_template_names_the_valid_ones() {
        let err = find("nope").err().unwrap();
        assert_eq!(
            err.to_string(),
            "unknown template 'nope' (use terminal, web, markdown)"
        );
    }
}
