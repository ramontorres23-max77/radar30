//! End-to-end offline integration tests for the radar30 pipeline and CLI contract.

use std::collections::BTreeMap;
use std::time::Duration;

use chrono::NaiveDate;
use radar30::pipeline::{self, RunOptions};
use radar30::render::{self, CANONICAL_OUTPUT_END};
use radar30::schema::{
    DateConfidence, DateWindow, Depth, EmitFormat, Engagement, RESEARCH_SCHEMA, SourceId,
    SourceItem, SourceState,
};
use radar30::sources::FixtureFetcher;

fn write_fixture(dir: &std::path::Path, source: SourceId, items: &[SourceItem]) {
    let path = dir.join(format!("{}.json", source.as_str()));
    std::fs::write(path, serde_json::to_string_pretty(items).unwrap()).unwrap();
}

fn item(
    id: &str,
    source: SourceId,
    title: &str,
    url: &str,
    date: &str,
    score: i64,
    comments: i64,
) -> SourceItem {
    SourceItem {
        item_id: id.to_string(),
        source,
        title: title.to_string(),
        body: format!("Detailed discussion about {title}"),
        url: url.to_string(),
        author: Some("tester".to_string()),
        container: None,
        published_at: Some(date.to_string()),
        date_confidence: DateConfidence::High,
        engagement: Engagement {
            score: Some(score),
            comments: Some(comments),
            ..Default::default()
        },
        snippet: format!("Summary: {title}"),
        metadata: BTreeMap::new(),
        local_relevance: None,
        entity_grounded: None,
        freshness: None,
        engagement_score: None,
        source_quality: None,
        local_rank_score: None,
    }
}

#[test]
fn offline_pipeline_fuses_corroborated_story_and_renders_all_formats() {
    let tmp = tempfile::tempdir().unwrap();
    write_fixture(
        tmp.path(),
        SourceId::HackerNews,
        &[
            item(
                "hn-1",
                SourceId::HackerNews,
                "Rust 1.100 ships faster incremental linker",
                "https://blog.rust-lang.org/2026/11/rust-1-100?utm_source=hn",
                "2026-11-18",
                520,
                210,
            ),
            item(
                "hn-2",
                SourceId::HackerNews,
                "Completely unrelated coffee recipe",
                "https://coffee.example/brew",
                "2026-11-18",
                900,
                300,
            ),
        ],
    );
    write_fixture(
        tmp.path(),
        SourceId::Reddit,
        &[
            item(
                "r-1",
                SourceId::Reddit,
                "Rust 1.100 ships faster incremental linker",
                "https://www.blog.rust-lang.org/2026/11/rust-1-100/",
                "2026-11-18",
                340,
                95,
            ),
            item(
                "r-2",
                SourceId::Reddit,
                "Old Rust 2023 retrospective",
                "https://www.reddit.com/r/rust/comments/abc123/old/",
                "2023-05-01",
                800,
                120,
            ),
        ],
    );

    let opts = RunOptions {
        topic: "Rust 1.100 release".to_string(),
        depth: Depth::Default,
        window: DateWindow {
            from: NaiveDate::from_ymd_opt(2026, 10, 20).unwrap(),
            to: NaiveDate::from_ymd_opt(2026, 11, 19).unwrap(),
        },
        sources: vec![SourceId::HackerNews, SourceId::Reddit],
        timeout: Duration::from_secs(10),
    };

    let fetcher = FixtureFetcher::new(tmp.path());
    let report = pipeline::run(&opts, &fetcher).unwrap();

    assert_eq!(report.schema, RESEARCH_SCHEMA);
    assert!(report.degraded_sources().is_empty());
    assert!(
        report
            .source_outcomes
            .iter()
            .all(|o| o.state == SourceState::Ok)
    );
    // Top candidate must be the corroborated in-window Rust 1.100 story (fused from HN + Reddit).
    assert_eq!(report.candidates[0].sources.len(), 2);
    assert!(report.candidates[0].title.contains("Rust 1.100"));
    assert!(!report.candidates[0].out_of_window);
    // Out-of-window 2023 post must sort strictly after in-window items.
    let old_pos = report
        .candidates
        .iter()
        .position(|c| c.title.contains("2023"))
        .unwrap();
    assert_eq!(old_pos, report.candidates.len() - 1);
    assert!(report.candidates[old_pos].out_of_window);

    // Verify all 3 renderers.
    let compact = render::render(&report, EmitFormat::Compact).unwrap();
    assert!(compact.contains(CANONICAL_OUTPUT_END));
    assert!(compact.contains("Rust 1.100"));

    let md = render::render(&report, EmitFormat::Md).unwrap();
    assert!(md.contains("# Research Brief: Rust 1.100 release"));
    assert!(md.contains("| Source | State | Items |"));

    let json_text = render::render(&report, EmitFormat::Json).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json_text).unwrap();
    assert_eq!(parsed["schema"], RESEARCH_SCHEMA);
}
