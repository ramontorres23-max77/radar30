//! Daily and weekly briefing generator over accumulated SQLite watchlist findings.

use std::fmt::Write as _;

use anyhow::Result;
use chrono::{Duration, Utc};
use serde::Serialize;

use crate::store::{Store, StoredFinding};

#[derive(Clone, Debug, Serialize)]
pub struct TopicBriefing {
    pub name: String,
    pub new_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_run_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_status: Option<String>,
    pub findings: Vec<StoredFinding>,
}

#[derive(Clone, Debug, Serialize)]
pub struct BriefingReport {
    pub period: String,
    pub since: String,
    pub generated_at: String,
    pub total_topics: usize,
    pub total_new: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_finding: Option<StoredFinding>,
    pub topics: Vec<TopicBriefing>,
}

/// Generate a daily (last 1 day) or weekly (last 7 days) briefing from the SQLite store.
pub fn generate(
    store: &Store,
    weekly: bool,
    since_override: Option<&str>,
) -> Result<BriefingReport> {
    let lookback_days = if weekly { 7 } else { 1 };
    let since = since_override.map(ToOwned::to_owned).unwrap_or_else(|| {
        (Utc::now() - Duration::days(lookback_days))
            .format("%Y-%m-%d")
            .to_string()
    });

    let enabled: Vec<_> = store
        .list_topics()?
        .into_iter()
        .filter(|t| t.enabled)
        .collect();

    let mut topics = Vec::with_capacity(enabled.len());
    let mut total_new = 0usize;
    let mut top_finding: Option<StoredFinding> = None;

    for t in enabled {
        let findings = store.findings_since(t.id, &since)?;
        total_new += findings.len();
        for f in &findings {
            let replace = match &top_finding {
                None => true,
                Some(best) => f.final_score > best.final_score,
            };
            if replace {
                top_finding = Some(f.clone());
            }
        }
        topics.push(TopicBriefing {
            name: t.name,
            new_count: findings.len(),
            last_run_at: t.last_run_at,
            last_status: t.last_status,
            findings,
        });
    }

    Ok(BriefingReport {
        period: if weekly { "weekly" } else { "daily" }.to_string(),
        since,
        generated_at: Utc::now().to_rfc3339(),
        total_topics: topics.len(),
        total_new,
        top_finding,
        topics,
    })
}

pub fn format_markdown(report: &BriefingReport) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "# radar30 {} briefing (since {})\n",
        report.period, report.since
    );
    let _ = writeln!(
        out,
        "- **Watched topics:** {}\n- **New findings:** {}\n",
        report.total_topics, report.total_new
    );

    if let Some(ref top) = report.top_finding {
        let _ = writeln!(
            out,
            "## Top Signal\n\n- **[{}]** [{}]({}) (topic: {}, score {:.1})\n",
            top.source, top.title, top.url, top.topic, top.final_score
        );
    }

    if report.topics.is_empty() {
        let _ = writeln!(
            out,
            "_No enabled watchlist topics. Add one with `radar30 watch add \"<topic>\"`._"
        );
        return out;
    }

    for t in &report.topics {
        let _ = writeln!(out, "## {} ({} new)\n", t.name, t.new_count);
        if t.findings.is_empty() {
            let _ = writeln!(out, "_No new findings since {}._\n", report.since);
            continue;
        }
        for f in t.findings.iter().take(8) {
            let _ = writeln!(
                out,
                "- **[{}]** [{}]({}) (score {:.1}, seen {}x)",
                f.source, f.title, f.url, f.final_score, f.sighting_count
            );
            if !f.snippet.trim().is_empty() {
                let _ = writeln!(
                    out,
                    "  - {}",
                    f.snippet.chars().take(180).collect::<String>()
                );
            }
        }
        let _ = writeln!(out);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{
        Candidate, DateWindow, Depth, Engagement, Intent, RESEARCH_SCHEMA, SourceId,
    };
    use chrono::NaiveDate;
    use std::collections::BTreeMap;

    #[test]
    fn generates_daily_briefing_from_stored_findings() {
        let tmp = tempfile::tempdir().unwrap();
        let mut store = Store::open(tmp.path().join("research.db")).unwrap();
        let report = crate::schema::ResearchReport {
            schema: RESEARCH_SCHEMA.to_string(),
            version: "0.2.0".to_string(),
            topic: "Rust".to_string(),
            primary_entity: "Rust".to_string(),
            intent: Intent::BreakingNews,
            depth: Depth::Quick,
            window: DateWindow {
                from: NaiveDate::from_ymd_opt(2026, 10, 20).unwrap(),
                to: NaiveDate::from_ymd_opt(2026, 11, 19).unwrap(),
            },
            generated_at: "2026-11-19T12:00:00Z".to_string(),
            total_fetched: 1,
            candidates: vec![Candidate {
                candidate_id: "cand-1".to_string(),
                canonical_key: "https://example.com/1".to_string(),
                primary_source: SourceId::HackerNews,
                sources: vec![SourceId::HackerNews],
                title: "Rust 1.100".to_string(),
                url: "https://example.com/1".to_string(),
                snippet: "Release notes".to_string(),
                published_at: Some("2026-11-19".to_string()),
                out_of_window: false,
                subquery_labels: vec!["primary".to_string()],
                native_ranks: BTreeMap::new(),
                local_relevance: 0.9,
                entity_grounded: true,
                freshness: 100,
                engagement: Engagement {
                    score: Some(400),
                    ..Default::default()
                },
                engagement_score: 0.85,
                source_quality: 0.8,
                rrf_score: 0.015,
                final_score: 91.0,
                cluster_id: None,
                metadata: BTreeMap::new(),
            }],
            clusters: vec![],
            source_outcomes: vec![],
            warnings: vec![],
        };
        store.record_report(&report).unwrap();

        let brief = generate(&store, false, Some("2020-01-01")).unwrap();
        assert_eq!(brief.total_topics, 1);
        assert_eq!(brief.total_new, 1);
        assert!(brief.top_finding.is_some());
        let md = format_markdown(&brief);
        assert!(md.contains("Rust 1.100"));
    }
}
