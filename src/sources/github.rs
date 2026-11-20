//! GitHub keyless adapter via public REST Search API (`api.github.com/search/issues`).
//!
//! Uses optional `GITHUB_TOKEN` when present in the environment for higher rate limits,
//! and stays strictly within conservative anonymous caps when unauthenticated.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::json;
use url::Url;

use crate::http::{self, HttpError};
use crate::schema::{DateConfidence, DateWindow, Depth, Engagement, SourceId, SourceItem};

const ISSUES_SEARCH_URL: &str = "https://api.github.com/search/issues";
const UNAUTH_CAP: usize = 10;

#[derive(Deserialize)]
struct GhIssuesResponse {
    #[serde(default)]
    items: Vec<GhIssue>,
}

#[derive(Deserialize)]
struct GhIssue {
    id: u64,
    #[serde(default)]
    title: String,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    body: Option<String>,
    user: Option<GhUser>,
    comments: Option<i64>,
    reactions: Option<GhReactions>,
    created_at: Option<String>,
    repository_url: Option<String>,
    #[serde(default)]
    state: String,
}

#[derive(Deserialize)]
struct GhUser {
    login: String,
}

#[derive(Deserialize)]
struct GhReactions {
    total_count: Option<i64>,
}

pub fn fetch(
    query: &str,
    window: DateWindow,
    depth: Depth,
    timeout: Duration,
    deadline: Instant,
) -> Result<(Vec<SourceItem>, Option<String>), HttpError> {
    let token = crate::creds::resolve("GITHUB_TOKEN");
    let limit = match token {
        Some(_) => depth.source_fetch_limit().min(30),
        None => depth.source_fetch_limit().min(UNAUTH_CAP),
    };

    let q = format!(
        "{query} created:{}..{}",
        window.from.format("%Y-%m-%d"),
        window.to.format("%Y-%m-%d")
    );
    let mut url = Url::parse(ISSUES_SEARCH_URL).map_err(|e| HttpError::Decode {
        detail: e.to_string(),
    })?;
    url.query_pairs_mut()
        .append_pair("q", &q)
        .append_pair("sort", "reactions-+1")
        .append_pair("order", "desc")
        .append_pair("per_page", &limit.to_string());

    let auth_header = token.as_ref().map(|t| format!("Bearer {t}"));
    let mut headers: Vec<(&str, &str)> = vec![("Accept", "application/vnd.github+json")];
    if let Some(ref auth) = auth_header {
        headers.push(("Authorization", auth.as_str()));
    }

    let parsed: GhIssuesResponse = http::get_json(url.as_str(), &headers, timeout, deadline, 1)?;
    let detail = if token.is_some() {
        "rest-search (authenticated)".to_string()
    } else {
        "rest-search (anonymous)".to_string()
    };
    Ok((normalize_issues(parsed.items), Some(detail)))
}

fn normalize_issues(issues: Vec<GhIssue>) -> Vec<SourceItem> {
    let mut out = Vec::new();
    for issue in issues {
        if issue.title.trim().is_empty() || issue.html_url.trim().is_empty() {
            continue;
        }
        let repo = issue
            .repository_url
            .as_deref()
            .and_then(|u| u.strip_prefix("https://api.github.com/repos/"))
            .map(ToOwned::to_owned);
        let published_at = issue
            .created_at
            .as_deref()
            .and_then(|s| s.get(..10))
            .map(ToOwned::to_owned);
        let body = issue.body.unwrap_or_default();
        let snippet: String = body
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(240)
            .collect();

        let mut metadata = BTreeMap::new();
        if !issue.state.is_empty() {
            metadata.insert("state".to_string(), json!(issue.state));
        }

        out.push(SourceItem {
            item_id: issue.id.to_string(),
            source: SourceId::GitHub,
            title: issue.title,
            body,
            url: issue.html_url,
            author: issue.user.map(|u| u.login),
            container: repo,
            published_at,
            date_confidence: DateConfidence::High,
            engagement: Engagement {
                score: issue.reactions.and_then(|r| r.total_count),
                comments: issue.comments,
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
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_github_issues() {
        let issues = vec![GhIssue {
            id: 99,
            title: "Support Rust 2024 in workspace".into(),
            html_url: "https://github.com/org/repo/issues/42".into(),
            body: Some("Tracking issue for Rust 2024".into()),
            user: Some(GhUser {
                login: "alice".into(),
            }),
            comments: Some(15),
            reactions: Some(GhReactions {
                total_count: Some(32),
            }),
            created_at: Some("2026-11-15T12:00:00Z".into()),
            repository_url: Some("https://api.github.com/repos/org/repo".into()),
            state: "open".into(),
        }];
        let items = normalize_issues(issues);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].container.as_deref(), Some("org/repo"));
        assert_eq!(items[0].published_at.as_deref(), Some("2026-11-15"));
        assert_eq!(items[0].engagement.score, Some(32));
    }
}
