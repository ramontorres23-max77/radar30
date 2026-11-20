//! URL canonicalization, richer-copy merge, and Weighted Reciprocal Rank Fusion (RRF).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

use regex::Regex;
use url::Url;

use crate::schema::{Candidate, DateWindow, Depth, QueryPlan, SourceId, SourceItem};

/// Standard RRF smoothing constant (Cormack et al. 2009).
pub const RRF_K: f64 = 60.0;

/// One ranked stream returned by `(subquery_label, source)`.
#[derive(Clone, Debug)]
pub struct RankedStream {
    pub subquery_label: String,
    pub source: SourceId,
    pub weight: f64,
    pub items: Vec<SourceItem>,
}

/// Normalize a URL for deduplication: lowercase host, strip `www.`/`old.`/`m.`,
/// strip `utm_*` tracking params, trim trailing slashes, and drop fragments.
pub fn normalize_url(raw: &str) -> String {
    let trimmed = raw.trim();
    let Ok(mut parsed) = Url::parse(trimmed) else {
        return trimmed.to_ascii_lowercase();
    };
    parsed.set_fragment(None);
    if let Some(host) = parsed.host_str() {
        let mut h = host.to_ascii_lowercase();
        for prefix in ["www.", "old.", "m."] {
            if let Some(stripped) = h.strip_prefix(prefix) {
                h = stripped.to_string();
                break;
            }
        }
        let _ = parsed.set_host(Some(&h));
    }
    let clean_pairs: Vec<(String, String)> = parsed
        .query_pairs()
        .filter(|(k, _)| {
            let lk = k.to_ascii_lowercase();
            !lk.starts_with("utm_") && lk != "ref" && lk != "fbclid"
        })
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    if clean_pairs.is_empty() {
        parsed.set_query(None);
    } else {
        let mut pairs = parsed.query_pairs_mut();
        pairs.clear();
        for (k, v) in clean_pairs {
            pairs.append_pair(&k, &v);
        }
    }
    let trimmed_path = parsed.path().trim_end_matches('/').to_string();
    parsed.set_path(&trimmed_path);
    parsed.to_string().trim_end_matches('/').to_string()
}

/// Canonical Reddit thread key (`reddit:t3_<id>`) when the URL points to a Reddit thread.
pub fn reddit_thread_key(raw: &str) -> Option<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"(?i)(?:reddit\.com/r/[^/]+/comments/|redd\.it/)([a-z0-9]{5,10})")
            .expect("valid regex")
    });
    re.captures(raw)
        .and_then(|c| c.get(1))
        .map(|m| format!("reddit:{}", m.as_str().to_ascii_lowercase()))
}

/// Cross-stream candidate identity key.
pub fn candidate_key(item: &SourceItem) -> String {
    if !item.url.trim().is_empty() {
        if let Some(rkey) = reddit_thread_key(&item.url) {
            return rkey;
        }
        return normalize_url(&item.url);
    }
    format!("{}:{}", item.source.as_str(), item.item_id)
}

/// Merge richer fields from `incoming` into `existing`.
fn merge_item_into(existing: &mut SourceItem, incoming: &SourceItem) {
    existing.engagement.merge_max(&incoming.engagement);
    if incoming.body.len() > existing.body.len() {
        existing.body = incoming.body.clone();
    }
    if incoming.snippet.len() > existing.snippet.len() {
        existing.snippet = incoming.snippet.clone();
    }
    if existing.published_at.is_none() && incoming.published_at.is_some() {
        existing.published_at = incoming.published_at.clone();
        existing.date_confidence = incoming.date_confidence;
    }
    for (k, v) in &incoming.metadata {
        existing
            .metadata
            .entry(k.clone())
            .or_insert_with(|| v.clone());
    }
}

/// Fuse all `(subquery, source)` streams via Weighted RRF, reserving engagement-keeper
/// slots for top-discussed grounded threads, and capping at `depth.pool_limit()`.
pub fn fuse_streams(
    streams: &[RankedStream],
    plan: &QueryPlan,
    window: DateWindow,
    depth: Depth,
) -> Vec<Candidate> {
    struct Acc {
        canonical_key: String,
        primary: SourceItem,
        sources: BTreeSet<SourceId>,
        subquery_labels: BTreeSet<String>,
        native_ranks: BTreeMap<String, usize>,
        rrf_score: f64,
        max_relevance: f64,
        any_grounded: bool,
        max_freshness: u32,
        max_eng_score: f64,
        max_quality: f64,
        all_out_of_window: bool,
    }

    let per_stream = depth.per_stream_limit();
    let mut map: BTreeMap<String, Acc> = BTreeMap::new();

    for stream in streams {
        let src_weight = plan
            .source_weights
            .get(stream.source.as_str())
            .copied()
            .unwrap_or_else(|| stream.source.quality_weight());

        // Keep top `per_stream` plus top 3 grounded high-engagement keepers.
        let mut kept_indices: BTreeSet<usize> = (0..stream.items.len().min(per_stream)).collect();
        let mut eng_order: Vec<usize> = (0..stream.items.len())
            .filter(|&i| {
                let it = &stream.items[i];
                it.entity_grounded.unwrap_or(false) && it.local_relevance.unwrap_or(0.0) >= 0.20
            })
            .collect();
        eng_order.sort_by(|&a, &b| {
            stream.items[b]
                .engagement
                .primary_metric()
                .total_cmp(&stream.items[a].engagement.primary_metric())
        });
        for idx in eng_order.into_iter().take(depth.engagement_keeper_slots()) {
            kept_indices.insert(idx);
        }

        for idx in kept_indices {
            let item = &stream.items[idx];
            let rank = idx + 1;
            let key = candidate_key(item);
            let stream_key = format!("{}:{}", stream.subquery_label, stream.source.as_str());
            let rrf_contrib = (stream.weight * src_weight) / (RRF_K + rank as f64);

            let rel = item.local_relevance.unwrap_or(0.0);
            let grounded = item.entity_grounded.unwrap_or(true);
            let fresh = item.freshness.unwrap_or(45);
            let eng = item.engagement_score.unwrap_or(0.0);
            let qual = item
                .source_quality
                .unwrap_or_else(|| item.source.quality_weight());
            let oow = item.is_out_of_window(window);

            map.entry(key.clone())
                .and_modify(|acc| {
                    merge_item_into(&mut acc.primary, item);
                    acc.sources.insert(item.source);
                    acc.subquery_labels.insert(stream.subquery_label.clone());
                    acc.native_ranks
                        .entry(stream_key.clone())
                        .and_modify(|r| *r = (*r).min(rank))
                        .or_insert(rank);
                    acc.rrf_score += rrf_contrib;
                    acc.max_relevance = acc.max_relevance.max(rel);
                    acc.any_grounded = acc.any_grounded || grounded;
                    acc.max_freshness = acc.max_freshness.max(fresh);
                    acc.max_eng_score = acc.max_eng_score.max(eng);
                    acc.max_quality = acc.max_quality.max(qual);
                    acc.all_out_of_window = acc.all_out_of_window && oow;
                })
                .or_insert_with(|| {
                    let mut sources = BTreeSet::new();
                    sources.insert(item.source);
                    let mut labels = BTreeSet::new();
                    labels.insert(stream.subquery_label.clone());
                    let mut ranks = BTreeMap::new();
                    ranks.insert(stream_key, rank);
                    Acc {
                        canonical_key: key,
                        primary: item.clone(),
                        sources,
                        subquery_labels: labels,
                        native_ranks: ranks,
                        rrf_score: rrf_contrib,
                        max_relevance: rel,
                        any_grounded: grounded,
                        max_freshness: fresh,
                        max_eng_score: eng,
                        max_quality: qual,
                        all_out_of_window: oow,
                    }
                });
        }
    }

    let max_rrf = map
        .values()
        .map(|a| a.rrf_score)
        .fold(0.0_f64, f64::max)
        .max(1e-9);

    let mut candidates: Vec<Candidate> = map
        .into_values()
        .map(|acc| {
            let norm_rrf = (acc.rrf_score / max_rrf).clamp(0.0, 1.0);
            let corroboration_bonus = if acc.sources.len() >= 2 {
                ((acc.sources.len() - 1) as f64 * 6.0).min(15.0)
            } else {
                0.0
            };
            let mut final_score = (norm_rrf * 38.0)
                + (acc.max_relevance * 27.0)
                + (acc.max_eng_score * 20.0)
                + (f64::from(acc.max_freshness) * 0.15)
                + corroboration_bonus;
            if !acc.any_grounded {
                final_score *= 0.25;
            }
            if acc.all_out_of_window {
                final_score *= 0.45;
            }
            let final_score = (final_score * 100.0).round() / 100.0;
            let snippet = if !acc.primary.snippet.is_empty() {
                acc.primary.snippet.clone()
            } else {
                acc.primary.body.chars().take(240).collect()
            };

            Candidate {
                candidate_id: String::new(),
                canonical_key: acc.canonical_key,
                primary_source: acc.primary.source,
                sources: acc.sources.into_iter().collect(),
                title: acc.primary.title,
                url: acc.primary.url,
                snippet,
                published_at: acc.primary.published_at,
                out_of_window: acc.all_out_of_window,
                subquery_labels: acc.subquery_labels.into_iter().collect(),
                native_ranks: acc.native_ranks,
                local_relevance: (acc.max_relevance * 1000.0).round() / 1000.0,
                entity_grounded: acc.any_grounded,
                freshness: acc.max_freshness,
                engagement: acc.primary.engagement,
                engagement_score: (acc.max_eng_score * 1000.0).round() / 1000.0,
                source_quality: acc.max_quality,
                rrf_score: (acc.rrf_score * 10000.0).round() / 10000.0,
                final_score,
                cluster_id: None,
                metadata: acc.primary.metadata,
            }
        })
        .collect();

    // Out-of-window evidence sorts strictly below in-window evidence.
    candidates.sort_by(|a, b| {
        a.out_of_window
            .cmp(&b.out_of_window)
            .then_with(|| b.final_score.total_cmp(&a.final_score))
            .then_with(|| b.rrf_score.total_cmp(&a.rrf_score))
            .then_with(|| a.title.cmp(&b.title))
    });

    candidates.truncate(depth.pool_limit());
    for (i, c) in candidates.iter_mut().enumerate() {
        c.candidate_id = format!("cand-{}", i + 1);
    }
    candidates
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::build_plan;
    use crate::schema::{DateConfidence, Engagement};
    use chrono::NaiveDate;

    #[test]
    fn url_normalization_strips_www_and_utm() {
        assert_eq!(
            normalize_url("https://www.Example.com/post/?utm_source=hn&id=42#top"),
            "https://example.com/post?id=42"
        );
        assert_eq!(
            reddit_thread_key("https://old.reddit.com/r/rust/comments/1abcde2/title/"),
            Some("reddit:1abcde2".to_string())
        );
    }

    #[test]
    fn rrf_fuses_duplicate_urls_across_sources_and_boosts_corroboration() {
        let plan = build_plan("Rust", &[SourceId::HackerNews, SourceId::Reddit]);
        let window = DateWindow {
            from: NaiveDate::from_ymd_opt(2026, 10, 20).unwrap(),
            to: NaiveDate::from_ymd_opt(2026, 11, 19).unwrap(),
        };
        let make = |src: SourceId, url: &str, title: &str| SourceItem {
            item_id: "1".to_string(),
            source: src,
            title: title.to_string(),
            body: String::new(),
            url: url.to_string(),
            author: None,
            container: None,
            published_at: Some("2026-11-18".to_string()),
            date_confidence: DateConfidence::High,
            engagement: Engagement {
                score: Some(100),
                comments: Some(30),
                ..Default::default()
            },
            snippet: String::new(),
            metadata: BTreeMap::new(),
            local_relevance: Some(0.9),
            entity_grounded: Some(true),
            freshness: Some(95),
            engagement_score: Some(0.7),
            source_quality: Some(src.quality_weight()),
            local_rank_score: Some(80.0),
        };

        let streams = vec![
            RankedStream {
                subquery_label: "primary".to_string(),
                source: SourceId::HackerNews,
                weight: 1.0,
                items: vec![make(
                    SourceId::HackerNews,
                    "https://blog.rust-lang.org/2026/release?utm_source=hn",
                    "Rust 1.100 released",
                )],
            },
            RankedStream {
                subquery_label: "primary".to_string(),
                source: SourceId::Reddit,
                weight: 1.0,
                items: vec![make(
                    SourceId::Reddit,
                    "https://www.blog.rust-lang.org/2026/release/",
                    "Rust 1.100 released",
                )],
            },
        ];

        let fused = fuse_streams(&streams, &plan, window, Depth::Default);
        assert_eq!(fused.len(), 1);
        assert_eq!(fused[0].sources.len(), 2);
        assert_eq!(fused[0].candidate_id, "cand-1");
    }
}
