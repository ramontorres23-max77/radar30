//! X / Twitter adapter via the official X API v2 recent search
//! (`api.x.com/2/tweets/search/recent`) with an app-only `X_BEARER_TOKEN`.
//!
//! Recent search reaches back ~7 days; a wider requested window is truncated
//! and reported in the stream detail (matching `last30days` bearer coverage).

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant};

use chrono::{NaiveTime, Utc};
use serde::Deserialize;
use serde_json::json;
use url::Url;

use crate::http::{self, HttpError};
use crate::schema::{DateConfidence, DateWindow, Depth, Engagement, SourceId, SourceItem};

const RECENT_URL: &str = "https://api.x.com/2/tweets/search/recent";
const RECENT_WINDOW_DAYS: i64 = 7;

#[derive(Deserialize)]
struct XResponse {
    #[serde(default)]
    data: Vec<XTweet>,
    #[serde(default)]
    includes: Option<XIncludes>,
}

#[derive(Deserialize)]
struct XIncludes {
    #[serde(default)]
    users: Vec<XUser>,
}

#[derive(Deserialize)]
struct XUser {
    id: String,
    username: String,
}

#[derive(Deserialize)]
struct XTweet {
    id: String,
    #[serde(default)]
    text: String,
    author_id: Option<String>,
    created_at: Option<String>,
    public_metrics: Option<XMetrics>,
}

#[derive(Deserialize)]
struct XMetrics {
    like_count: Option<i64>,
    retweet_count: Option<i64>,
    reply_count: Option<i64>,
}

pub fn fetch(
    query: &str,
    window: DateWindow,
    depth: Depth,
    timeout: Duration,
    deadline: Instant,
    token: &str,
) -> Result<(Vec<SourceItem>, Option<String>), HttpError> {
    let max_results = match depth {
        Depth::Quick => 10,
        Depth::Default => 30,
        Depth::Deep => 60,
    }
    .clamp(10, 100);

    // Recent search only covers the last 7 days; clamp and report truncation.
    let now = Utc::now();
    let earliest = now - chrono::Duration::days(RECENT_WINDOW_DAYS);
    let requested_start = window.from.and_time(NaiveTime::MIN).and_utc();
    let start = requested_start.max(earliest);
    let end = (window.to.and_time(NaiveTime::MIN).and_utc() + chrono::Duration::days(1))
        .min(now - chrono::Duration::seconds(30));
    let truncated = requested_start < earliest;

    let clean_query: String = query.chars().take(480).collect();
    let mut url = Url::parse(RECENT_URL).map_err(|e| HttpError::Decode {
        detail: e.to_string(),
    })?;
    url.query_pairs_mut()
        .append_pair("query", &clean_query)
        .append_pair("max_results", &max_results.to_string())
        .append_pair(
            "start_time",
            &start.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
        )
        .append_pair("end_time", &end.format("%Y-%m-%dT%H:%M:%SZ").to_string())
        .append_pair("tweet.fields", "created_at,public_metrics,author_id")
        .append_pair("expansions", "author_id");

    let auth = format!("Bearer {token}");
    let headers = [("Authorization", auth.as_str())];
    let parsed: XResponse = http::get_json(url.as_str(), &headers, timeout, deadline, 1)?;

    let users: HashMap<String, String> = parsed
        .includes
        .map(|i| i.users.into_iter().map(|u| (u.id, u.username)).collect())
        .unwrap_or_default();

    let items = normalize_tweets(parsed.data, &users);
    let detail = if truncated {
        "xapi/recent (window truncated to 7 days)".to_string()
    } else {
        "xapi/recent".to_string()
    };
    Ok((items, Some(detail)))
}

fn normalize_tweets(tweets: Vec<XTweet>, users: &HashMap<String, String>) -> Vec<SourceItem> {
    let mut out = Vec::new();
    for t in tweets {
        let text = t.text.trim().to_string();
        if text.is_empty() {
            continue;
        }
        let username = t
            .author_id
            .as_ref()
            .and_then(|id| users.get(id))
            .cloned()
            .unwrap_or_else(|| "i".to_string());
        let url = format!("https://x.com/{username}/status/{}", t.id);
        let title: String = text.chars().take(110).collect();
        let published_at = t
            .created_at
            .as_deref()
            .and_then(|s| s.get(..10))
            .map(ToOwned::to_owned);
        let metrics = t.public_metrics.unwrap_or(XMetrics {
            like_count: None,
            retweet_count: None,
            reply_count: None,
        });

        let mut metadata = BTreeMap::new();
        metadata.insert("handle".to_string(), json!(username));

        out.push(SourceItem {
            item_id: t.id,
            source: SourceId::X,
            title,
            body: text.clone(),
            url,
            author: Some(format!("@{username}")),
            container: Some("x.com".to_string()),
            published_at,
            date_confidence: DateConfidence::High,
            engagement: Engagement {
                likes: metrics.like_count,
                score: metrics.retweet_count,
                comments: metrics.reply_count,
                ..Default::default()
            },
            snippet: text.chars().take(240).collect(),
            metadata,
            local_relevance: None,
            entity_grounded: None,
            freshness: None,
            engagement_score: None,
            source_quality: None,
            local_rank_score: None,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_tweets_with_author_expansion() {
        let mut users = HashMap::new();
        users.insert("42".to_string(), "rustlang".to_string());
        let tweets = vec![XTweet {
            id: "1990001".into(),
            text: "Rust 1.100 is out with faster incremental builds".into(),
            author_id: Some("42".into()),
            created_at: Some("2026-11-18T12:00:00Z".into()),
            public_metrics: Some(XMetrics {
                like_count: Some(1500),
                retweet_count: Some(210),
                reply_count: Some(88),
            }),
        }];
        let items = normalize_tweets(tweets, &users);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].url, "https://x.com/rustlang/status/1990001");
        assert_eq!(items[0].engagement.likes, Some(1500));
        assert_eq!(items[0].author.as_deref(), Some("@rustlang"));
    }
}
