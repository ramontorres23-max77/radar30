//! YouTube adapter via the local `yt-dlp` binary (`ytsearchN:<query>`, flat playlist).
//!
//! Runs bounded through `crate::process` (stdin closed, hard kill at timeout) so a
//! hung extractor can never stall a run. Flat search returns view counts and titles;
//! full transcript extraction is a later refinement.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::http::HttpError;
use crate::process;
use crate::schema::{DateConfidence, DateWindow, Depth, Engagement, SourceId, SourceItem};

#[derive(Deserialize)]
struct FlatPlaylist {
    #[serde(default)]
    entries: Vec<FlatEntry>,
}

#[derive(Deserialize)]
struct FlatEntry {
    #[serde(default)]
    id: String,
    #[serde(default)]
    title: String,
    url: Option<String>,
    view_count: Option<i64>,
    channel: Option<String>,
    uploader: Option<String>,
    duration: Option<f64>,
}

pub fn fetch(
    query: &str,
    _window: DateWindow,
    depth: Depth,
    timeout: Duration,
    deadline: Instant,
) -> Result<(Vec<SourceItem>, Option<String>), HttpError> {
    let count = match depth {
        Depth::Quick => 8,
        Depth::Default => 15,
        Depth::Deep => 25,
    };
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err(HttpError::Timeout {
            detail: "deadline exceeded before yt-dlp".to_string(),
        });
    }
    let budget = timeout.min(remaining).max(Duration::from_secs(3));
    let search = format!("ytsearch{count}:{query}");
    let output = process::run_capture(
        "yt-dlp",
        &[
            "--flat-playlist",
            "--dump-single-json",
            "--no-warnings",
            "--skip-download",
            &search,
        ],
        budget,
    )
    .map_err(|e| match e {
        process::ProcError::Missing(d) => HttpError::Unreachable { detail: d },
        process::ProcError::Timeout(d) => HttpError::Timeout { detail: d },
        process::ProcError::Failed(d) => HttpError::Status {
            status: 0,
            detail: d,
        },
    })?;

    let parsed: FlatPlaylist = serde_json::from_str(&output).map_err(|e| HttpError::Decode {
        detail: format!("yt-dlp JSON: {e}"),
    })?;
    Ok((
        normalize_entries(parsed.entries),
        Some("yt-dlp/flat".to_string()),
    ))
}

fn normalize_entries(entries: Vec<FlatEntry>) -> Vec<SourceItem> {
    let mut out = Vec::new();
    for e in entries {
        if e.title.trim().is_empty() || e.id.is_empty() {
            continue;
        }
        let url = e
            .url
            .filter(|u| u.starts_with("http"))
            .unwrap_or_else(|| format!("https://www.youtube.com/watch?v={}", e.id));
        let author = e.channel.or(e.uploader);
        let mut metadata = BTreeMap::new();
        if let Some(d) = e.duration {
            metadata.insert("duration_seconds".to_string(), serde_json::json!(d));
        }
        out.push(SourceItem {
            item_id: e.id,
            source: SourceId::YouTube,
            title: e.title,
            body: String::new(),
            url,
            author,
            container: Some("youtube.com".to_string()),
            // Flat playlist rarely carries upload dates; freshness stays neutral.
            published_at: None,
            date_confidence: DateConfidence::Low,
            engagement: Engagement {
                views: e.view_count,
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
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_flat_playlist_entries() {
        let entries = vec![FlatEntry {
            id: "abc123".into(),
            title: "Rust 1.100 deep dive".into(),
            url: None,
            view_count: Some(120_000),
            channel: Some("RustConf".into()),
            uploader: None,
            duration: Some(930.0),
        }];
        let items = normalize_entries(entries);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].url, "https://www.youtube.com/watch?v=abc123");
        assert_eq!(items[0].engagement.views, Some(120_000));
    }
}
