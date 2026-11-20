//! ScrapeCreators BYOK adapters for TikTok, Instagram Reels, Threads, LinkedIn,
//! and Pinterest. One shared key (`SCRAPECREATORS_API_KEY`), lenient parsing of
//! the provider's varying response envelopes.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde_json::Value;
use url::Url;

use crate::http::{self, HttpError};
use crate::schema::{DateConfidence, DateWindow, Depth, Engagement, SourceId, SourceItem};

const BASE: &str = "https://api.scrapecreators.com";

pub fn fetch(
    platform: SourceId,
    query: &str,
    _window: DateWindow,
    depth: Depth,
    timeout: Duration,
    deadline: Instant,
    token: &str,
) -> Result<(Vec<SourceItem>, Option<String>), HttpError> {
    let (path, extra): (&str, Vec<(&str, &str)>) = match platform {
        SourceId::TikTok => ("/v1/tiktok/search/keyword", vec![("sort_by", "relevance")]),
        SourceId::Instagram => ("/v2/instagram/reels/search", vec![]),
        SourceId::Threads => ("/v1/threads/search", vec![]),
        SourceId::LinkedIn => (
            "/v1/linkedin/search/posts",
            vec![("date_posted", "past-month")],
        ),
        SourceId::Pinterest => ("/v1/pinterest/search", vec![]),
        _ => {
            return Err(HttpError::Decode {
                detail: format!("unsupported ScrapeCreators platform {platform}"),
            });
        }
    };

    let mut url = Url::parse(&format!("{BASE}{path}")).map_err(|e| HttpError::Decode {
        detail: e.to_string(),
    })?;
    {
        let mut pairs = url.query_pairs_mut();
        pairs.append_pair("query", query);
        for (k, v) in extra {
            pairs.append_pair(k, v);
        }
    }

    let headers = [("x-api-key", token)];
    let data: Value = http::get_json(url.as_str(), &headers, timeout, deadline, 2)?;
    let limit = depth.source_fetch_limit();
    let items = normalize(platform, &data, limit);
    Ok((items, Some(format!("scrapecreators{}", path))))
}

/// Pull the first array present under any of the provider's envelope keys.
fn envelope_array<'a>(data: &'a Value, keys: &[&str]) -> Vec<&'a Value> {
    for key in keys {
        if let Some(arr) = data.get(*key).and_then(Value::as_array) {
            return arr.iter().collect();
        }
    }
    Vec::new()
}

fn s(v: &Value, keys: &[&str]) -> Option<String> {
    for k in keys {
        if let Some(val) = v.get(*k) {
            if let Some(text) = val.as_str().filter(|t| !t.trim().is_empty()) {
                return Some(text.to_string());
            }
            if let Some(n) = val.as_i64() {
                return Some(n.to_string());
            }
        }
    }
    None
}

fn n(v: &Value, keys: &[&str]) -> Option<i64> {
    for k in keys {
        if let Some(val) = v.get(*k) {
            if let Some(i) = val.as_i64() {
                return Some(i);
            }
            if let Some(text) = val.as_str()
                && let Ok(i) = text.replace(',', "").parse::<i64>()
            {
                return Some(i);
            }
        }
    }
    None
}

fn date10(v: &Value, keys: &[&str]) -> Option<String> {
    for k in keys {
        if let Some(val) = v.get(*k) {
            if let Some(text) = val.as_str()
                && text.len() >= 10
            {
                let prefix = &text[..10];
                if chrono::NaiveDate::parse_from_str(prefix, "%Y-%m-%d").is_ok() {
                    return Some(prefix.to_string());
                }
            }
            if let Some(ts) = val.as_i64()
                && let Some(dt) = chrono::DateTime::from_timestamp(ts, 0)
            {
                return Some(dt.format("%Y-%m-%d").to_string());
            }
        }
    }
    None
}

fn normalize(platform: SourceId, data: &Value, limit: usize) -> Vec<SourceItem> {
    let mut out = Vec::new();
    match platform {
        SourceId::TikTok => {
            for entry in envelope_array(data, &["search_item_list", "data", "items"]) {
                let info = entry.get("aweme_info").unwrap_or(entry);
                let Some(id) = s(info, &["aweme_id", "id"]) else {
                    continue;
                };
                let desc = s(info, &["desc", "title"]).unwrap_or_default();
                if desc.is_empty() {
                    continue;
                }
                let stats = info.get("statistics").cloned().unwrap_or(Value::Null);
                let author = info
                    .get("author")
                    .and_then(|a| s(a, &["unique_id", "nickname"]))
                    .unwrap_or_default();
                let url = if author.is_empty() {
                    format!("https://www.tiktok.com/video/{id}")
                } else {
                    format!("https://www.tiktok.com/@{author}/video/{id}")
                };
                out.push(build(
                    platform,
                    id,
                    desc,
                    url,
                    (!author.is_empty()).then_some(author),
                    date10(info, &["create_time", "created_at"]),
                    Engagement {
                        views: n(&stats, &["play_count", "views"]),
                        likes: n(&stats, &["digg_count", "likes"]),
                        comments: n(&stats, &["comment_count", "comments"]),
                        score: n(&stats, &["share_count"]),
                        ..Default::default()
                    },
                ));
                if out.len() >= limit {
                    break;
                }
            }
        }
        SourceId::Instagram => {
            for entry in envelope_array(data, &["items", "data", "reels", "results"]) {
                let id = s(entry, &["id", "code", "pk"]).unwrap_or_default();
                let caption = entry
                    .get("caption")
                    .and_then(|c| c.get("text").and_then(Value::as_str))
                    .map(ToOwned::to_owned)
                    .or_else(|| s(entry, &["caption", "title", "text"]))
                    .unwrap_or_default();
                let code = s(entry, &["code"]).unwrap_or(id.clone());
                if code.is_empty() {
                    continue;
                }
                let url = s(entry, &["url", "link"])
                    .unwrap_or_else(|| format!("https://www.instagram.com/reel/{code}/"));
                let owner = entry
                    .get("owner")
                    .or_else(|| entry.get("user"))
                    .and_then(|u| s(u, &["username", "full_name"]));
                out.push(build(
                    platform,
                    id,
                    caption,
                    url,
                    owner,
                    date10(entry, &["taken_at", "created_at", "date"]),
                    Engagement {
                        views: n(entry, &["play_count", "view_count", "views"]),
                        likes: n(entry, &["like_count", "likes"]),
                        comments: n(entry, &["comment_count", "comments"]),
                        ..Default::default()
                    },
                ));
                if out.len() >= limit {
                    break;
                }
            }
        }
        SourceId::Threads => {
            for entry in envelope_array(
                data,
                &["items", "data", "threads", "posts", "search_results"],
            ) {
                let id = s(entry, &["id", "code", "pk"]).unwrap_or_default();
                let text = s(entry, &["text", "caption", "content"]).unwrap_or_default();
                if text.is_empty() {
                    continue;
                }
                let user = entry
                    .get("user")
                    .and_then(|u| s(u, &["username"]))
                    .unwrap_or_default();
                let code = s(entry, &["code"]).unwrap_or(id.clone());
                let url = s(entry, &["url", "link"]).unwrap_or_else(|| {
                    if user.is_empty() {
                        format!("https://www.threads.net/post/{code}")
                    } else {
                        format!("https://www.threads.net/@{user}/post/{code}")
                    }
                });
                out.push(build(
                    platform,
                    id,
                    text,
                    url,
                    (!user.is_empty()).then_some(user),
                    date10(entry, &["taken_at", "created_at", "date"]),
                    Engagement {
                        likes: n(entry, &["like_count", "likes"]),
                        comments: n(entry, &["reply_count", "replies_count", "comments"]),
                        score: n(entry, &["repost_count", "quote_count"]),
                        ..Default::default()
                    },
                ));
                if out.len() >= limit {
                    break;
                }
            }
        }
        SourceId::LinkedIn => {
            for entry in envelope_array(data, &["posts", "items", "data", "results"]) {
                let Some(url) = s(entry, &["url", "link", "post_url"]) else {
                    continue;
                };
                let text =
                    s(entry, &["text", "commentary", "content", "title"]).unwrap_or_default();
                let author = entry
                    .get("author")
                    .and_then(|a| s(a, &["name", "username"]))
                    .or_else(|| s(entry, &["author_name"]));
                out.push(build(
                    platform,
                    url.clone(),
                    text,
                    url,
                    author,
                    date10(
                        entry,
                        &["date_posted", "published_at", "date", "created_at"],
                    ),
                    Engagement {
                        likes: n(entry, &["num_likes", "likes_count", "likes"]),
                        comments: n(entry, &["num_comments", "comments_count", "comments"]),
                        score: n(entry, &["num_shares", "shares"]),
                        ..Default::default()
                    },
                ));
                if out.len() >= limit {
                    break;
                }
            }
        }
        SourceId::Pinterest => {
            for entry in envelope_array(data, &["pins", "results", "data", "items"]) {
                let id = s(entry, &["id"]).unwrap_or_default();
                let title = s(entry, &["title", "description", "grid_title"]).unwrap_or_default();
                let url = s(entry, &["link", "url"])
                    .unwrap_or_else(|| format!("https://www.pinterest.com/pin/{id}/"));
                let pinner = entry
                    .get("pinner")
                    .and_then(|p| s(p, &["username", "full_name"]));
                out.push(build(
                    platform,
                    id,
                    title,
                    url,
                    pinner,
                    date10(entry, &["created_at", "date"]),
                    Engagement {
                        score: n(entry, &["saves", "save_count", "repin_count"]),
                        comments: n(entry, &["comment_count", "comments"]),
                        ..Default::default()
                    },
                ));
                if out.len() >= limit {
                    break;
                }
            }
        }
        _ => {}
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn build(
    platform: SourceId,
    id: String,
    text: String,
    url: String,
    author: Option<String>,
    published_at: Option<String>,
    engagement: Engagement,
) -> SourceItem {
    let title: String = text.chars().take(110).collect();
    SourceItem {
        item_id: id,
        source: platform,
        title,
        body: text.clone(),
        url,
        author,
        container: Some(platform.as_str().to_string()),
        published_at,
        date_confidence: DateConfidence::Med,
        engagement,
        snippet: text.chars().take(240).collect(),
        metadata: BTreeMap::new(),
        local_relevance: None,
        entity_grounded: None,
        freshness: None,
        engagement_score: None,
        source_quality: None,
        local_rank_score: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_tiktok_aweme_envelope() {
        let data = json!({
            "search_item_list": [{
                "aweme_info": {
                    "aweme_id": "73001",
                    "desc": "Rust CLI tricks",
                    "create_time": 1763424000,
                    "author": {"unique_id": "devtok"},
                    "statistics": {"play_count": 90000, "digg_count": 4200, "comment_count": 130}
                }
            }]
        });
        let items = normalize(SourceId::TikTok, &data, 10);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].url, "https://www.tiktok.com/@devtok/video/73001");
        assert_eq!(items[0].engagement.views, Some(90000));
        assert_eq!(items[0].engagement.likes, Some(4200));
    }

    #[test]
    fn parses_linkedin_posts_envelope() {
        let data = json!({
            "posts": [{
                "url": "https://www.linkedin.com/posts/rust-123",
                "text": "Shipping Rust at scale",
                "num_likes": 240,
                "num_comments": 31,
                "author": {"name": "Jane"}
            }]
        });
        let items = normalize(SourceId::LinkedIn, &data, 10);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].engagement.likes, Some(240));
        assert_eq!(items[0].author.as_deref(), Some("Jane"));
    }
}
