//! Wave 2 offline integration tests: SQLite store, watchlist batch run, briefing,
//! library search + Atom feed, and MCP Wave 2 tools.

use std::collections::BTreeMap;
use std::time::Duration;

use radar30::schema::{DateConfidence, Depth, Engagement, SourceId, SourceItem};
use radar30::sources::FixtureFetcher;
use radar30::store::Store;
use radar30::{briefing, library, mcp, watchlist};
use serde_json::json;

#[test]
fn wave2_watchlist_store_briefing_library_and_mcp_work_end_to_end() {
    let tmp = tempfile::tempdir().unwrap();
    let fixtures_dir = tmp.path().join("fixtures");
    let briefs_dir = tmp.path().join("briefs");
    let site_dir = tmp.path().join("site");
    let db_path = tmp.path().join("research.db");
    std::fs::create_dir_all(&fixtures_dir).unwrap();

    let items = vec![SourceItem {
        item_id: "hn-100".to_string(),
        source: SourceId::HackerNews,
        title: "Rust 1.100 ships faster incremental builds".to_string(),
        body: "Detailed release benchmarks for Rust 1.100 incremental compilation.".to_string(),
        url: "https://blog.rust-lang.org/2026/11/12/Rust-1.100.0/".to_string(),
        author: Some("rustacean".to_string()),
        container: Some("news.ycombinator.com".to_string()),
        published_at: Some("2026-11-18".to_string()),
        date_confidence: DateConfidence::High,
        engagement: Engagement {
            score: Some(610),
            comments: Some(240),
            ..Default::default()
        },
        snippet: "Incremental compilation speedups in Rust 1.100.".to_string(),
        metadata: BTreeMap::new(),
        local_relevance: None,
        entity_grounded: None,
        freshness: None,
        engagement_score: None,
        source_quality: None,
        local_rank_score: None,
    }];
    std::fs::write(
        fixtures_dir.join("hackernews.json"),
        serde_json::to_string_pretty(&items).unwrap(),
    )
    .unwrap();

    // 1. Add watchlist topic and run batch into SQLite + briefs dir.
    let mut store = Store::open(&db_path).unwrap();
    store
        .ensure_topic("Rust 1.100", Some("0 8 * * *"), Some("hackernews"))
        .unwrap();

    let fetcher = FixtureFetcher::new(&fixtures_dir);
    let run1 = watchlist::run_watchlist(
        &mut store,
        None,
        Depth::Quick,
        30,
        Duration::from_secs(10),
        Some(&briefs_dir),
        &fetcher,
    )
    .unwrap();
    assert_eq!(run1.ran, 1);
    assert_eq!(run1.total_new, 1);
    assert_eq!(run1.total_updated, 0);

    // Second run deduplicates URL and increments sighting_count.
    let run2 = watchlist::run_watchlist(
        &mut store,
        None,
        Depth::Quick,
        30,
        Duration::from_secs(10),
        Some(&briefs_dir),
        &fetcher,
    )
    .unwrap();
    assert_eq!(run2.total_new, 0);
    assert_eq!(run2.total_updated, 1);

    // 2. Daily briefing over stored findings.
    let brief = briefing::generate(&store, false, Some("2020-01-01")).unwrap();
    assert_eq!(brief.total_topics, 1);
    assert_eq!(brief.total_new, 1);
    assert_eq!(brief.top_finding.as_ref().unwrap().sighting_count, 2);

    // 3. Offline library search merges brief + SQLite FTS5 hits, and feed builds Atom XML.
    let search = library::search("incremental", &briefs_dir, Some(&db_path), 10).unwrap();
    assert!(search.hits.iter().any(|h| h.kind == "brief"));
    assert!(search.hits.iter().any(|h| h.kind == "finding"));

    let feed = library::build_feed(&briefs_dir, &site_dir, "https://example.org/radar30").unwrap();
    assert_eq!(feed.briefs_count, 1);
    assert!(feed.feed_xml.is_file());

    // 4. Verify Wave 2 MCP tools (`radar30_library_search` & `radar30_briefing`) via JSON-RPC.
    let mcp_search = mcp::handle(&json!({
        "jsonrpc": "2.0",
        "id": 10,
        "method": "tools/call",
        "params": {
            "name": "radar30_library_search",
            "arguments": {
                "query": "incremental",
                "briefs_dir": briefs_dir.to_string_lossy(),
                "db": db_path.to_string_lossy()
            }
        }
    }))
    .unwrap();
    assert_eq!(mcp_search["result"]["isError"], false);
    assert!(
        !mcp_search["result"]["structuredContent"]["hits"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    let mcp_brief = mcp::handle(&json!({
        "jsonrpc": "2.0",
        "id": 11,
        "method": "tools/call",
        "params": {
            "name": "radar30_briefing",
            "arguments": {
                "since": "2020-01-01",
                "db": db_path.to_string_lossy()
            }
        }
    }))
    .unwrap();
    assert_eq!(mcp_brief["result"]["isError"], false);
    assert_eq!(mcp_brief["result"]["structuredContent"]["total_new"], 1);
}
