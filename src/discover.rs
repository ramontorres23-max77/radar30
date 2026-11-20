//! Topic-less Discovery mode (`radar30 discover [domain]`):
//! sweeps Hacker News front/top stories and Reddit hot/rising listings, clusters nominations,
//! filters junk shapes, enforces an absolute confidence floor, reports an honest
//! `nothing_solid` outcome when no topic clears the floor, and annotates repeats via `topic_queue`.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::time::{Duration, Instant};

use anyhow::Result;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::http::{self, HttpError};
use crate::query::tokenize;
use crate::schema::{DateWindow, Depth, SourceId, SourceItem, SourceState};
use crate::store::Store;

/// Stable wire schema for JSON discovery output.
pub const DISCOVER_SCHEMA: &str = "radar30/discover@1";

/// Absolute confidence-floor thresholds (modeled after `last30days` `CONCEPTS.md`).
const MIN_ENGAGEMENT_FLOOR: f64 = 15.0;
const SINGLE_SOURCE_SPIKE_FLOOR: f64 = 180.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DiscoverOutcome {
    TrendsFound,
    NothingSolid,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DiscoveredTopic {
    pub name: String,
    pub velocity_score: f64,
    pub momentum: String,
    pub sources: Vec<SourceId>,
    pub item_count: usize,
    pub total_engagement: f64,
    pub junk_shape: bool,
    pub top_urls: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub queue_note: Option<String>,
    pub follow_up_cmd: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DiscoverReport {
    pub schema: String,
    pub version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
    pub outcome: DiscoverOutcome,
    pub generated_at: String,
    pub swept_items: usize,
    pub trends: Vec<DiscoveredTopic>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub weak_signal: Option<DiscoveredTopic>,
    pub source_states: BTreeMap<String, String>,
}

/// Injectable sweep provider so discovery is 100% testable offline.
pub trait RiverSweeper: Send + Sync {
    fn sweep(
        &self,
        domain: Option<&str>,
        depth: Depth,
        deadline: Instant,
    ) -> (Vec<SourceItem>, BTreeMap<String, String>);
}

/// Live river sweeper: Hacker News front page (`tags=front_page`) + Reddit via Arctic Shift
/// (`programming`/`technology` or domain-matched subreddit).
#[derive(Clone, Debug)]
pub struct LiveSweeper {
    pub per_request_timeout: Duration,
}

impl Default for LiveSweeper {
    fn default() -> Self {
        Self {
            per_request_timeout: Duration::from_secs(12),
        }
    }
}

impl RiverSweeper for LiveSweeper {
    fn sweep(
        &self,
        domain: Option<&str>,
        depth: Depth,
        deadline: Instant,
    ) -> (Vec<SourceItem>, BTreeMap<String, String>) {
        let timeout = self.per_request_timeout;
        let domain_owned = domain.map(ToOwned::to_owned);

        let (hn_res, reddit_res) = std::thread::scope(|s| {
            let d1 = domain_owned.clone();
            let h1 = s.spawn(move || sweep_hn_front(d1.as_deref(), depth, timeout, deadline));
            let d2 = domain_owned.clone();
            let h2 = s.spawn(move || sweep_reddit_river(d2.as_deref(), depth, timeout, deadline));
            (
                h1.join().expect("hn sweep panicked"),
                h2.join().expect("reddit sweep panicked"),
            )
        });

        let mut items = Vec::new();
        let mut states = BTreeMap::new();

        match hn_res {
            Ok(list) => {
                states.insert(
                    "hackernews".to_string(),
                    format!("ok ({} items)", list.len()),
                );
                items.extend(list);
            }
            Err(e) => {
                states.insert("hackernews".to_string(), format!("{:?}: {e}", e.state()));
            }
        }
        match reddit_res {
            Ok((list, label)) => {
                states.insert(
                    "reddit".to_string(),
                    format!("ok ({} items, {label})", list.len()),
                );
                items.extend(list);
            }
            Err(e) => {
                states.insert("reddit".to_string(), format!("{:?}: {e}", e.state()));
            }
        }
        (items, states)
    }
}

fn sweep_hn_front(
    domain: Option<&str>,
    depth: Depth,
    timeout: Duration,
    deadline: Instant,
) -> Result<Vec<SourceItem>, HttpError> {
    let limit = match depth {
        Depth::Quick => 20,
        Depth::Default => 35,
        Depth::Deep => 50,
    };
    let mut url =
        Url::parse("https://hn.algolia.com/api/v1/search").map_err(|e| HttpError::Decode {
            detail: e.to_string(),
        })?;
    {
        let mut pairs = url.query_pairs_mut();
        if let Some(d) = domain.filter(|s| !s.trim().is_empty()) {
            pairs.append_pair("query", d);
            pairs.append_pair("tags", "story");
        } else {
            pairs.append_pair("tags", "front_page");
        }
        pairs.append_pair("hitsPerPage", &limit.to_string());
    }
    let val: serde_json::Value = http::get_json(url.as_str(), &[], timeout, deadline, 1)?;
    let hits = val["hits"].as_array().cloned().unwrap_or_default();
    let mut out = Vec::new();
    for h in hits {
        let Some(title) = h["title"].as_str().filter(|s| !s.trim().is_empty()) else {
            continue;
        };
        let id = h["objectID"].as_str().unwrap_or("").to_string();
        let url = h["url"]
            .as_str()
            .filter(|s| !s.trim().is_empty())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| format!("https://news.ycombinator.com/item?id={id}"));
        let points = h["points"].as_i64().unwrap_or(0);
        let comments = h["num_comments"].as_i64().unwrap_or(0);
        out.push(SourceItem {
            item_id: id,
            source: SourceId::HackerNews,
            title: title.to_string(),
            body: String::new(),
            url,
            author: h["author"].as_str().map(ToOwned::to_owned),
            container: Some("news.ycombinator.com".to_string()),
            published_at: h["created_at"]
                .as_str()
                .and_then(|s| s.get(..10))
                .map(ToOwned::to_owned),
            date_confidence: crate::schema::DateConfidence::High,
            engagement: crate::schema::Engagement {
                score: Some(points),
                comments: Some(comments),
                ..Default::default()
            },
            snippet: String::new(),
            metadata: BTreeMap::new(),
            local_relevance: None,
            entity_grounded: None,
            freshness: None,
            engagement_score: None,
            source_quality: None,
            local_rank_score: None,
        });
    }
    Ok(out)
}

fn sweep_reddit_river(
    domain: Option<&str>,
    depth: Depth,
    timeout: Duration,
    deadline: Instant,
) -> Result<(Vec<SourceItem>, String), HttpError> {
    let today = Utc::now().date_naive();
    let window = DateWindow {
        from: today - chrono::Duration::days(7),
        to: today,
    };
    let query = domain
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("technology");
    let (items, detail) = crate::sources::reddit::fetch(query, window, depth, timeout, deadline)?;
    Ok((items, detail.unwrap_or_else(|| "reddit".to_string())))
}

/// Run Discovery over a `RiverSweeper`, applying junk-shape detection, confidence floor,
/// honest `nothing_solid` fallback, and optional `topic_queue` annotation.
pub fn run(
    domain: Option<&str>,
    depth: Depth,
    timeout: Duration,
    sweeper: &dyn RiverSweeper,
    store: Option<&Store>,
) -> Result<DiscoverReport> {
    let deadline = Instant::now() + timeout;
    let (raw_items, source_states) = sweeper.sweep(domain, depth, deadline);
    let swept_items = raw_items.len();

    let domain_tokens: Vec<String> = domain
        .map(|d| tokenize(d).into_iter().filter(|t| t.len() >= 2).collect())
        .unwrap_or_default();

    // Filter to domain when scoped, then cluster nominations by token overlap.
    let filtered: Vec<&SourceItem> = raw_items
        .iter()
        .filter(|it| {
            if domain_tokens.is_empty() {
                return true;
            }
            let lower = format!(
                "{} {} {}",
                it.title,
                it.snippet,
                it.container.as_deref().unwrap_or("")
            )
            .to_ascii_lowercase();
            domain_tokens.iter().any(|t| lower.contains(t.as_str()))
        })
        .collect();

    let mut groups: Vec<Vec<&SourceItem>> = Vec::new();
    for item in filtered {
        let tokens = significant_tokens(&item.title);
        let mut assigned = false;
        for g in &mut groups {
            let leader_tokens = significant_tokens(&g[0].title);
            if jaccard_tokens(&tokens, &leader_tokens) >= 0.35 {
                g.push(item);
                assigned = true;
                break;
            }
        }
        if !assigned {
            groups.push(vec![item]);
        }
    }

    let mut passing = Vec::new();
    let mut sub_floor = Vec::new();

    for group in groups {
        let mut sorted = group;
        sorted.sort_by(|a, b| {
            b.engagement
                .primary_metric()
                .total_cmp(&a.engagement.primary_metric())
        });
        let lead = sorted[0];
        let sources: BTreeSet<SourceId> = sorted.iter().map(|i| i.source).collect();
        let total_eng: f64 = sorted.iter().map(|i| i.engagement.primary_metric()).sum();
        let junk = is_junk_shape(&lead.title);

        // Confidence floor:
        // 1. Must clear MIN_ENGAGEMENT_FLOOR.
        // 2. Either cross-source corroboration (>= 2 distinct sources), OR (if non-junk)
        //    a genuinely strong single-source spike (>= SINGLE_SOURCE_SPIKE_FLOOR).
        let clears_floor = total_eng >= MIN_ENGAGEMENT_FLOOR
            && (sources.len() >= 2 || (!junk && total_eng >= SINGLE_SOURCE_SPIKE_FLOOR));

        let name = distill_topic_name(&lead.title);
        let cross_bonus = if sources.len() >= 2 { 25.0 } else { 0.0 };
        let velocity = (((1.0 + total_eng).ln() * 14.0) + cross_bonus).min(100.0);
        let velocity = (velocity * 10.0).round() / 10.0;
        let momentum = if sources.len() >= 2 && total_eng >= 250.0 {
            "surging"
        } else if total_eng >= 200.0 || sources.len() >= 2 {
            "rising"
        } else {
            "emerging"
        }
        .to_string();

        let top_urls: Vec<String> = sorted.iter().take(3).map(|i| i.url.clone()).collect();
        let mut card = DiscoveredTopic {
            follow_up_cmd: format!("radar30 research \"{}\"", name.replace('"', "")),
            name,
            velocity_score: velocity,
            momentum,
            sources: sources.into_iter().collect(),
            item_count: sorted.len(),
            total_engagement: total_eng.round(),
            junk_shape: junk,
            top_urls,
            queue_note: None,
        };

        if clears_floor {
            if let Some(st) = store
                && let Ok((prior, _)) = st.record_queue_sighting(&card.name, card.velocity_score)
                && let Some(prev) = prior
            {
                card.queue_note = if prev.status == "covered" {
                    Some(format!(
                        "already covered (surfaced {}x)",
                        prev.surfaced_count + 1
                    ))
                } else {
                    Some(format!("surfaced {}x before", prev.surfaced_count))
                };
            }
            passing.push(card);
        } else {
            sub_floor.push(card);
        }
    }

    passing.sort_by(|a, b| {
        b.velocity_score
            .total_cmp(&a.velocity_score)
            .then_with(|| b.total_engagement.total_cmp(&a.total_engagement))
    });
    passing.truncate(10);

    sub_floor.sort_by(|a, b| {
        a.junk_shape
            .cmp(&b.junk_shape)
            .then_with(|| b.velocity_score.total_cmp(&a.velocity_score))
    });

    let (outcome, weak_signal) = if passing.is_empty() {
        (DiscoverOutcome::NothingSolid, sub_floor.into_iter().next())
    } else {
        (DiscoverOutcome::TrendsFound, None)
    };

    Ok(DiscoverReport {
        schema: DISCOVER_SCHEMA.to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        domain: domain.map(ToOwned::to_owned),
        outcome,
        generated_at: Utc::now().to_rfc3339(),
        swept_items,
        trends: passing,
        weak_signal,
        source_states,
    })
}

/// Detect help-me / beginner-ask / personal-musing shapes that engagement alone can't distinguish from news.
pub fn is_junk_shape(title: &str) -> bool {
    let lower = title.trim().to_ascii_lowercase();
    let prefixes = [
        "help me",
        "need help",
        "beginner question",
        "newbie",
        "eli5",
        "am i the only one",
        "does anyone else",
        "how do i fix",
        "why does my",
        "what should i learn",
        "ask hn: who is hiring",
        "weekly thread",
        "daily thread",
    ];
    prefixes
        .iter()
        .any(|p| lower.starts_with(p) || lower.contains(p))
}

fn distill_topic_name(title: &str) -> String {
    let trimmed = title.trim();
    let stripped = trimmed
        .strip_prefix("Show HN:")
        .or_else(|| trimmed.strip_prefix("Tell HN:"))
        .or_else(|| trimmed.strip_prefix("Ask HN:"))
        .or_else(|| trimmed.strip_prefix("Launch HN:"))
        .unwrap_or(trimmed)
        .trim();
    stripped.chars().take(90).collect()
}

fn significant_tokens(text: &str) -> BTreeSet<String> {
    const STOP: &[&str] = &[
        "the", "and", "for", "with", "that", "this", "from", "your", "have", "about", "into",
        "what", "when", "where", "how", "why", "show", "ask", "tell",
    ];
    tokenize(text)
        .into_iter()
        .filter(|t| t.len() >= 3 && !STOP.contains(&t.as_str()))
        .collect()
}

fn jaccard_tokens(a: &BTreeSet<String>, b: &BTreeSet<String>) -> f64 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let inter = a.intersection(b).count() as f64;
    let union = a.union(b).count() as f64;
    if union == 0.0 { 0.0 } else { inter / union }
}

pub fn format_text(report: &DiscoverReport) -> String {
    let mut out = String::new();
    let scope = report.domain.as_deref().unwrap_or("global");
    let _ = writeln!(
        out,
        "# radar30 discover ({scope}) — swept {} items",
        report.swept_items
    );
    match report.outcome {
        DiscoverOutcome::NothingSolid => {
            let _ = writeln!(
                out,
                "\nOutcome: nothing-solid (zero topics cleared the confidence floor)."
            );
            if let Some(ref weak) = report.weak_signal {
                let _ = writeln!(
                    out,
                    "Closest weak signal: {} (velocity {:.1}, eng {:.0}, junk={})",
                    weak.name, weak.velocity_score, weak.total_engagement, weak.junk_shape
                );
            }
        }
        DiscoverOutcome::TrendsFound => {
            let _ = writeln!(out);
            for (i, t) in report.trends.iter().enumerate() {
                let srcs = t
                    .sources
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                let note = t
                    .queue_note
                    .as_deref()
                    .map(|n| format!(" [{n}]"))
                    .unwrap_or_default();
                let _ = writeln!(
                    out,
                    "{}. {} (velocity {:.1} · {} · {} · eng {:.0}){}",
                    i + 1,
                    t.name,
                    t.velocity_score,
                    t.momentum,
                    srcs,
                    t.total_engagement,
                    note
                );
                if let Some(u) = t.top_urls.first() {
                    let _ = writeln!(out, "   url: {u}");
                }
                let _ = writeln!(out, "   next: {}", t.follow_up_cmd);
            }
        }
    }
    let _ = writeln!(out, "\nSources:");
    for (k, v) in &report.source_states {
        let _ = writeln!(out, "  - {k}: {v}");
    }
    let _ = SourceState::Ok;
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{DateConfidence, Engagement};

    struct StaticSweeper(Vec<SourceItem>);
    impl RiverSweeper for StaticSweeper {
        fn sweep(
            &self,
            _domain: Option<&str>,
            _depth: Depth,
            _deadline: Instant,
        ) -> (Vec<SourceItem>, BTreeMap<String, String>) {
            (
                self.0.clone(),
                BTreeMap::from([("mock".into(), "ok".into())]),
            )
        }
    }

    fn mk(src: SourceId, title: &str, score: i64, comments: i64) -> SourceItem {
        SourceItem {
            item_id: title.to_string(),
            source: src,
            title: title.to_string(),
            body: String::new(),
            url: format!("https://example.com/{}", crate::library::slugify(title)),
            author: None,
            container: None,
            published_at: Some("2026-11-19".to_string()),
            date_confidence: DateConfidence::High,
            engagement: Engagement {
                score: Some(score),
                comments: Some(comments),
                ..Default::default()
            },
            snippet: String::new(),
            metadata: BTreeMap::new(),
            local_relevance: None,
            entity_grounded: None,
            freshness: None,
            engagement_score: None,
            source_quality: None,
            local_rank_score: None,
        }
    }

    #[test]
    fn discovery_enforces_confidence_floor_and_nothing_solid() {
        let weak_only = StaticSweeper(vec![
            mk(
                SourceId::HackerNews,
                "Help me fix my small script",
                500,
                100,
            ), // junk shape, single source -> blocked
            mk(SourceId::Reddit, "Tiny minor announcement", 8, 2), // below floor
        ]);
        let res = run(None, Depth::Quick, Duration::from_secs(5), &weak_only, None).unwrap();
        assert_eq!(res.outcome, DiscoverOutcome::NothingSolid);
        assert!(res.trends.is_empty());
        assert_eq!(
            res.weak_signal.as_ref().unwrap().name,
            "Tiny minor announcement"
        );
    }

    #[test]
    fn discovery_ranks_corroborated_story_and_annotates_queue() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(tmp.path().join("research.db")).unwrap();

        let sweeper = StaticSweeper(vec![
            mk(
                SourceId::HackerNews,
                "Rust 1.100 incremental linker released",
                120,
                40,
            ),
            mk(
                SourceId::Reddit,
                "Rust 1.100 incremental linker discussion",
                95,
                35,
            ),
        ]);
        let r1 = run(
            None,
            Depth::Quick,
            Duration::from_secs(5),
            &sweeper,
            Some(&store),
        )
        .unwrap();
        assert_eq!(r1.outcome, DiscoverOutcome::TrendsFound);
        assert_eq!(r1.trends.len(), 1);
        assert_eq!(r1.trends[0].sources.len(), 2);
        assert!(r1.trends[0].queue_note.is_none());

        let r2 = run(
            None,
            Depth::Quick,
            Duration::from_secs(5),
            &sweeper,
            Some(&store),
        )
        .unwrap();
        assert!(
            r2.trends[0]
                .queue_note
                .as_deref()
                .unwrap()
                .contains("surfaced 1x before")
        );
    }
}
