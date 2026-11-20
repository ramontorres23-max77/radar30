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
use radar30::store::{self, Store};
use radar30::{
    briefing, creds, discover, doctor, library, mcp, preflight, render, setup, watchlist,
};

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
        /// Comma-separated sources: reddit, hackernews/hn, github/gh, polymarket/pm, bluesky/bsky, devto, arxiv, x/twitter, youtube/yt, tiktok, instagram, threads, linkedin, pinterest, perplexity, web/brave (BYOK lanes need credentials).
        #[arg(long)]
        sources: Option<String>,
        /// Output format: compact (agent-friendly), md (Markdown brief), or json (`radar30/research@1`).
        #[arg(long, value_enum, default_value_t = EmitFormat::Compact)]
        emit: EmitFormat,
        /// Write rendered output to this file instead of stdout.
        #[arg(long, short)]
        out: Option<PathBuf>,
        /// Persist sightings into the SQLite research store (`--db` overrides default path).
        #[arg(long)]
        store: bool,
        /// Also save a dated Markdown brief into this directory (defaults to `~/Documents/Radar30` when `--save` is used).
        #[arg(long)]
        save_dir: Option<PathBuf>,
        /// Override the SQLite database path (defaults to `~/.local/share/radar30/research.db`).
        #[arg(long)]
        db: Option<PathBuf>,
        /// Wall-clock timeout budget in seconds.
        #[arg(long, default_value_t = 25)]
        timeout: u64,
        /// Load `<source>.json` arrays from a local directory instead of calling the network.
        #[arg(long)]
        offline_fixtures: Option<PathBuf>,
    },
    /// Investigate a company: US patent publications, careers-page signals, and a finance line.
    Company {
        company: String,
        /// Optional ticker override for the 30-day stock benchmark line.
        #[arg(long)]
        ticker: Option<String>,
        /// Override job boards (e.g. `acme=greenhouse:acme,beta=ashby:beta`).
        #[arg(long)]
        boards: Option<String>,
        #[arg(long, default_value_t = 30)]
        days: u64,
        #[arg(long, value_enum, default_value_t = EmitFormat::Compact)]
        emit: EmitFormat,
        #[arg(long, short)]
        out: Option<PathBuf>,
        #[arg(long, default_value_t = 30)]
        timeout: u64,
    },
    /// Render a saved Markdown brief as a standalone HTML page (local publisher).
    Publish {
        /// Saved `.md` brief to publish.
        #[arg(long, short)]
        input: PathBuf,
        /// Output directory for the generated `.html`.
        #[arg(long, short)]
        out_dir: PathBuf,
    },
    /// Topic-less discovery: sweep Hacker News & Reddit rivers, enforce confidence floor, track repeats.
    Discover {
        /// Optional domain scope (e.g. "AI agents", "Rust"). Omit for global sweep.
        domain: Option<String>,
        #[arg(long, value_enum, default_value_t = Depth::Default)]
        depth: Depth,
        /// Wall-clock timeout budget in seconds.
        #[arg(long, default_value_t = 25)]
        timeout: u64,
        /// Skip writing surfaced topics to the discovery topic queue.
        #[arg(long)]
        no_queue: bool,
        /// Override the SQLite database path for the topic queue.
        #[arg(long)]
        db: Option<PathBuf>,
        /// Print machine-readable JSON (`radar30/discover@1`).
        #[arg(long)]
        json: bool,
    },
    /// Search saved briefs + SQLite sightings offline, or build a static HTML + Atom feed.
    Library {
        #[command(subcommand)]
        command: LibraryCmd,
    },
    /// Manage monitored watchlist topics and run batch updates into SQLite.
    Watch {
        #[command(subcommand)]
        command: WatchCmd,
    },
    /// Generate daily or weekly briefings from accumulated SQLite watchlist findings.
    Briefing {
        /// Generate a 7-day weekly digest instead of a 24-hour daily briefing.
        #[arg(long)]
        weekly: bool,
        /// Override lower date bound (`YYYY-MM-DD`).
        #[arg(long)]
        since: Option<String>,
        /// Override the SQLite database path.
        #[arg(long)]
        db: Option<PathBuf>,
        /// Print machine-readable JSON.
        #[arg(long)]
        json: bool,
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
    /// Show credential presence (never values) or store/delete a Keychain secret.
    Creds {
        #[command(subcommand)]
        command: Option<CredsCmd>,
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
    /// Start the HTTP MCP server (`GET /health`, `POST /mcp`) for remote clients.
    ServeHttp {
        #[arg(long, default_value = "0.0.0.0")]
        host: String,
        #[arg(long, default_value_t = 8080)]
        port: u16,
    },
}

#[derive(Subcommand)]
enum CredsCmd {
    /// Store a Keychain secret read from stdin (one line; never passed as an argument).
    Set {
        /// Known key name, e.g. X_BEARER_TOKEN.
        key: String,
    },
    /// Delete a Keychain secret by key name.
    Delete { key: String },
}

#[derive(Subcommand)]
enum LibraryCmd {
    /// Offline deterministic search across saved Markdown briefs and SQLite findings.
    Search {
        query: String,
        #[arg(long, default_value_t = 15)]
        limit: usize,
        #[arg(long)]
        briefs_dir: Option<PathBuf>,
        #[arg(long)]
        db: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// Build `index.html`, per-brief HTML pages, and a subscribable Atom `feed.xml`.
    Feed {
        #[arg(long)]
        briefs_dir: Option<PathBuf>,
        /// Output directory for `index.html`, `feed.xml`, and `briefs/*.html`.
        #[arg(long, short)]
        out_dir: PathBuf,
        /// Base public URL used in Atom `<id>` and `<link>` tags.
        #[arg(long, default_value = "https://example.org/radar30")]
        site_url: String,
        #[arg(long)]
        json: bool,
    },
    /// List discovery topic queue entries or mark a topic as `covered`.
    Queue {
        /// Mark this topic name as `covered` in the discovery queue.
        #[arg(long)]
        cover: Option<String>,
        #[arg(long)]
        db: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum WatchCmd {
    /// Add or update a topic on the watchlist.
    Add {
        topic: String,
        /// Weekly schedule (`0 8 * * 1`) instead of daily (`0 8 * * *`).
        #[arg(long)]
        weekly: bool,
        /// Custom cron schedule expression.
        #[arg(long)]
        schedule: Option<String>,
        /// Optional pinned comma-separated sources for this topic.
        #[arg(long)]
        sources: Option<String>,
        #[arg(long)]
        db: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// Remove a topic from the watchlist.
    Remove {
        topic: String,
        #[arg(long)]
        db: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// List watchlist topics and accumulated finding counts.
    List {
        #[arg(long)]
        db: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// Run all enabled watchlist topics (or one topic via `--topic`) and persist sightings.
    Run {
        #[arg(long)]
        topic: Option<String>,
        #[arg(long, value_enum, default_value_t = Depth::Quick)]
        depth: Depth,
        #[arg(long, default_value_t = 30)]
        days: u64,
        #[arg(long, default_value_t = 25)]
        timeout: u64,
        #[arg(long)]
        briefs_dir: Option<PathBuf>,
        #[arg(long)]
        db: Option<PathBuf>,
        #[arg(long)]
        offline_fixtures: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
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
            store: persist,
            save_dir,
            db,
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

            if persist {
                let db_path = resolve_db(db)?;
                let mut st = Store::open(&db_path)?;
                let summary = st.record_report(&report)?;
                eprintln!(
                    "[radar30] stored in {}: +{} new, {} updated (run #{})",
                    summary.db_path.display(),
                    summary.findings_new,
                    summary.findings_updated,
                    summary.run_id
                );
            }
            if let Some(dir) = save_dir {
                let saved = library::save_brief(&report, &dir)?;
                eprintln!("[radar30] saved brief: {}", saved.display());
            }

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
        Cmd::Company {
            company,
            ticker,
            boards,
            days,
            emit,
            out,
            timeout,
        } => {
            let window = RunOptions::resolve_window(days, None, None)?;
            let report = radar30::company::run(&radar30::company::CompanyOptions {
                company,
                window,
                ticker,
                boards_override: boards,
                timeout: Duration::from_secs(timeout.clamp(5, 120)),
            })?;
            let rendered = radar30::company::render(&report, emit)?;
            if let Some(path) = out {
                std::fs::write(&path, &rendered)
                    .with_context(|| format!("writing {}", path.display()))?;
            } else {
                say!("{rendered}");
            }
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Publish { input, out_dir } => {
            let path = library::publish_markdown(&input, &out_dir)?;
            say!("Published HTML: {}", path.display());
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Discover {
            domain,
            depth,
            timeout,
            no_queue,
            db,
            json,
        } => {
            let sweeper = discover::LiveSweeper::default();
            let st = if no_queue {
                None
            } else {
                let path = resolve_db(db)?;
                Some(Store::open(&path)?)
            };
            let report = discover::run(
                domain.as_deref(),
                depth,
                Duration::from_secs(timeout.clamp(5, 120)),
                &sweeper,
                st.as_ref(),
            )?;
            if json {
                say!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                say!("{}", discover::format_text(&report));
            }
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Library { command } => match command {
            LibraryCmd::Search {
                query,
                limit,
                briefs_dir,
                db,
                json,
            } => {
                let bdir = match briefs_dir {
                    Some(d) => d,
                    None => store::default_briefs_dir()?,
                };
                let db_path = resolve_db(db)?;
                let res = library::search(&query, &bdir, Some(&db_path), limit)?;
                if json {
                    say!("{}", serde_json::to_string_pretty(&res)?);
                } else {
                    say!("{}", library::format_search_text(&res));
                }
                Ok(ExitCode::SUCCESS)
            }
            LibraryCmd::Feed {
                briefs_dir,
                out_dir,
                site_url,
                json,
            } => {
                let bdir = match briefs_dir {
                    Some(d) => d,
                    None => store::default_briefs_dir()?,
                };
                let res = library::build_feed(&bdir, &out_dir, &site_url)?;
                if json {
                    say!("{}", serde_json::to_string_pretty(&res)?);
                } else {
                    say!(
                        "Built library feed ({} brief(s)):\n  index: {}\n  atom:  {}",
                        res.briefs_count,
                        res.index_html.display(),
                        res.feed_xml.display()
                    );
                }
                Ok(ExitCode::SUCCESS)
            }
            LibraryCmd::Queue { cover, db, json } => {
                let st = Store::open(resolve_db(db)?)?;
                if let Some(name) = cover {
                    let ok = st.mark_topic_covered(&name)?;
                    if json {
                        say!("{}", serde_json::json!({"topic": name, "covered": ok}));
                    } else if ok {
                        say!("Marked \"{name}\" as covered.");
                    } else {
                        say!("Topic \"{name}\" not found in discovery queue.");
                    }
                    return Ok(ExitCode::SUCCESS);
                }
                let entries = st.list_queue()?;
                if json {
                    say!("{}", serde_json::to_string_pretty(&entries)?);
                } else if entries.is_empty() {
                    say!("Discovery topic queue is empty.");
                } else {
                    for e in entries {
                        say!(
                            "  [{:8}] {} (surfaced {}x, velocity {:.1}, last {})",
                            e.status,
                            e.name,
                            e.surfaced_count,
                            e.last_velocity,
                            e.last_surfaced_at
                        );
                    }
                }
                Ok(ExitCode::SUCCESS)
            }
        },
        Cmd::Watch { command } => match command {
            WatchCmd::Add {
                topic,
                weekly,
                schedule,
                sources,
                db,
                json,
            } => {
                let st = Store::open(resolve_db(db)?)?;
                let sched = if weekly {
                    "0 8 * * 1".to_string()
                } else {
                    schedule.unwrap_or_else(|| "0 8 * * *".to_string())
                };
                let id = st.ensure_topic(&topic, Some(&sched), sources.as_deref())?;
                if json {
                    say!(
                        "{}",
                        serde_json::json!({
                            "action": "added",
                            "id": id,
                            "topic": topic,
                            "schedule": sched
                        })
                    );
                } else {
                    say!("Added \"{topic}\" to watchlist (id={id}, schedule={sched}).");
                }
                Ok(ExitCode::SUCCESS)
            }
            WatchCmd::Remove { topic, db, json } => {
                let st = Store::open(resolve_db(db)?)?;
                let removed = st.remove_topic(&topic)?;
                if json {
                    say!(
                        "{}",
                        serde_json::json!({"topic": topic, "removed": removed})
                    );
                } else if removed {
                    say!("Removed \"{topic}\" from watchlist.");
                } else {
                    say!("Topic \"{topic}\" not found.");
                }
                Ok(ExitCode::SUCCESS)
            }
            WatchCmd::List { db, json } => {
                let st = Store::open(resolve_db(db)?)?;
                let topics = st.list_topics()?;
                if json {
                    say!("{}", serde_json::to_string_pretty(&topics)?);
                } else {
                    say!("{}", watchlist::format_list(&topics));
                }
                Ok(ExitCode::SUCCESS)
            }
            WatchCmd::Run {
                topic,
                depth,
                days,
                timeout,
                briefs_dir,
                db,
                offline_fixtures,
                json,
            } => {
                let mut st = Store::open(resolve_db(db)?)?;
                let report = match offline_fixtures {
                    Some(dir) => {
                        let fetcher = FixtureFetcher::new(dir);
                        watchlist::run_watchlist(
                            &mut st,
                            topic.as_deref(),
                            depth,
                            days,
                            Duration::from_secs(timeout.clamp(3, 180)),
                            briefs_dir.as_deref(),
                            &fetcher,
                        )?
                    }
                    None => {
                        let fetcher = LiveFetcher::default();
                        watchlist::run_watchlist(
                            &mut st,
                            topic.as_deref(),
                            depth,
                            days,
                            Duration::from_secs(timeout.clamp(3, 180)),
                            briefs_dir.as_deref(),
                            &fetcher,
                        )?
                    }
                };
                if json {
                    say!("{}", serde_json::to_string_pretty(&report)?);
                } else {
                    say!("{}", watchlist::format_run_report(&report));
                }
                Ok(ExitCode::SUCCESS)
            }
        },
        Cmd::Briefing {
            weekly,
            since,
            db,
            json,
        } => {
            let st = Store::open(resolve_db(db)?)?;
            let report = briefing::generate(&st, weekly, since.as_deref())?;
            if json {
                say!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                say!("{}", briefing::format_markdown(&report));
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
        Cmd::Creds { command, json } => {
            match command {
                None => {
                    let statuses = creds::status();
                    if json {
                        say!("{}", serde_json::to_string_pretty(&statuses)?);
                    } else {
                        say!("radar30 credentials (presence only, values never shown):");
                        for s in &statuses {
                            let origin = s.origin.unwrap_or("absent");
                            say!(
                                "  [{:7}] {:26} {:8} — unlocks {}",
                                if s.present { "set" } else { "missing" },
                                s.key,
                                origin,
                                s.unlocks
                            );
                        }
                        if let Some(p) = creds::dotenv_present() {
                            say!("  .env file: {}", p.display());
                        }
                    }
                }
                Some(CredsCmd::Set { key }) => {
                    if !creds::KNOWN_KEYS.iter().any(|(k, _)| *k == key) {
                        anyhow::bail!("unknown key `{key}` (see `radar30 creds`)");
                    }
                    use std::io::BufRead as _;
                    let mut line = String::new();
                    std::io::stdin().lock().read_line(&mut line)?;
                    let secret = line.trim_end_matches(['\r', '\n']).to_string();
                    if secret.is_empty() {
                        anyhow::bail!("empty secret on stdin; nothing stored");
                    }
                    creds::keychain_set(&key, &secret)?;
                    say!("Stored {key} in macOS Keychain (service radar30-{key}).");
                }
                Some(CredsCmd::Delete { key }) => {
                    let removed = creds::keychain_delete(&key)?;
                    if json {
                        say!("{}", serde_json::json!({"key": key, "removed": removed}));
                    } else if removed {
                        say!("Deleted {key} from macOS Keychain.");
                    } else {
                        say!("No Keychain item found for {key}.");
                    }
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
        Cmd::ServeHttp { host, port } => {
            radar30::http_server::serve(&host, port)?;
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

fn resolve_db(override_path: Option<PathBuf>) -> Result<PathBuf> {
    match override_path {
        Some(p) => Ok(p),
        None => store::default_db_path(),
    }
}
