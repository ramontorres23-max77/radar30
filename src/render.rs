//! Deterministic output renderers: `compact` (agent-facing), `md` (Markdown brief), and `json`.

use std::collections::HashMap;
use std::fmt::Write as _;

use anyhow::Result;

use crate::schema::{Candidate, EmitFormat, Engagement, ResearchReport, SourceState};

/// Canonical end-of-output boundary marker (modeled after `last30days`).
pub const CANONICAL_OUTPUT_END: &str = "# END OF radar30 CANONICAL OUTPUT";

/// Render a `ResearchReport` in the requested format.
pub fn render(report: &ResearchReport, format: EmitFormat) -> Result<String> {
    match format {
        EmitFormat::Json => Ok(serde_json::to_string_pretty(report)?),
        EmitFormat::Compact => Ok(render_compact(report)),
        EmitFormat::Md => Ok(render_markdown(report)),
    }
}

fn render_compact(report: &ResearchReport) -> String {
    let by_id: HashMap<&str, &Candidate> = report
        .candidates
        .iter()
        .map(|c| (c.candidate_id.as_str(), c))
        .collect();

    let mut out = String::new();
    let _ = writeln!(
        out,
        "# radar30: {} ({} .. {}, depth={})",
        report.topic,
        report.window.from,
        report.window.to,
        report.depth.as_str()
    );
    let _ = writeln!(
        out,
        "Entity: {} | Intent: {:?} | Fetched: {} | Candidates: {} | Clusters: {}",
        report.primary_entity,
        report.intent,
        report.total_fetched,
        report.candidates.len(),
        report.clusters.len()
    );
    let _ = writeln!(out);

    if report.clusters.is_empty() {
        let _ = writeln!(out, "No matching evidence found in the requested window.");
    } else {
        for (idx, cluster) in report.clusters.iter().take(12).enumerate() {
            let srcs = cluster
                .sources
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            let unc = cluster
                .uncertainty
                .map(|u| format!(" [{u:?}]"))
                .unwrap_or_default();
            let _ = writeln!(
                out,
                "## {}. {} (score {:.1} · {}){}",
                idx + 1,
                cluster.title,
                cluster.score,
                srcs,
                unc
            );
            for rep_id in &cluster.representative_ids {
                if let Some(c) = by_id.get(rep_id.as_str()) {
                    let eng = format_engagement(&c.engagement);
                    let date = c.published_at.as_deref().unwrap_or("undated");
                    let _ = writeln!(
                        out,
                        "- [{}] {} ({}{}) — {}",
                        c.primary_source.as_str(),
                        c.title,
                        date,
                        eng,
                        c.url
                    );
                    if !c.snippet.trim().is_empty() {
                        let _ = writeln!(out, "  > {}", collapse_ws(&c.snippet, 180));
                    }
                }
            }
            let _ = writeln!(out);
        }
    }

    let _ = writeln!(out, "## Source coverage");
    for o in &report.source_outcomes {
        let icon = match o.state {
            SourceState::Ok => "ok",
            SourceState::NoResults => "empty",
            SourceState::Partial => "partial",
            _ => "degraded",
        };
        let detail = o
            .detail
            .as_deref()
            .map(|d| format!(" — {d}"))
            .unwrap_or_default();
        let _ = writeln!(
            out,
            "- {}: {} ({} items, {} ms){}",
            o.source.as_str(),
            icon,
            o.item_count,
            o.elapsed_ms,
            detail
        );
    }

    if !report.warnings.is_empty() {
        let _ = writeln!(out, "\n## Warnings");
        for w in &report.warnings {
            let _ = writeln!(out, "- {w}");
        }
    }

    let _ = writeln!(out, "\n---\n{CANONICAL_OUTPUT_END}");
    out
}

fn render_markdown(report: &ResearchReport) -> String {
    let by_id: HashMap<&str, &Candidate> = report
        .candidates
        .iter()
        .map(|c| (c.candidate_id.as_str(), c))
        .collect();

    let mut out = String::new();
    let _ = writeln!(out, "# Research Brief: {}\n", report.topic);
    let _ = writeln!(
        out,
        "- **Window:** {} to {}",
        report.window.from, report.window.to
    );
    let _ = writeln!(out, "- **Primary entity:** {}", report.primary_entity);
    let _ = writeln!(out, "- **Depth:** {}", report.depth.as_str());
    let _ = writeln!(out, "- **Generated:** {}\n", report.generated_at);

    let _ = writeln!(out, "## Top Ranked Stories\n");
    if report.clusters.is_empty() {
        let _ = writeln!(out, "_No evidence matched the query window._\n");
    } else {
        for (idx, cluster) in report.clusters.iter().take(15).enumerate() {
            let srcs = cluster
                .sources
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            let _ = writeln!(
                out,
                "### {}. {} (`{:.1}` · {})\n",
                idx + 1,
                cluster.title,
                cluster.score,
                srcs
            );
            for rep_id in &cluster.representative_ids {
                if let Some(c) = by_id.get(rep_id.as_str()) {
                    let eng = format_engagement(&c.engagement);
                    let date = c.published_at.as_deref().unwrap_or("undated");
                    let _ = writeln!(
                        out,
                        "- **[{}]** [{}]({}) ({}{})",
                        c.primary_source.as_str(),
                        c.title,
                        c.url,
                        date,
                        eng
                    );
                    if !c.snippet.trim().is_empty() {
                        let _ = writeln!(out, "  - {}", collapse_ws(&c.snippet, 220));
                    }
                }
            }
            let _ = writeln!(out);
        }
    }

    let _ = writeln!(out, "## Source Telemetry\n");
    let _ = writeln!(out, "| Source | State | Items | Time (ms) | Detail |");
    let _ = writeln!(out, "| --- | --- | ---: | ---: | --- |");
    for o in &report.source_outcomes {
        let _ = writeln!(
            out,
            "| {} | {:?} | {} | {} | {} |",
            o.source.as_str(),
            o.state,
            o.item_count,
            o.elapsed_ms,
            o.detail.as_deref().unwrap_or("")
        );
    }
    out
}

fn format_engagement(e: &Engagement) -> String {
    let mut parts = Vec::new();
    if let Some(s) = e.score {
        parts.push(format!("{s} pts"));
    }
    if let Some(c) = e.comments {
        parts.push(format!("{c} comments"));
    }
    if let Some(st) = e.stars {
        parts.push(format!("{st} stars"));
    }
    if let Some(p) = e.probability {
        parts.push(format!("{:.0}% odds", p * 100.0));
    }
    if let Some(v) = e.volume_usd {
        if v >= 1_000_000.0 {
            parts.push(format!("${:.1}M vol", v / 1_000_000.0));
        } else if v >= 1_000.0 {
            parts.push(format!("${:.1}K vol", v / 1_000.0));
        } else if v > 0.0 {
            parts.push(format!("${v:.0} vol"));
        }
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!(" · {}", parts.join(", "))
    }
}

fn collapse_ws(s: &str, max_chars: usize) -> String {
    let joined = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if joined.chars().count() <= max_chars {
        joined
    } else {
        let mut out: String = joined.chars().take(max_chars).collect();
        out.push_str("...");
        out
    }
}
