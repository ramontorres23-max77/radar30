//! Bluesky adapter: authenticated AppView (`api.bsky.app`) when an app password is
//! configured, otherwise the unauthenticated public mirror (`public.api.bsky.app`).
//! The public mirror is frequently refused (HTTP 403); an app password is recommended.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::json;
use url::Url;

use crate::http::{self, HttpError};
use crate::schema::{DateConfidence, DateWindow, Depth, Engagement, SourceId, SourceItem};

const SEARCH_URL: &str = "https://public.api.bsky.app/xrpc/app.bsky.feed.searchPosts";
const AUTH_SEARCH_URL: &str = "https://api.bsky.app/xrpc/app.bsky.feed.searchPosts";
const SESSION_URL: &str = "https://bsky.social/xrpc/com.atproto.server.createSession";

#[derive(Deserialize)]
struct SearchResponse {
    #[serde(default)]
    posts: Vec<BskyPost>,
}

#[derive(Deserialize)]
struct BskyPost {
    #[serde(default)]
    uri: String,
    author: Option<BskyAuthor>,
    record: Option<BskyRecord>,
    #[serde(rename = "likeCount", default)]
    like_count: Option<i64>,
    #[serde(rename = "repostCount", default)]
    repost_count: Option<i64>,
    #[serde(rename = "replyCount", default)]
    reply_count: Option<i64>,
}

#[derive(Deserialize)]
struct BskyAuthor {
    #[serde(default)]
    handle: String,
    #[serde(rename = "displayName", default)]
    display_name: Option<String>,
}

#[derive(Deserialize)]
struct BskyRecord {
    #[serde(default)]
    text: String,
    #[serde(rename = "createdAt", default)]
    created_at: Option<String>,
}

pub fn fetch(
    query: &str,
    _window: DateWindow,
    depth: Depth,
    timeout: Duration,
    deadline: Instant,
) -> Result<(Vec<SourceItem>, Option<String>), HttpError> {
    let limit = depth.source_fetch_limit().min(50);

    // Preferred: authenticated canonical AppView with an app password.
    let handle = crate::creds::resolve("BLUESKY_HANDLE");
    let password = crate::creds::resolve("BLUESKY_APP_PASSWORD");
    if let (Some(handle), Some(password)) = (handle, password)
        && let Ok(session) = create_session(&handle, &password, timeout, deadline)
        && let Ok(parsed) = search(
            AUTH_SEARCH_URL,
            query,
            limit,
            Some(&session),
            timeout,
            deadline,
        )
    {
        return Ok((
            normalize_posts(parsed.posts),
            Some("atproto/authenticated".to_string()),
        ));
    }

    // Fallback: public mirror (may be blocked for some networks).
    let parsed = match search(SEARCH_URL, query, limit, None, timeout, deadline) {
        Ok(p) => p,
        Err(HttpError::Forbidden { .. }) | Err(HttpError::RateLimited { .. }) => {
            return Err(HttpError::Forbidden {
                detail: "public mirror refused the request; configure BLUESKY_HANDLE + BLUESKY_APP_PASSWORD (`radar30 creds set ... --stdin`)".to_string(),
            });
        }
        Err(e) => return Err(e),
    };
    Ok((
        normalize_posts(parsed.posts),
        Some("atproto/public".to_string()),
    ))
}

#[derive(Deserialize)]
struct SessionResponse {
    #[serde(rename = "accessJwt", default)]
    access_jwt: String,
}

fn create_session(
    handle: &str,
    password: &str,
    timeout: Duration,
    deadline: Instant,
) -> Result<String, HttpError> {
    let body = json!({ "identifier": handle, "password": password });
    let session: SessionResponse = http::post_json(SESSION_URL, &[], &body, timeout, deadline, 1)?;
    if session.access_jwt.is_empty() {
        return Err(HttpError::Decode {
            detail: "Bluesky createSession returned no accessJwt".to_string(),
        });
    }
    Ok(session.access_jwt)
}

fn search(
    base: &str,
    query: &str,
    limit: usize,
    token: Option<&str>,
    timeout: Duration,
    deadline: Instant,
) -> Result<SearchResponse, HttpError> {
    let mut url = Url::parse(base).map_err(|e| HttpError::Decode {
        detail: e.to_string(),
    })?;
    url.query_pairs_mut()
        .append_pair("q", query)
        .append_pair("limit", &limit.to_string())
        .append_pair("sort", "top");
    let auth = token.map(|t| format!("Bearer {t}"));
    let mut headers: Vec<(&str, &str)> = Vec::new();
    if let Some(ref auth) = auth {
        headers.push(("Authorization", auth.as_str()));
    }
    http::get_json(url.as_str(), &headers, timeout, deadline, 1)
}

fn normalize_posts(posts: Vec<BskyPost>) -> Vec<SourceItem> {
    let mut out = Vec::new();
    for p in posts {
        let Some(record) = p.record else { continue };
        let text = record.text.trim().to_string();
        if text.is_empty() || p.uri.is_empty() {
            continue;
        }
        let handle = p
            .author
            .as_ref()
            .map(|a| a.handle.clone())
            .unwrap_or_default();
        let rkey = p.uri.rsplit('/').next().unwrap_or("");
        let url = if !handle.is_empty() && !rkey.is_empty() {
            format!("https://bsky.app/profile/{handle}/post/{rkey}")
        } else {
            continue;
        };
        let title: String = text.chars().take(110).collect();
        let published_at = record
            .created_at
            .as_deref()
            .and_then(|s| s.get(..10))
            .map(ToOwned::to_owned);
        let author = p
            .author
            .and_then(|a| a.display_name.filter(|s| !s.is_empty()).or(Some(a.handle)));

        out.push(SourceItem {
            item_id: rkey.to_string(),
            source: SourceId::Bluesky,
            title,
            body: text.clone(),
            url,
            author,
            container: Some("bsky.app".to_string()),
            published_at,
            date_confidence: DateConfidence::High,
            engagement: Engagement {
                likes: p.like_count,
                score: p.repost_count,
                comments: p.reply_count,
                ..Default::default()
            },
            snippet: text.chars().take(240).collect(),
            metadata: BTreeMap::new(),
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
    fn normalizes_bluesky_posts_to_web_urls() {
        let posts = vec![BskyPost {
            uri: "at://did:plc:abc/app.bsky.feed.post/3xyz123".into(),
            author: Some(BskyAuthor {
                handle: "alice.bsky.social".into(),
                display_name: Some("Alice".into()),
            }),
            record: Some(BskyRecord {
                text: "Rust 1.100 incremental builds are wild".into(),
                created_at: Some("2026-11-18T10:00:00Z".into()),
            }),
            like_count: Some(320),
            repost_count: Some(45),
            reply_count: Some(18),
        }];
        let items = normalize_posts(posts);
        assert_eq!(items.len(), 1);
        assert_eq!(
            items[0].url,
            "https://bsky.app/profile/alice.bsky.social/post/3xyz123"
        );
        assert_eq!(items[0].engagement.likes, Some(320));
        assert_eq!(items[0].published_at.as_deref(), Some("2026-11-18"));
    }
}
