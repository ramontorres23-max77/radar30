//! Zero-network, zero-write preflight inspection (`radar30 preflight`).

use std::fmt::Write as _;
use std::path::PathBuf;

use serde::Serialize;

use crate::query::build_plan;
use crate::schema::{DateWindow, Depth, PREFLIGHT_SCHEMA, QueryPlan, SourceId};

/// Machine-readable preflight forecast.
#[derive(Clone, Debug, Serialize)]
pub struct PreflightReport {
    pub schema: String,
    pub version: String,
    pub topic: Option<String>,
    pub depth: Depth,
    pub window: DateWindow,
    pub sources: Vec<PlannedSource>,
    pub planned_writes: Vec<PathBuf>,
    pub github_token_present: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan: Option<QueryPlan>,
}

#[derive(Clone, Debug, Serialize)]
pub struct PlannedSource {
    pub id: SourceId,
    pub name: &'static str,
    pub endpoint: &'static str,
    pub auth_mode: &'static str,
}

/// Build a preflight report without performing any network calls or file writes.
pub fn inspect(
    topic: Option<&str>,
    depth: Depth,
    window: DateWindow,
    sources: &[SourceId],
    out_file: Option<PathBuf>,
) -> PreflightReport {
    let github_token_present = std::env::var("GITHUB_TOKEN")
        .ok()
        .is_some_and(|s| !s.trim().is_empty());

    let planned_sources = sources
        .iter()
        .map(|&id| PlannedSource {
            id,
            name: id.display_name(),
            endpoint: id.endpoint_summary(),
            auth_mode: match id {
                SourceId::GitHub if github_token_present => "authenticated (GITHUB_TOKEN)",
                SourceId::GitHub => "anonymous (set GITHUB_TOKEN for higher limits)",
                _ => "keyless",
            },
        })
        .collect();

    let plan = topic
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|t| build_plan(t, sources));

    let planned_writes = out_file.into_iter().collect();

    PreflightReport {
        schema: PREFLIGHT_SCHEMA.to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        topic: topic.map(ToOwned::to_owned),
        depth,
        window,
        sources: planned_sources,
        planned_writes,
        github_token_present,
        plan,
    }
}

/// Human-readable summary of a `PreflightReport`.
pub fn format_text(report: &PreflightReport) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "radar30 preflight (no network calls, no writes made)");
    if let Some(ref topic) = report.topic {
        let _ = writeln!(out, "  topic:   {topic}");
    }
    if let Some(ref plan) = report.plan {
        let _ = writeln!(
            out,
            "  entity:  {} (head: {}, intent: {:?}, subqueries: {})",
            plan.primary_entity,
            plan.head_token,
            plan.intent,
            plan.subqueries.len()
        );
    }
    let _ = writeln!(
        out,
        "  window:  {} .. {} ({} days)",
        report.window.from,
        report.window.to,
        report.window.days()
    );
    let _ = writeln!(out, "  depth:   {}", report.depth.as_str());
    let _ = writeln!(out, "  sources:");
    for s in &report.sources {
        let _ = writeln!(
            out,
            "    - {:11} {} [{}]",
            s.id.as_str(),
            s.endpoint,
            s.auth_mode
        );
    }
    if report.planned_writes.is_empty() {
        let _ = writeln!(out, "  writes:  none (stdout only)");
    } else {
        let _ = writeln!(out, "  writes:");
        for w in &report.planned_writes {
            let _ = writeln!(out, "    - {}", w.display());
        }
    }
    out
}
