//! DEV Community (dev.to / Forem) keyless adapter (`dev.to/api/articles`).

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::json;
use url::Url;

use crate::http::{self, HttpError};
use crate::schema::{DateConfidence, DateWindow, Depth, Engagement, SourceId, SourceItem};
use crate::signals::token_overlap_relevance;

const ARTICLES_URL: &str = "https://dev.to/api/articles";
const RELEVANCE_FLOOR: f64 = 0.25;

#[derive(Deserialize)]
struct DevToArticle {
    id: u64,
    #[serde(default)]
    title: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    url: String,
    user: Option<DevToUser>,
    published_at: Option<String>,
    positive_reactions_count: Option<i64>,
    comments_count: Option<i64>,
    #[serde(default)]
    tag_list: Vec<String>,
}

#[derive(Deserialize)]
struct DevToUser {
    username: String,
}

pub fn fetch(
    query: &str,
    window: DateWindow,
    depth: Depth,
    timeout: Duration,
    deadline: Instant,
) -> Result<(Vec<SourceItem>, Option<String>), HttpError> {
    let per_page = depth.source_fetch_limit().min(30);
    let top_days = window.days().clamp(1, 365);

    // Derive a clean alphanumeric tag from the head token for the Forem top-articles endpoint.
    let tag: String = crate::query::extract_head_token(query)
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();

    let mut url = Url::parse(ARTICLES_URL).map_err(|e| HttpError::Decode {
        detail: e.to_string(),
    })?;
    {
        let mut pairs = url.query_pairs_mut();
        if !tag.is_empty() {
            pairs.append_pair("tag", &tag);
        }
        pairs.append_pair("top", &top_days.to_string());
        pairs.append_pair("per_page", &per_page.to_string());
    }

    let headers = [("Accept", "application/vnd.forem.api-v1+json")];
    let parsed: Vec<DevToArticle> = http::get_json(url.as_str(), &headers, timeout, deadline, 1)?;
    let items = normalize_articles(parsed, query);
    Ok((items, Some(format!("forem/tag:{tag}"))))
}

fn normalize_articles(articles: Vec<DevToArticle>, query: &str) -> Vec<SourceItem> {
    let mut out = Vec::new();
    for a in articles {
        if a.title.trim().is_empty() || a.url.trim().is_empty() {
            continue;
        }
        let combined = format!("{} {} {}", a.title, a.description, a.tag_list.join(" "));
        if token_overlap_relevance(query, &combined) < RELEVANCE_FLOOR {
            continue;
        }
        let published_at = a
            .published_at
            .as_deref()
            .and_then(|s| s.get(..10))
            .map(ToOwned::to_owned);
        let mut metadata = BTreeMap::new();
        if !a.tag_list.is_empty() {
            metadata.insert("tags".to_string(), json!(a.tag_list));
        }

        out.push(SourceItem {
            item_id: a.id.to_string(),
            source: SourceId::DevTo,
            title: a.title,
            body: a.description.clone(),
            url: a.url,
            author: a.user.map(|u| u.username),
            container: Some("dev.to".to_string()),
            published_at,
            date_confidence: DateConfidence::High,
            engagement: Engagement {
                score: a.positive_reactions_count,
                comments: a.comments_count,
                ..Default::default()
            },
            snippet: a.description.chars().take(240).collect(),
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
    fn filters_irrelevant_devto_articles() {
        let articles = vec![
            DevToArticle {
                id: 1,
                title: "Building fast CLIs in Rust".into(),
                description: "How Rust and clap power developer tools".into(),
                url: "https://dev.to/a/rust-cli".into(),
                user: Some(DevToUser {
                    username: "alice".into(),
                }),
                published_at: Some("2026-11-12T08:00:00Z".into()),
                positive_reactions_count: Some(85),
                comments_count: Some(12),
                tag_list: vec!["rust".into(), "cli".into()],
            },
            DevToArticle {
                id: 2,
                title: "CSS Grid tricks".into(),
                description: "Responsive layouts".into(),
                url: "https://dev.to/b/css".into(),
                user: None,
                published_at: Some("2026-11-12T08:00:00Z".into()),
                positive_reactions_count: Some(500),
                comments_count: Some(40),
                tag_list: vec!["css".into()],
            },
        ];
        let items = normalize_articles(articles, "Rust CLI");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].item_id, "1");
    }
}
