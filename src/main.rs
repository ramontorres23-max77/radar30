#![forbid(unsafe_code)]

use std::io::Write as _;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use anyhow::{Context, Result};
use chrono::NaiveDate;
use clap::{Parser, Subcommand};

use radar30::pipeline::{self, RunOptions};
use radar30::schema::{Depth, EmitFormat, SourceId};
use radar30::sources::{self, FixtureFetcher, LiveFetcher};
use radar30::{doctor, mcp, preflight, render, setup};

/// `println!` that stops quietly when stdout is closed (`radar30 ... | head`).
macro_rules! say {
    ($($arg:tt)*) => {{
        let _ = writeln!(std::io::stdout().lock(), $($arg)*);
    }};
}

#[derive(Parser)]
#[command(
    name = "radar30",
    version,
    about = "Multi-source, engagement-scored research engine over the last 30 days (CLI + MCP server + Skill)",
    after_help = "Quick start: radar30 preflight \"Rust\" && radar30 research \"Rust\" --depth quick"
)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run a multi-source research pass scored by engagement, recency, and entity grounding.
    Research {
        /// Topic to research (person, company, product, comparison, or event).
        topic: String,
        /// Lookback window in days (ignored when --from is specified).
        #[arg(long, default_value_t = 30)]
        days: u64,
        /// Start date (YYYY-MM-DD).
        #[arg(long)]
        from: Option<NaiveDate>,
        /// End date (YYYY-MM-DD; defaults to today UTC).
        #[arg(long)]
        to: Option<NaiveDate>,
        /// Search depth profile.
        #[arg(long, value_enum, default_value_t = Depth::Default)]
        depth: Depth,
        /// Comma-separated sources (reddit, hackernews/hn, github/gh, polymarket/pm, devto, arxiv).
        #[arg(long)]
        sources: Option<String>,
        /// Output format: compact (agent-friendly), md (Markdown brief), or json (`radar30/research@1`).
        #[arg(long, value_enum, default_value_t = EmitFormat::Compact)]
        emit: EmitFormat,
        /// Write rendered output to this file instead of stdout.
        #[arg(long, short)]
        out: Option<PathBuf>,
        /// Wall-clock timeout budget in seconds.
        #[arg(long, default_value_t = 25)]
        timeout: u64,
        /// Load `<source>.json` arrays from a local directory instead of calling the network.
        #[arg(long)]
        offline_fixtures: Option<PathBuf>,
    },
    /// Inspect planned sources, endpoints, query plan, and writes without touching the network.
    Preflight {
        /// Optional topic to plan.
        topic: Option<String>,
        #[arg(long, default_value_t = 30)]
        days: u64,
        #[arg(long)]
        from: Option<NaiveDate>,
        #[arg(long)]
        to: Option<NaiveDate>,
        #[arg(long, value_enum, default_value_t = Depth::Default)]
        depth: Depth,
        #[arg(long)]
        sources: Option<String>,
        /// Planned output file (not written).
        #[arg(long, short)]
        out: Option<PathBuf>,
        /// Print machine-readable JSON (`radar30/preflight@1`).
        #[arg(long)]
        json: bool,
    },
    /// List all supported sources, auth modes, and endpoints.
    Sources {
        #[arg(long)]
        json: bool,
    },
    /// Check binary health, optional auth env, local agent configs, and run a live MCP self-test.
    Doctor {
        #[arg(long)]
        json: bool,
    },
    /// Register `radar30 mcp` and its embedded `SKILL.md` with local coding agents.
    Setup {
        /// Agents to configure (auto-detects installed agents when omitted).
        #[arg(long = "agent", value_enum)]
        agents: Vec<setup::Agent>,
        /// Remove `radar30` entries instead of adding them.
        #[arg(long)]
        remove: bool,
        /// Show planned changes without modifying any files.
        #[arg(long)]
        dry_run: bool,
        /// Print machine-readable JSON report.
        #[arg(long)]
        json: bool,
    },
    /// Start the stdio JSON-RPC 2.0 MCP server.
    Mcp,
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<ExitCode> {
    let cli = Cli::parse();
    match cli.command {
        Cmd::Research {
            topic,
            days,
            from,
            to,
            depth,
            sources: raw_sources,
            emit,
            out,
            timeout,
            offline_fixtures,
        } => {
            let window = RunOptions::resolve_window(days, from, to)?;
            let selected = resolve_sources(raw_sources.as_deref())?;
            let opts = RunOptions {
                topic,
                depth,
                window,
                sources: selected,
                timeout: Duration::from_secs(timeout.clamp(3, 180)),
            };
            let report = match offline_fixtures {
                Some(dir) => {
                    let fetcher = FixtureFetcher::new(dir);
                    pipeline::run(&opts, &fetcher)?
                }
                None => {
                    let fetcher = LiveFetcher::default();
                    pipeline::run(&opts, &fetcher)?
                }
            };
            let rendered = render::render(&report, emit)?;
            if let Some(path) = out {
                if let Some(parent) = path.parent()
                    && !parent.as_os_str().is_empty()
                {
                    std::fs::create_dir_all(parent)
                        .with_context(|| format!("creating {}", parent.display()))?;
                }
                std::fs::write(&path, rendered)
                    .with_context(|| format!("writing {}", path.display()))?;
            } else {
                say!("{rendered}");
            }
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Preflight {
            topic,
            days,
            from,
            to,
            depth,
            sources: raw_sources,
            out,
            json,
        } => {
            let window = RunOptions::resolve_window(days, from, to)?;
            let selected = resolve_sources(raw_sources.as_deref())?;
            let report = preflight::inspect(topic.as_deref(), depth, window, &selected, out);
            if json {
                say!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                say!("{}", preflight::format_text(&report));
            }
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Sources { json } => {
            let all = sources::describe_all();
            if json {
                say!("{}", serde_json::to_string_pretty(&all)?);
            } else {
                say!(
                    "Supported sources in radar30 v{}:",
                    env!("CARGO_PKG_VERSION")
                );
                for s in all {
                    let flag = if s.default_enabled {
                        "default"
                    } else {
                        "opt-in "
                    };
                    say!(
                        "  {:11} [{}] {} — {} ({})",
                        s.id.as_str(),
                        flag,
                        s.name,
                        s.endpoint,
                        s.auth
                    );
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Doctor { json } => {
            let report = doctor::run();
            if json {
                say!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                say!("{}", doctor::format_text(&report));
            }
            Ok(if report.ok() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            })
        }
        Cmd::Setup {
            agents,
            remove,
            dry_run,
            json,
        } => {
            let home = setup::home()?;
            let command = std::env::current_exe()
                .and_then(|p| std::fs::canonicalize(&p).or(Ok(p)))
                .unwrap_or_else(|_| PathBuf::from(setup::NAME));
            let report = setup::run(&setup::Options {
                home: &home,
                agents,
                command,
                remove,
                dry_run,
            })?;
            if json {
                say!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                let prefix = if report.dry_run { "[dry-run] " } else { "" };
                for c in &report.changes {
                    say!(
                        "{prefix}{:12} {:9} {}",
                        c.agent.name(),
                        c.action,
                        c.file.display()
                    );
                    if let Some(ref bak) = c.backup {
                        say!("             backup: {}", bak.display());
                    }
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Mcp => {
            mcp::serve()?;
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn resolve_sources(raw: Option<&str>) -> Result<Vec<SourceId>> {
    match raw {
        Some(s) if !s.trim().is_empty() => sources::parse_source_list(s),
        _ => Ok(SourceId::DEFAULT_ENABLED.to_vec()),
    }
}
