// The `capture` tool schema in mcp.rs is one deep `json!` literal.
#![recursion_limit = "256"]

mod bootstrap;
mod doctor;
mod init;
mod mcp;
mod self_update;

use anyhow::{Context, Result};
use clap::{CommandFactory, Parser, Subcommand};
use std::path::PathBuf;
use teasr_core::types::{ColorScheme, RunReport, SceneConfig, SceneStatus};

/// How a command reports its result.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
enum ReportFormat {
    // Styled progress on stderr. Plain comments: a doc comment here would
    // switch every `--help` to clap's long layout.
    Human,
    // One JSON document on stdout.
    Json,
}

#[derive(Parser, Debug)]
#[command(
    name = "teasr",
    about = "Capture showcase screenshots and GIFs",
    version
)]
struct Cli {
    /// Result output: human (on stderr) or json (one document on stdout)
    #[arg(long, global = true, default_value = "human", value_enum)]
    format: ReportFormat,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Write a starter teasr.toml in the current directory
    Init {
        /// Template to write (default: terminal). See --list
        template: Option<String>,

        /// List the templates and exit
        #[arg(long)]
        list: bool,

        /// Overwrite teasr.toml if it already exists
        #[arg(long)]
        force: bool,
    },
    /// Run capture scenes from teasr.toml
    #[command(alias = "showme")]
    Run {
        /// Path to config file (default: search for teasr.toml)
        #[arg(short, long)]
        config: Option<PathBuf>,

        /// Output directory (overrides config)
        #[arg(short, long)]
        output: Option<String>,

        /// Output formats (comma-separated: png,gif,mp4)
        #[arg(long, value_delimiter = ',')]
        formats: Option<Vec<String>>,

        /// Enable verbose logging
        #[arg(long)]
        verbose: bool,

        /// Global timeout in milliseconds
        #[arg(long, default_value = "60000")]
        timeout: u64,

        /// Frames per second (overrides config, converted to frame_duration)
        #[arg(long)]
        fps: Option<u32>,

        /// Target output duration in seconds (overrides config)
        #[arg(long)]
        seconds: Option<f64>,

        /// Per-scene wall-clock timeout in seconds (overrides config)
        #[arg(long)]
        scene_timeout: Option<f64>,

        /// Emulated prefers-color-scheme for web scenes: light or dark (overrides config)
        #[arg(long, value_name = "SCHEME")]
        color_scheme: Option<ColorScheme>,

        /// Only run scenes matching these names or types (repeatable, or comma-separated)
        #[arg(
            long = "scene",
            visible_alias = "scenes",
            value_name = "NAME",
            value_delimiter = ','
        )]
        scenes: Option<Vec<String>>,
    },
    /// List the fonts teasr can install, or install one by name
    Fonts {
        /// Font to install (e.g. "JetBrainsMono Nerd Font"). Lists the fonts if omitted
        name: Option<String>,
    },
    /// Check that teasr can work on this machine
    Doctor,
    /// Install the bundled agent skill and register the MCP server
    Bootstrap {
        /// Replace a different skill that is already named teasr
        #[arg(long)]
        force: bool,

        /// Show what would run without changing anything
        #[arg(long)]
        dry_run: bool,
    },
    /// Serve teasr to agents over MCP
    #[command(arg_required_else_help = true)]
    Mcp {
        #[command(subcommand)]
        action: McpAction,
    },
    /// Print shell completions on stdout
    Completions {
        /// bash, zsh, fish, elvish or powershell
        shell: clap_complete::Shell,
    },
    /// Old name of `fonts` and `doctor`. Kept for one release.
    #[command(hide = true)]
    Setup {
        #[arg(long)]
        fonts: Option<Option<String>>,

        #[arg(long)]
        check: bool,
    },
    /// Self-update teasr to the latest release
    Update,
    /// Print the current version
    Version,
}

#[derive(Subcommand, Debug)]
enum McpAction {
    /// Serve the `capture` tool on stdio until stdin closes
    Serve {
        /// Scene types a caller may run (comma-separated: web,terminal,screen).
        /// terminal runs shell commands; screen records the display.
        #[arg(long, value_delimiter = ',', default_value = "web")]
        allow: Vec<String>,

        /// Local files a web scene loads must be inside this directory
        #[arg(long, default_value = ".")]
        root: PathBuf,

        /// Each call writes to a new directory under this one
        #[arg(short, long, default_value = "./teasr-output")]
        output: PathBuf,

        /// Time limit for one capture call in milliseconds
        #[arg(long, default_value = "60000")]
        timeout: u64,

        /// Enable verbose logging (on stderr)
        #[arg(long)]
        verbose: bool,
    },
    /// Print the tool list `serve` offers, as JSON on stdout
    Tools {
        /// Scene types, as for `serve`: the schema lists only these
        #[arg(long, value_delimiter = ',', default_value = "web")]
        allow: Vec<String>,
    },
}

/// Send logs to stderr. stdout carries only data: a JSON report or the MCP
/// protocol.
fn init_logging(verbose: bool) {
    let filter = if verbose { "debug" } else { "warn" };
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| filter.into()),
        )
        .with_target(false)
        .with_writer(std::io::stderr)
        .init();
}

fn print_json(value: &impl serde::Serialize) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

async fn fonts(name: Option<String>) -> Result<()> {
    match name {
        Some(name) => teasr_core::setup::install_font(&name).await,
        None => {
            teasr_core::setup::list_fonts();
            teasr_core::ui::info("Install with: teasr fonts \"<font name>\"");
            Ok(())
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Some(Command::Init {
            template,
            list,
            force,
        }) => {
            if list {
                return match cli.format {
                    ReportFormat::Json => print_json(&serde_json::json!({
                        "templates": init::TEMPLATES.iter().map(|t| serde_json::json!({
                            "name": t.name, "about": t.about,
                        })).collect::<Vec<_>>(),
                    })),
                    ReportFormat::Human => {
                        for t in init::TEMPLATES {
                            teasr_core::ui::info(&format!("{:<9} {}", t.name, t.about));
                        }
                        Ok(())
                    }
                };
            }
            let template = init::find(template.as_deref().unwrap_or(init::TEMPLATES[0].name))?;
            let cwd = std::env::current_dir().context("failed to get cwd")?;
            let path = init::write(&cwd, template, force)?;
            match cli.format {
                ReportFormat::Json => print_json(&serde_json::json!({
                    "path": path, "template": template.name,
                })),
                ReportFormat::Human => {
                    teasr_core::ui::phase_ok("wrote", Some(&path.display().to_string()));
                    teasr_core::ui::info("Next: teasr run");
                    Ok(())
                }
            }
        }
        Some(Command::Fonts { name }) => {
            init_logging(false);
            fonts(name).await
        }
        Some(Command::Doctor) => {
            let cwd = std::env::current_dir().context("failed to get cwd")?;
            let checks = doctor::run(&cwd);
            match cli.format {
                ReportFormat::Json => print_json(&serde_json::json!({ "checks": checks }))?,
                ReportFormat::Human => {
                    teasr_core::ui::header("teasr doctor");
                    for c in &checks {
                        match (&c.fix, c.required) {
                            (None, _) => teasr_core::ui::phase_ok(&c.detail, None),
                            (Some(fix), true) => {
                                teasr_core::ui::error(&format!("{}. Fix: {fix}", c.detail))
                            }
                            (Some(fix), false) => {
                                teasr_core::ui::warn(&format!("{}. Fix: {fix}", c.detail))
                            }
                        }
                    }
                }
            }
            if doctor::failed(&checks) {
                anyhow::bail!("a required check failed");
            }
            Ok(())
        }
        Some(Command::Bootstrap { force, dry_run }) => {
            let report = bootstrap::bootstrap(force, dry_run)?;
            match cli.format {
                ReportFormat::Json => print_json(&report)?,
                ReportFormat::Human => {
                    use bootstrap::SkillState;
                    teasr_core::ui::header("teasr bootstrap");
                    for step in &report.steps {
                        if step.ok {
                            let detail = (!step.detail.is_empty()).then_some(step.detail.as_str());
                            teasr_core::ui::phase_ok(&step.command, detail);
                        } else {
                            teasr_core::ui::error(&format!("{}: {}", step.command, step.detail));
                        }
                    }
                    match report.skill {
                        SkillState::Installed => teasr_core::ui::phase_ok(
                            "skill installed for agents",
                            Some(&report.skill_source),
                        ),
                        SkillState::Kept => teasr_core::ui::warn(
                            "another skill named teasr is installed and was kept. Replace it with: teasr bootstrap --force",
                        ),
                        SkillState::Written => {
                            teasr_core::ui::warn("agentspec is not on PATH, so nothing was installed into your AI tools");
                            teasr_core::ui::info(&format!("the skill is at {}", report.skill_source));
                            teasr_core::ui::info("add this server to your tool's MCP config:");
                            eprintln!("{}", serde_json::to_string_pretty(&report.mcp_server)?);
                        }
                    }
                }
            }
            if report.failed() {
                anyhow::bail!("a bootstrap step failed");
            }
            Ok(())
        }
        Some(Command::Completions { shell }) => {
            clap_complete::generate(shell, &mut Cli::command(), "teasr", &mut std::io::stdout());
            Ok(())
        }
        Some(Command::Setup { fonts: name, check }) => {
            init_logging(false);
            teasr_core::ui::warn("`teasr setup` is now `teasr fonts` and `teasr doctor`");
            if check {
                let family = teasr_core::types::FontConfig::default().family;
                if !teasr_core::setup::check_font(&family) {
                    anyhow::bail!("font '{family}' is NOT available. Run: teasr fonts");
                }
                teasr_core::ui::phase_ok(&format!("font '{family}' is available"), None);
                return Ok(());
            }
            fonts(name.flatten()).await
        }
        Some(Command::Run {
            config,
            output,
            formats,
            verbose,
            timeout,
            fps,
            seconds,
            scene_timeout,
            color_scheme,
            scenes,
        }) => {
            init_logging(verbose);

            teasr_core::ui::header("teasr run");

            let timeout_dur = std::time::Duration::from_millis(timeout);
            let result = tokio::time::timeout(
                timeout_dur,
                run(
                    cli.format,
                    config,
                    output,
                    formats,
                    fps,
                    seconds,
                    scene_timeout,
                    color_scheme,
                    scenes,
                ),
            )
            .await;

            match result {
                Ok(Ok(())) => Ok(()),
                Ok(Err(e)) => Err(e),
                Err(_) => {
                    anyhow::bail!("teasr timed out after {}ms", timeout_dur.as_millis());
                }
            }
        }
        Some(Command::Mcp { action }) => match action {
            McpAction::Serve {
                allow,
                root,
                output,
                timeout,
                verbose,
            } => {
                init_logging(verbose);
                mcp::Server::new(mcp::Options {
                    allow,
                    root,
                    output,
                    timeout: std::time::Duration::from_millis(timeout),
                })?
                .serve()
                .await
            }
            McpAction::Tools { allow } => print_json(&mcp::tools_list(allow)?),
        },
        Some(Command::Update) => {
            let current = env!("CARGO_PKG_VERSION");
            let result = self_update::self_update("urmzd/teasr", current, "teasr")?;
            let (from, to, updated) = match &result {
                self_update::UpdateResult::AlreadyUpToDate => (current, current, false),
                self_update::UpdateResult::Updated { from, to } => {
                    (from.as_str(), to.as_str(), true)
                }
            };
            match cli.format {
                ReportFormat::Json => print_json(&serde_json::json!({
                    "from": from, "to": to, "updated": updated,
                }))?,
                ReportFormat::Human if updated => {
                    teasr_core::ui::phase_ok(&format!("updated: {from} → {to}"), None);
                    teasr_core::ui::info("Refresh the agent skill with: teasr bootstrap");
                }
                ReportFormat::Human => {
                    teasr_core::ui::phase_ok(&format!("already up to date (v{current})"), None)
                }
            }
            Ok(())
        }
        Some(Command::Version) => {
            match cli.format {
                ReportFormat::Human => println!("teasr v{}", env!("CARGO_PKG_VERSION")),
                ReportFormat::Json => println!(
                    "{}",
                    serde_json::json!({ "name": "teasr", "version": env!("CARGO_PKG_VERSION") })
                ),
            }
            Ok(())
        }
        None => {
            Cli::parse_from(["teasr", "--help"]);
            Ok(())
        }
    }
}

fn scene_matches(scene: &SceneConfig, filter: &str) -> bool {
    scene.name().eq_ignore_ascii_case(filter) || scene.scene_type().eq_ignore_ascii_case(filter)
}

/// Keep only the scenes selected by `filter`. Every filter entry must match
/// at least one scene, so a typo fails loudly instead of silently running less.
fn filter_scenes(scenes: &mut Vec<SceneConfig>, filter: &[String]) -> Result<()> {
    let unmatched: Vec<&str> = filter
        .iter()
        .filter(|f| !scenes.iter().any(|s| scene_matches(s, f)))
        .map(String::as_str)
        .collect();
    if !unmatched.is_empty() {
        let available: Vec<&str> = scenes.iter().map(|s| s.name()).collect();
        anyhow::bail!(
            "no scene matched: {} (available: {})",
            unmatched.join(", "),
            available.join(", ")
        );
    }
    scenes.retain(|s| filter.iter().any(|f| scene_matches(s, f)));
    Ok(())
}

/// Rewrite the report's paths as absolute, so a caller in another working
/// directory can open them. A path that cannot be resolved is left as written.
fn absolute_paths(mut report: RunReport) -> RunReport {
    let abs = |p: &str| {
        std::fs::canonicalize(p)
            .map(|a| a.to_string_lossy().into_owned())
            .unwrap_or_else(|_| p.to_string())
    };
    report.output_dir = abs(&report.output_dir);
    for scene in &mut report.scenes {
        for file in &mut scene.files {
            *file = abs(file);
        }
    }
    report
}

#[allow(clippy::too_many_arguments)]
async fn run(
    format: ReportFormat,
    config: Option<PathBuf>,
    output: Option<String>,
    formats: Option<Vec<String>>,
    fps: Option<u32>,
    seconds: Option<f64>,
    scene_timeout: Option<f64>,
    color_scheme: Option<ColorScheme>,
    scenes: Option<Vec<String>>,
) -> Result<()> {
    let config_path = if let Some(path) = &config {
        path.clone()
    } else {
        let cwd = std::env::current_dir().context("failed to get cwd")?;
        teasr_core::config::discover_config(&cwd)
            .context("no teasr.toml found (searched from cwd to root). Use --config to specify.")?
    };

    teasr_core::ui::info(&format!("using config: {}", config_path.display()));
    let mut config = teasr_core::config::load_config(&config_path)?;

    if let Some(output) = &output {
        config.output.dir = output.clone();
    }

    if let Some(fps) = fps {
        config.frame_duration_ms = 1000 / fps.max(1) as u64;
    }

    if let Some(secs) = seconds {
        config.seconds = secs;
    }

    if let Some(t) = scene_timeout {
        config.scene_timeout = t;
    }

    if color_scheme.is_some() {
        config.color_scheme = color_scheme;
    }

    if let Some(ref filter) = scenes {
        filter_scenes(&mut config.scenes, filter)?;
    }

    if let Some(formats) = &formats {
        let parsed: Vec<teasr_core::types::OutputFormat> = formats
            .iter()
            .map(|f: &String| match f.as_str() {
                "png" => Ok(teasr_core::types::OutputFormat::Png(Default::default())),
                "gif" => Ok(teasr_core::types::OutputFormat::Gif(Default::default())),
                "mp4" => Ok(teasr_core::types::OutputFormat::Mp4(Default::default())),
                other => anyhow::bail!("unknown format: {other}"),
            })
            .collect::<Result<_>>()?;
        config.output.formats = parsed;
    }

    let report = teasr_core::orchestrator::run_report(&config).await?;

    match format {
        ReportFormat::Human => {
            for scene in report.scenes.iter().filter(|s| s.status == SceneStatus::Ok) {
                for file in &scene.files {
                    teasr_core::ui::phase_ok("wrote", Some(file));
                }
            }
        }
        // Printed before the all-failed error below, so a caller always gets
        // the per-scene errors as data.
        ReportFormat::Json => {
            let report = absolute_paths(report.clone());
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
    }

    if report.all_failed() {
        anyhow::bail!("all scenes failed");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scenes() -> Vec<SceneConfig> {
        serde_json::from_value(serde_json::json!([
            { "type": "terminal", "name": "demo", "command": "true" },
            { "type": "terminal", "name": "cli-help", "command": "true" },
            { "type": "web", "name": "site", "uri": "https://example.com" },
        ]))
        .unwrap()
    }

    fn names(scenes: &[SceneConfig]) -> Vec<&str> {
        scenes.iter().map(|s| s.name()).collect()
    }

    fn filter(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn filter_keeps_only_named_scenes_in_config_order() {
        let mut s = scenes();
        filter_scenes(&mut s, &filter(&["site", "DEMO"])).unwrap();
        assert_eq!(names(&s), ["demo", "site"]);
    }

    #[test]
    fn filter_matches_scene_type() {
        let mut s = scenes();
        filter_scenes(&mut s, &filter(&["terminal"])).unwrap();
        assert_eq!(names(&s), ["demo", "cli-help"]);
    }

    #[test]
    fn filter_errors_when_any_name_matches_nothing() {
        let mut s = scenes();
        let err = filter_scenes(&mut s, &filter(&["demo", "typo"])).unwrap_err();
        assert_eq!(
            err.to_string(),
            "no scene matched: typo (available: demo, cli-help, site)"
        );
        assert_eq!(s.len(), 3, "scenes are untouched on error");
    }

    #[test]
    fn scene_flag_is_repeatable_and_keeps_scenes_alias() {
        use clap::Parser;
        for args in [
            vec!["teasr", "run", "--scene", "a", "--scene", "b"],
            vec!["teasr", "run", "--scenes", "a,b"],
        ] {
            let Some(Command::Run { scenes, .. }) = Cli::parse_from(args).command else {
                panic!("expected run");
            };
            assert_eq!(scenes.unwrap(), ["a", "b"]);
        }
    }

    #[test]
    fn format_defaults_to_human_and_is_global() {
        use clap::Parser;
        assert_eq!(
            Cli::parse_from(["teasr", "run"]).format,
            ReportFormat::Human
        );
        for args in [
            ["teasr", "--format", "json", "run"],
            ["teasr", "run", "--format", "json"],
        ] {
            assert_eq!(Cli::parse_from(args).format, ReportFormat::Json);
        }
    }

    #[test]
    fn color_scheme_flag_accepts_only_light_and_dark() {
        use clap::Parser;
        let Some(Command::Run { color_scheme, .. }) =
            Cli::parse_from(["teasr", "run", "--color-scheme", "dark"]).command
        else {
            panic!("expected run");
        };
        assert_eq!(color_scheme, Some(ColorScheme::Dark));
        assert!(Cli::try_parse_from(["teasr", "run", "--color-scheme", "sepia"]).is_err());
    }

    #[test]
    fn json_report_names_failed_scenes_and_omits_error_on_success() {
        use teasr_core::types::SceneReport;
        let scene = |name: &str, status, error: Option<&str>| SceneReport {
            name: name.to_string(),
            scene_type: "web".to_string(),
            status,
            files: Vec::new(),
            error: error.map(str::to_string),
            duration_ms: 5,
        };
        let report = RunReport {
            output_dir: "out".to_string(),
            captured: 1,
            failed: 1,
            scenes: vec![
                scene("home", SceneStatus::Ok, None),
                scene("broken", SceneStatus::Failed, Some("file not found")),
            ],
        };
        assert!(!report.all_failed());
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(
            json["scenes"],
            serde_json::json!([
                { "name": "home", "type": "web", "status": "ok", "files": [], "duration_ms": 5 },
                { "name": "broken", "type": "web", "status": "failed", "files": [],
                  "error": "file not found", "duration_ms": 5 },
            ])
        );
    }
}
