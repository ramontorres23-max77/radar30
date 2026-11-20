//! Hacker News keyless adapter via Algolia HN Search API (`hn.algolia.com/api/v1/search`).

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use chrono::{DateTime, NaiveTime};
use serde::Deserialize;
use serde_json::json;
use url::Url;

use crate::http::{self, HttpError};
use crate::schema::{DateConfidence, DateWindow, Depth, Engagement, SourceId, SourceItem};

const ALGOLIA_SEARCH_URL: &str = "https://hn.algolia.com/api/v1/search";
const MIN_STORY_POINTS: i64 = 2;

#[derive(Deserialize)]
struct AlgoliaResponse {
    #[serde(default)]
    hits: Vec<AlgoliaHit>,
}

#[derive(Deserialize)]
struct AlgoliaHit {
    #[serde(rename = "objectID", default)]
    object_id: String,
    title: Option<String>,
    url: Option<String>,
    author: Option<String>,
    points: Option<i64>,
    num_comments: Option<i64>,
    created_at_i: Option<i64>,
    story_text: Option<String>,
}

pub fn fetch(
    query: &str,
    window: DateWindow,
    depth: Depth,
    timeout: Duration,
    deadline: Instant,
) -> Result<(Vec<SourceItem>, Option<String>), HttpError> {
    let from_ts = window.from.and_time(NaiveTime::MIN).and_utc().timestamp();
    let to_ts = window.to.and_time(NaiveTime::MIN).and_utc().timestamp() + 86_400;

    // Flatten hyphens/commas so Algolia tokenizes themed queries cleanly.
    let flat_query: String = query
        .chars()
        .map(|c| if c == '-' || c == ',' { ' ' } else { c })
        .collect();
    let flat_query = flat_query.split_whitespace().collect::<Vec<_>>().join(" ");

    // Overfetch 2x because Algolia forbids `points>N` in numericFilters (HTTP 400);
    // low-point stories are filtered client-side instead.
    let target = depth.source_fetch_limit();
    let fetch_count = (target * 2).min(100);

    let mut url = Url::parse(ALGOLIA_SEARCH_URL).map_err(|e| HttpError::Decode {
        detail: e.to_string(),
    })?;
    url.query_pairs_mut()
        .append_pair("query", &flat_query)
        .append_pair("tags", "story")
        .append_pair(
            "numericFilters",
            &format!("created_at_i>{from_ts},created_at_i<{to_ts}"),
        )
        .append_pair("hitsPerPage", &fetch_count.to_string());

    let parsed: AlgoliaResponse = http::get_json(url.as_str(), &[], timeout, deadline, 2)?;
    let items = normalize_hits(parsed.hits, target);
    Ok((items, Some("algolia/v1".to_string())))
}

fn normalize_hits(hits: Vec<AlgoliaHit>, limit: usize) -> Vec<SourceItem> {
    let mut out = Vec::new();
    for hit in hits {
        let Some(title) = hit.title.filter(|t| !t.trim().is_empty()) else {
            continue;
        };
        let points = hit.points.unwrap_or(0);
        if points < MIN_STORY_POINTS {
            continue;
        }
        let hn_url = format!("https://news.ycombinator.com/item?id={}", hit.object_id);
        let item_url = hit
            .url
            .filter(|u| !u.trim().is_empty())
            .unwrap_or_else(|| hn_url.clone());
        let published_at = hit
            .created_at_i
            .and_then(|ts| DateTime::from_timestamp(ts, 0))
            .map(|dt| dt.format("%Y-%m-%d").to_string());
        let body = hit.story_text.map(|s| strip_html(&s)).unwrap_or_default();
        let snippet: String = body.chars().take(240).collect();

        let mut metadata = BTreeMap::new();
        metadata.insert("hn_url".to_string(), json!(hn_url));

        out.push(SourceItem {
            item_id: hit.object_id,
            source: SourceId::HackerNews,
            title,
            body,
            url: item_url,
            author: hit.author,
            container: Some("news.ycombinator.com".to_string()),
            published_at,
            date_confidence: DateConfidence::High,
            engagement: Engagement {
                score: Some(points),
                comments: hit.num_comments,
                ..Default::default()
            },
            snippet,
            metadata,
            local_relevance: None,
            entity_grounded: None,
            freshness: None,
            engagement_score: None,
            source_quality: None,
            local_rank_score: None,
        });
        if out.len() >= limit {
            break;
        }
    }
    out
}

fn strip_html(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut in_tag = false;
    for ch in input.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                out.push(' ');
            }
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out.replace("&quot;", "\"")
        .replace("&#x27;", "'")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_low_points_and_normalizes_hits() {
        let hits = vec![
            AlgoliaHit {
                object_id: "1".into(),
                title: Some("Low point spam".into()),
                url: None,
                author: Some("a".into()),
                points: Some(1),
                num_comments: Some(0),
                created_at_i: Some(1_763_424_000),
                story_text: None,
            },
            AlgoliaHit {
                object_id: "2".into(),
                title: Some("Rust 1.100 Released".into()),
                url: Some("https://blog.rust-lang.org/1.100".into()),
                author: Some("rustacean".into()),
                points: Some(420),
                num_comments: Some(180),
                created_at_i: Some(1_763_424_000),
                story_text: Some("<p>Fast <b>builds</b>!</p>".into()),
            },
        ];
        let items = normalize_hits(hits, 10);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].engagement.score, Some(420));
        assert_eq!(items[0].body, "Fast builds !");
    }
}
