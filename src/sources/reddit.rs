//! Keyless Reddit adapter via Reddit's server-rendered shreddit search fragment
//! (`/svc/shreddit/search/?q=...&type=posts&t=...&sort=relevance`).
//!
//! Reddit's anonymous `.json` endpoints return HTTP 403, and Arctic Shift requires
//! an explicit `subreddit` or `author` for `title` searches. Reddit's own shreddit
//! search HTML fragment serves HTTP 200 keyless with post id, title, subreddit,
//! canonical permalink, timestamp, vote count, and comment count.

use std::collections::{BTreeMap, HashSet};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use chrono::{DateTime, NaiveTime};
use regex::Regex;
use serde::Deserialize;
use serde_json::{Value, json};
use url::Url;

use crate::http::{self, HttpError};
use crate::schema::{DateConfidence, DateWindow, Depth, Engagement, SourceId, SourceItem};

const SHREDDIT_SEARCH_URL: &str = "https://www.reddit.com/svc/shreddit/search/";
const ARCTIC_SUBS_URL: &str = "https://arctic-shift.photon-reddit.com/api/subreddits/search";
const ARCTIC_POSTS_URL: &str = "https://arctic-shift.photon-reddit.com/api/posts/search";
const BROWSER_UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";

#[derive(Deserialize)]
struct ArcticSubsResp {
    #[serde(default)]
    data: Vec<ArcticSub>,
}

#[derive(Deserialize)]
struct ArcticSub {
    #[serde(default)]
    display_name: String,
    #[serde(default)]
    subscribers: Option<i64>,
}

#[derive(Deserialize)]
struct ArcticPostsResp {
    #[serde(default)]
    data: Vec<ArcticPost>,
}

#[derive(Deserialize)]
struct ArcticPost {
    #[serde(default)]
    id: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    selftext: String,
    #[serde(default)]
    subreddit: String,
    author: Option<String>,
    permalink: Option<String>,
    url: Option<String>,
    score: Option<i64>,
    num_comments: Option<i64>,
    created_utc: Option<f64>,
}

pub fn fetch(
    query: &str,
    window: DateWindow,
    depth: Depth,
    timeout: Duration,
    deadline: Instant,
) -> Result<(Vec<SourceItem>, Option<String>), HttpError> {
    match fetch_shreddit(query, window, depth, timeout, deadline) {
        Ok(res) if !res.0.is_empty() => Ok(res),
        shreddit_res => {
            // Fallback to Arctic Shift when Reddit blocks non-browser TLS fingerprints (403)
            // or returns empty. Discover matching subreddit(s) via prefix search first.
            match fetch_arctic_fallback(query, window, depth, timeout, deadline) {
                Ok(res) if !res.0.is_empty() => Ok(res),
                Ok(_) => shreddit_res,
                Err(_) => shreddit_res,
            }
        }
    }
}

fn fetch_shreddit(
    query: &str,
    window: DateWindow,
    depth: Depth,
    timeout: Duration,
    deadline: Instant,
) -> Result<(Vec<SourceItem>, Option<String>), HttpError> {
    let time_filter = match window.days() {
        d if d <= 1 => "day",
        d if d <= 7 => "week",
        d if d <= 31 => "month",
        _ => "year",
    };

    let mut url = Url::parse(SHREDDIT_SEARCH_URL).map_err(|e| HttpError::Decode {
        detail: e.to_string(),
    })?;
    url.query_pairs_mut()
        .append_pair("q", query)
        .append_pair("type", "posts")
        .append_pair("t", time_filter)
        .append_pair("sort", "relevance");

    let headers = [
        ("User-Agent", BROWSER_UA),
        ("Accept", "text/html,application/xhtml+xml"),
        ("Accept-Language", "en-US,en;q=0.9"),
    ];
    let html = http::get_text(url.as_str(), &headers, timeout, deadline, 1)?;
    let limit = depth.source_fetch_limit();
    let items = parse_shreddit_html(&html, limit);
    if items.is_empty()
        && !html.contains("search/view/no_results")
        && !html.contains("search/view/post")
    {
        return Err(HttpError::Decode {
            detail: "unrecognized Reddit search response (possible challenge page)".to_string(),
        });
    }
    Ok((items, Some("shreddit/search".to_string())))
}

fn fetch_arctic_fallback(
    query: &str,
    window: DateWindow,
    depth: Depth,
    timeout: Duration,
    deadline: Instant,
) -> Result<(Vec<SourceItem>, Option<String>), HttpError> {
    let prefix: String = crate::query::extract_head_token(query)
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    if prefix.len() < 2 {
        return Ok((Vec::new(), None));
    }

    let mut sub_url = Url::parse(ARCTIC_SUBS_URL).map_err(|e| HttpError::Decode {
        detail: e.to_string(),
    })?;
    sub_url
        .query_pairs_mut()
        .append_pair("subreddit_prefix", &prefix)
        .append_pair("limit", "5");

    let subs: ArcticSubsResp = http::get_json(sub_url.as_str(), &[], timeout, deadline, 1)?;
    let mut ranked_subs: Vec<ArcticSub> = subs
        .data
        .into_iter()
        .filter(|s| !s.display_name.is_empty())
        .collect();
    ranked_subs.sort_by_key(|s| std::cmp::Reverse(s.subscribers.unwrap_or(0)));
    let Some(best_sub) = ranked_subs.first() else {
        return Ok((Vec::new(), None));
    };

    let after = window.from.and_time(NaiveTime::MIN).and_utc().timestamp();
    let before = window.to.and_time(NaiveTime::MIN).and_utc().timestamp() + 86_400;
    let limit = depth.source_fetch_limit().min(30);

    let mut posts_url = Url::parse(ARCTIC_POSTS_URL).map_err(|e| HttpError::Decode {
        detail: e.to_string(),
    })?;
    posts_url
        .query_pairs_mut()
        .append_pair("subreddit", &best_sub.display_name)
        .append_pair("after", &after.to_string())
        .append_pair("before", &before.to_string())
        .append_pair("sort", "desc")
        .append_pair("limit", &limit.to_string());

    let posts: ArcticPostsResp = http::get_json(posts_url.as_str(), &[], timeout, deadline, 2)?;
    let items = normalize_arctic_posts(posts.data);
    Ok((
        items,
        Some(format!("arctic-shift/r/{}", best_sub.display_name)),
    ))
}

fn normalize_arctic_posts(posts: Vec<ArcticPost>) -> Vec<SourceItem> {
    let mut out = Vec::new();
    for post in posts {
        let title = post.title.trim().to_string();
        if title.is_empty() {
            continue;
        }
        let permalink = match post.permalink {
            Some(ref p) if p.starts_with('/') => format!("https://www.reddit.com{p}"),
            Some(ref p) if !p.is_empty() => p.clone(),
            _ if !post.id.is_empty() => format!("https://redd.it/{}", post.id),
            _ => post.url.clone().unwrap_or_default(),
        };
        if permalink.is_empty() {
            continue;
        }
        let published_at = post
            .created_utc
            .and_then(|ts| DateTime::from_timestamp(ts as i64, 0))
            .map(|dt| dt.format("%Y-%m-%d").to_string());
        let body = if post.selftext == "[removed]" || post.selftext == "[deleted]" {
            String::new()
        } else {
            post.selftext.trim().to_string()
        };
        let snippet: String = body.chars().take(240).collect();

        let mut metadata = BTreeMap::new();
        if !post.subreddit.is_empty() {
            metadata.insert("subreddit".to_string(), json!(post.subreddit));
        }

        out.push(SourceItem {
            item_id: post.id,
            source: SourceId::Reddit,
            title,
            body,
            url: permalink,
            author: post.author,
            container: (!post.subreddit.is_empty()).then(|| format!("r/{}", post.subreddit)),
            published_at,
            date_confidence: DateConfidence::High,
            engagement: Engagement {
                score: post.score,
                comments: post.num_comments,
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

fn tracker_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r#"<search-telemetry-tracker\b[^>]*?\bdata-faceplate-tracking-context="([^"]*)""#,
        )
        .expect("valid regex")
    })
}

fn title_href_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r#"<a\b[^>]*?data-testid="post-title"[^>]*?\bhref="([^"]*)"|<a\b[^>]*?\bhref="([^"]*)"[^>]*?data-testid="post-title""#,
        )
        .expect("valid regex")
    })
}

fn timeago_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"<faceplate-timeago\b[^>]*?\bts="([^"]+)""#).expect("valid regex")
    })
}

fn counter_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r#"<faceplate-number\b[^>]*?\bnumber="(\d+)"[^>]*>\s*</faceplate-number>\s*(vote|comment)"#,
        )
        .expect("valid regex")
    })
}

/// Parse Reddit's `/svc/shreddit/search/` HTML fragment into normalized `SourceItem`s.
pub fn parse_shreddit_html(html: &str, limit: usize) -> Vec<SourceItem> {
    let mut units: Vec<(usize, Value)> = Vec::new();
    for cap in tracker_re().captures_iter(html) {
        let Some(m) = cap.get(0) else { continue };
        let Some(raw_ctx) = cap.get(1) else { continue };
        let unescaped = html_unescape(raw_ctx.as_str());
        let Ok(ctx) = serde_json::from_str::<Value>(&unescaped) else {
            continue;
        };
        if ctx["action_info"]["type"].as_str() == Some("post")
            && ctx["post"]["id"].as_str().is_some_and(|s| !s.is_empty())
        {
            units.push((m.start(), ctx));
        }
    }

    let mut out = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    for (idx, (start, ctx)) in units.iter().enumerate() {
        let raw_id = ctx["post"]["id"].as_str().unwrap_or("");
        let post_id = raw_id.strip_prefix("t3_").unwrap_or(raw_id).to_string();
        if post_id.is_empty() || !seen.insert(post_id.clone()) {
            continue;
        }
        let end = units.get(idx + 1).map_or(html.len(), |(pos, _)| *pos);
        let chunk = &html[*start..end];

        let title = ctx["post"]["title"]
            .as_str()
            .unwrap_or("")
            .trim()
            .to_string();
        if title.is_empty() {
            continue;
        }
        let subreddit = ctx["subreddit"]["name"]
            .as_str()
            .unwrap_or("")
            .trim()
            .to_string();

        let href = title_href_re()
            .captures(chunk)
            .and_then(|c| c.get(1).or_else(|| c.get(2)))
            .map(|m| html_unescape(m.as_str()))
            .unwrap_or_default();
        let href_clean = href.split('?').next().unwrap_or("");
        let url = if href_clean.starts_with("/r/")
            && href_clean.contains(&format!("/comments/{post_id}/"))
        {
            format!("https://www.reddit.com{href_clean}")
        } else if !subreddit.is_empty() {
            format!("https://www.reddit.com/r/{subreddit}/comments/{post_id}/")
        } else {
            format!("https://redd.it/{post_id}")
        };

        let published_at = timeago_re()
            .captures(chunk)
            .and_then(|c| c.get(1))
            .and_then(|m| m.as_str().get(..10))
            .map(ToOwned::to_owned);

        let mut score: Option<i64> = None;
        let mut comments: Option<i64> = None;
        for c in counter_re().captures_iter(chunk) {
            let num = c.get(1).and_then(|m| m.as_str().parse::<i64>().ok());
            match c.get(2).map(|m| m.as_str()) {
                Some("vote") if score.is_none() => score = num,
                Some("comment") if comments.is_none() => comments = num,
                _ => {}
            }
        }

        let mut metadata = BTreeMap::new();
        if !subreddit.is_empty() {
            metadata.insert("subreddit".to_string(), json!(subreddit));
        }

        out.push(SourceItem {
            item_id: post_id,
            source: SourceId::Reddit,
            title,
            body: String::new(),
            url,
            author: None,
            container: (!subreddit.is_empty()).then(|| format!("r/{subreddit}")),
            published_at,
            date_confidence: DateConfidence::High,
            engagement: Engagement {
                score,
                comments,
                ..Default::default()
            },
            snippet: String::new(),
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

fn html_unescape(s: &str) -> String {
    s.replace("&quot;", "\"")
        .replace("&#34;", "\"")
        .replace("&#x27;", "'")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_shreddit_search_fragment() {
        let html = r#"
        <div view-events="search/view/post">
          <search-telemetry-tracker data-faceplate-tracking-context="{&quot;action_info&quot;:{&quot;type&quot;:&quot;post&quot;},&quot;post&quot;:{&quot;id&quot;:&quot;t3_1abc23&quot;,&quot;title&quot;:&quot;Rust 1.100 released&quot;},&quot;subreddit&quot;:{&quot;name&quot;:&quot;rust&quot;}}">
            <a data-testid="post-title" href="/r/rust/comments/1abc23/rust_1100_released/?ref=search">Rust 1.100 released</a>
          </search-telemetry-tracker>
          <faceplate-timeago ts="2026-11-18T14:22:00.000Z"></faceplate-timeago>
          <faceplate-number number="342"></faceplate-number> votes
          <faceplate-number number="89"></faceplate-number> comments
        </div>
        "#;
        let items = parse_shreddit_html(html, 10);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].item_id, "1abc23");
        assert_eq!(
            items[0].url,
            "https://www.reddit.com/r/rust/comments/1abc23/rust_1100_released/"
        );
        assert_eq!(items[0].container.as_deref(), Some("r/rust"));
        assert_eq!(items[0].published_at.as_deref(), Some("2026-11-18"));
        assert_eq!(items[0].engagement.score, Some(342));
        assert_eq!(items[0].engagement.comments, Some(89));
    }
}
