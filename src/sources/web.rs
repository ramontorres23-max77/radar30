//! Web search lane via Brave Search API (`api.search.brave.com/res/v1/web/search`, BYOK).

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde::Deserialize;
use url::Url;

use crate::http::{self, HttpError};
use crate::schema::{DateConfidence, DateWindow, Depth, Engagement, SourceId, SourceItem};

const BRAVE_URL: &str = "https://api.search.brave.com/res/v1/web/search";

#[derive(Deserialize)]
struct BraveResponse {
    web: Option<WebResults>,
}

#[derive(Deserialize)]
struct WebResults {
    #[serde(default)]
    results: Vec<BraveResult>,
}

#[derive(Deserialize)]
struct BraveResult {
    #[serde(default)]
    title: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    description: String,
    page_age: Option<String>,
}

pub fn fetch(
    query: &str,
    window: DateWindow,
    depth: Depth,
    timeout: Duration,
    deadline: Instant,
    token: &str,
) -> Result<(Vec<SourceItem>, Option<String>), HttpError> {
    let count = depth.source_fetch_limit().min(20);
    let mut url = Url::parse(BRAVE_URL).map_err(|e| HttpError::Decode {
        detail: e.to_string(),
    })?;
    url.query_pairs_mut()
        .append_pair("q", query)
        .append_pair("count", &count.to_string())
        .append_pair(
            "freshness",
            &format!(
                "{}to{}",
                window.from.format("%Y-%m-%d"),
                window.to.format("%Y-%m-%d")
            ),
        );

    let headers = [("X-Subscription-Token", token)];
    let parsed: BraveResponse = http::get_json(url.as_str(), &headers, timeout, deadline, 1)?;
    let results = parsed.web.map(|w| w.results).unwrap_or_default();

    let mut out = Vec::new();
    for (idx, r) in results.into_iter().enumerate() {
        if r.title.trim().is_empty() || r.url.trim().is_empty() {
            continue;
        }
        let published_at = r
            .page_age
            .as_deref()
            .and_then(|s| s.get(..10))
            .filter(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").is_ok())
            .map(ToOwned::to_owned);
        out.push(SourceItem {
            item_id: format!("web-{}", idx + 1),
            source: SourceId::Web,
            title: r.title,
            body: r.description.clone(),
            url: r.url,
            author: None,
            container: None,
            published_at,
            date_confidence: DateConfidence::Med,
            engagement: Engagement::default(),
            snippet: r.description.chars().take(240).collect(),
            metadata: BTreeMap::new(),
            local_relevance: None,
            entity_grounded: None,
            freshness: None,
            engagement_score: None,
            source_quality: None,
            local_rank_score: None,
        });
    }
    Ok((out, Some("brave/web".to_string())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserializes_brave_web_results() {
        let raw = serde_json::json!({
            "web": {"results": [{
                "title": "Rust 1.100 released",
                "url": "https://blog.rust-lang.org/1.100",
                "description": "Faster incremental builds.",
                "page_age": "2026-11-12T00:00:00"
            }]}
        });
        let parsed: BraveResponse = serde_json::from_value(raw).unwrap();
        let results = parsed.web.unwrap().results;
        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0].page_age.as_deref().unwrap()[..10].to_string(),
            "2026-11-12"
        );
    }
}
