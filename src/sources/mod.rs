//! Keyless source adapters and injectable `SourceFetcher` trait for deterministic testing.

pub mod arxiv;
pub mod bluesky;
pub mod devto;
pub mod github;
pub mod hackernews;
pub mod perplexity;
pub mod polymarket;
pub mod reddit;
pub mod scrapecreators;
pub mod web;
pub mod x;
pub mod youtube;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::schema::{DateWindow, Depth, SourceId, SourceItem, SourceState};

/// Result of fetching one source for one query.
#[derive(Clone, Debug)]
pub struct FetchOutcome {
    pub source: SourceId,
    pub state: SourceState,
    pub items: Vec<SourceItem>,
    pub elapsed_ms: u64,
    pub detail: Option<String>,
}

/// Metadata describing a supported source (for `radar30 sources` and `preflight`).
#[derive(Clone, Debug, Serialize)]
pub struct SourceDescriptor {
    pub id: SourceId,
    pub name: &'static str,
    pub default_enabled: bool,
    pub auth: &'static str,
    pub endpoint: &'static str,
}

pub fn describe_all() -> Vec<SourceDescriptor> {
    SourceId::ALL
        .into_iter()
        .map(|id| SourceDescriptor {
            id,
            name: id.display_name(),
            default_enabled: SourceId::DEFAULT_ENABLED.contains(&id),
            auth: match id {
                SourceId::GitHub => "keyless (optional GITHUB_TOKEN for higher rate limits)",
                SourceId::Bluesky => {
                    "optional BLUESKY_HANDLE/BLUESKY_APP_PASSWORD, else public mirror"
                }
                SourceId::X => "requires X_BEARER_TOKEN",
                SourceId::YouTube => "requires yt-dlp on PATH",
                SourceId::TikTok
                | SourceId::Instagram
                | SourceId::Threads
                | SourceId::LinkedIn
                | SourceId::Pinterest => "requires SCRAPECREATORS_API_KEY",
                SourceId::Perplexity => "requires PERPLEXITY_API_KEY",
                SourceId::Web => "requires BRAVE_API_KEY",
                _ => "keyless (public API)",
            },
            endpoint: id.endpoint_summary(),
        })
        .collect()
}

/// Credential or local binary required to activate a lane (`None` for keyless sources).
pub fn required_credential(source: SourceId) -> Option<&'static str> {
    match source {
        SourceId::X => Some("X_BEARER_TOKEN"),
        SourceId::TikTok
        | SourceId::Instagram
        | SourceId::Threads
        | SourceId::LinkedIn
        | SourceId::Pinterest => Some("SCRAPECREATORS_API_KEY"),
        SourceId::Perplexity => Some("PERPLEXITY_API_KEY"),
        SourceId::Web => Some("BRAVE_API_KEY"),
        _ => None,
    }
}

/// Parse a comma-separated list of source names/aliases (e.g. `"reddit,hn,gh"`).
pub fn parse_source_list(raw: &str) -> anyhow::Result<Vec<SourceId>> {
    let mut out = Vec::new();
    for part in raw.split(',') {
        let trimmed = part.trim();
        if trimmed.is_empty() {
            continue;
        }
        let id: SourceId = trimmed
            .parse()
            .map_err(|e: String| anyhow::anyhow!("{e}"))?;
        if !out.contains(&id) {
            out.push(id);
        }
    }
    if out.is_empty() {
        anyhow::bail!("--sources requires at least one valid source");
    }
    Ok(out)
}

/// Abstraction over live network adapters and offline fixture replays.
pub trait SourceFetcher: Send + Sync {
    fn fetch(
        &self,
        source: SourceId,
        query: &str,
        window: DateWindow,
        depth: Depth,
        deadline: Instant,
    ) -> FetchOutcome;
}

/// Live HTTP source fetcher backed by real public APIs.
#[derive(Clone, Debug)]
pub struct LiveFetcher {
    pub per_request_timeout: Duration,
}

impl Default for LiveFetcher {
    fn default() -> Self {
        Self {
            per_request_timeout: Duration::from_secs(12),
        }
    }
}

impl SourceFetcher for LiveFetcher {
    fn fetch(
        &self,
        source: SourceId,
        query: &str,
        window: DateWindow,
        depth: Depth,
        deadline: Instant,
    ) -> FetchOutcome {
        let started = Instant::now();
        // BYOK lanes degrade visibly instead of failing the whole run when unconfigured.
        if source == SourceId::YouTube && !crate::process::on_path("yt-dlp") {
            return FetchOutcome {
                source,
                state: SourceState::Unavailable,
                items: Vec::new(),
                elapsed_ms: started.elapsed().as_millis() as u64,
                detail: Some("yt-dlp not on PATH; install it (brew install yt-dlp)".to_string()),
            };
        }
        if let Some(key) = required_credential(source)
            && crate::creds::resolve(key).is_none()
        {
            return FetchOutcome {
                source,
                state: SourceState::Unavailable,
                items: Vec::new(),
                elapsed_ms: started.elapsed().as_millis() as u64,
                detail: Some(format!(
                    "missing {key}; set it via env, ~/.config/radar30/.env, or `radar30 creds set {key} --stdin`"
                )),
            };
        }

        let res = match source {
            SourceId::HackerNews => {
                hackernews::fetch(query, window, depth, self.per_request_timeout, deadline)
            }
            SourceId::Reddit => {
                reddit::fetch(query, window, depth, self.per_request_timeout, deadline)
            }
            SourceId::GitHub => {
                github::fetch(query, window, depth, self.per_request_timeout, deadline)
            }
            SourceId::Polymarket => {
                polymarket::fetch(query, window, depth, self.per_request_timeout, deadline)
            }
            SourceId::DevTo => {
                devto::fetch(query, window, depth, self.per_request_timeout, deadline)
            }
            SourceId::Arxiv => {
                arxiv::fetch(query, window, depth, self.per_request_timeout, deadline)
            }
            SourceId::Bluesky => {
                bluesky::fetch(query, window, depth, self.per_request_timeout, deadline)
            }
            SourceId::YouTube => {
                youtube::fetch(query, window, depth, self.per_request_timeout, deadline)
            }
            SourceId::X => x::fetch(
                query,
                window,
                depth,
                self.per_request_timeout,
                deadline,
                &crate::creds::resolve("X_BEARER_TOKEN").unwrap_or_default(),
            ),
            SourceId::TikTok
            | SourceId::Instagram
            | SourceId::Threads
            | SourceId::LinkedIn
            | SourceId::Pinterest => scrapecreators::fetch(
                source,
                query,
                window,
                depth,
                self.per_request_timeout,
                deadline,
                &crate::creds::resolve("SCRAPECREATORS_API_KEY").unwrap_or_default(),
            ),
            SourceId::Perplexity => perplexity::fetch(
                query,
                window,
                depth,
                self.per_request_timeout,
                deadline,
                &crate::creds::resolve("PERPLEXITY_API_KEY").unwrap_or_default(),
            ),
            SourceId::Web => web::fetch(
                query,
                window,
                depth,
                self.per_request_timeout,
                deadline,
                &crate::creds::resolve("BRAVE_API_KEY").unwrap_or_default(),
            ),
        };
        let elapsed_ms = started.elapsed().as_millis() as u64;
        match res {
            Ok((items, detail)) => {
                let state = if items.is_empty() {
                    SourceState::NoResults
                } else {
                    SourceState::Ok
                };
                FetchOutcome {
                    source,
                    state,
                    items,
                    elapsed_ms,
                    detail,
                }
            }
            Err(err) => FetchOutcome {
                source,
                state: err.state(),
                items: Vec::new(),
                elapsed_ms,
                detail: Some(err.to_string()),
            },
        }
    }
}

/// Deterministic offline fetcher that reads `<dir>/<source>.json` arrays of `SourceItem`.
#[derive(Clone, Debug)]
pub struct FixtureFetcher {
    pub dir: PathBuf,
}

impl FixtureFetcher {
    pub fn new(dir: impl AsRef<Path>) -> Self {
        Self {
            dir: dir.as_ref().to_path_buf(),
        }
    }
}

impl SourceFetcher for FixtureFetcher {
    fn fetch(
        &self,
        source: SourceId,
        _query: &str,
        _window: DateWindow,
        _depth: Depth,
        _deadline: Instant,
    ) -> FetchOutcome {
        let started = Instant::now();
        let file = self.dir.join(format!("{}.json", source.as_str()));
        if !file.is_file() {
            return FetchOutcome {
                source,
                state: SourceState::NoResults,
                items: Vec::new(),
                elapsed_ms: started.elapsed().as_millis() as u64,
                detail: Some(format!("no fixture file {}", file.display())),
            };
        }
        match std::fs::read_to_string(&file)
            .ok()
            .and_then(|s| serde_json::from_str::<Vec<SourceItem>>(&s).ok())
        {
            Some(items) => {
                let state = if items.is_empty() {
                    SourceState::NoResults
                } else {
                    SourceState::Ok
                };
                FetchOutcome {
                    source,
                    state,
                    items,
                    elapsed_ms: started.elapsed().as_millis() as u64,
                    detail: Some("loaded from offline fixture".to_string()),
                }
            }
            None => FetchOutcome {
                source,
                state: SourceState::Error,
                items: Vec::new(),
                elapsed_ms: started.elapsed().as_millis() as u64,
                detail: Some(format!("invalid fixture JSON in {}", file.display())),
            },
        }
    }
}
