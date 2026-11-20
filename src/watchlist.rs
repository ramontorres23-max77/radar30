//! Watchlist management & batch execution over the SQLite store.

use std::fmt::Write as _;
use std::path::Path;
use std::time::Duration;

use anyhow::Result;
use serde::Serialize;

use crate::library;
use crate::pipeline::{self, RunOptions};
use crate::schema::{Depth, SourceId};
use crate::sources::{self, SourceFetcher};
use crate::store::{PersistSummary, Store, TopicRecord};

#[derive(Clone, Debug, Serialize)]
pub struct WatchRunItem {
    pub topic: String,
    pub status: String,
    pub findings_new: usize,
    pub findings_updated: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub brief_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct WatchRunReport {
    pub ran: usize,
    pub total_new: usize,
    pub total_updated: usize,
    pub results: Vec<WatchRunItem>,
}

/// Execute all enabled watchlist topics (or a single named topic) and persist findings + briefs.
pub fn run_watchlist(
    store: &mut Store,
    only_topic: Option<&str>,
    depth: Depth,
    days: u64,
    timeout: Duration,
    briefs_dir: Option<&Path>,
    fetcher: &dyn SourceFetcher,
) -> Result<WatchRunReport> {
    let topics: Vec<TopicRecord> = store
        .list_topics()?
        .into_iter()
        .filter(|t| match only_topic {
            Some(target) => t.name.eq_ignore_ascii_case(target.trim()),
            None => t.enabled,
        })
        .collect();

    let window = RunOptions::resolve_window(days, None, None)?;
    let mut results = Vec::new();
    let mut total_new = 0usize;
    let mut total_updated = 0usize;

    for topic in topics {
        let selected = match topic.sources.as_deref() {
            Some(s) if !s.trim().is_empty() => {
                sources::parse_source_list(s).unwrap_or_else(|_| SourceId::DEFAULT_ENABLED.to_vec())
            }
            _ => SourceId::DEFAULT_ENABLED.to_vec(),
        };
        let opts = RunOptions {
            topic: topic.name.clone(),
            depth,
            window,
            sources: selected,
            timeout,
        };
        match pipeline::run(&opts, fetcher) {
            Ok(report) => {
                let summary: PersistSummary = store.record_report(&report)?;
                total_new += summary.findings_new;
                total_updated += summary.findings_updated;
                let brief_path = briefs_dir
                    .and_then(|dir| library::save_brief(&report, dir).ok())
                    .map(|p| p.display().to_string());
                let status = if report.degraded_sources().is_empty() {
                    "ok".to_string()
                } else {
                    "degraded".to_string()
                };
                results.push(WatchRunItem {
                    topic: topic.name,
                    status,
                    findings_new: summary.findings_new,
                    findings_updated: summary.findings_updated,
                    brief_path,
                    error: None,
                });
            }
            Err(err) => {
                results.push(WatchRunItem {
                    topic: topic.name,
                    status: "failed".to_string(),
                    findings_new: 0,
                    findings_updated: 0,
                    brief_path: None,
                    error: Some(format!("{err:#}")),
                });
            }
        }
    }

    Ok(WatchRunReport {
        ran: results.len(),
        total_new,
        total_updated,
        results,
    })
}

pub fn format_list(topics: &[TopicRecord]) -> String {
    let mut out = String::new();
    if topics.is_empty() {
        let _ = writeln!(
            out,
            "Watchlist is empty. Add a topic with: radar30 watch add \"<topic>\""
        );
        return out;
    }
    let _ = writeln!(out, "Watchlist ({} topic(s)):", topics.len());
    for t in topics {
        let state = if t.enabled { "enabled" } else { "paused " };
        let last = t.last_run_at.as_deref().unwrap_or("never");
        let _ = writeln!(
            out,
            "  - [{}] {} (schedule: {}, findings: {}, last run: {})",
            state, t.name, t.schedule, t.total_findings, last
        );
    }
    out
}

pub fn format_run_report(report: &WatchRunReport) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "radar30 watch run: {} topic(s), {} new, {} updated",
        report.ran, report.total_new, report.total_updated
    );
    for r in &report.results {
        let _ = writeln!(
            out,
            "  - [{}] {} (+{} new, {} updated)",
            r.status, r.topic, r.findings_new, r.findings_updated
        );
        if let Some(ref p) = r.brief_path {
            let _ = writeln!(out, "    brief: {p}");
        }
        if let Some(ref e) = r.error {
            let _ = writeln!(out, "    error: {e}");
        }
    }
    out
}
