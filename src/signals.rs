//! Local scoring signals: token-overlap relevance, entity grounding, freshness,
//! and cross-platform log1p engagement normalization.

use std::collections::HashSet;

use chrono::NaiveDate;

use crate::query::tokenize;
use crate::schema::{DateWindow, QueryPlan, SourceId, SourceItem};

const STOP_WORDS: &[&str] = &[
    "the", "a", "an", "in", "on", "at", "of", "for", "and", "or", "to", "is", "are", "was", "were",
    "be", "by", "with", "from", "as", "it", "its", "not", "this", "that", "what", "who", "how",
    "why", "vs", "versus",
];

/// Compute normalized token-overlap relevance in `[0.0, 1.0]` between `query` and `text`.
pub fn token_overlap_relevance(query: &str, text: &str) -> f64 {
    let q_tokens: Vec<String> = tokenize(query)
        .into_iter()
        .filter(|t| t.len() >= 2 && !STOP_WORDS.contains(&t.as_str()))
        .collect();
    if q_tokens.is_empty() {
        return 0.5;
    }

    let doc_tokens: HashSet<String> = tokenize(text).into_iter().collect();
    if doc_tokens.is_empty() {
        return 0.0;
    }

    let mut matched = 0usize;
    let unique_q: HashSet<&str> = q_tokens.iter().map(String::as_str).collect();
    for &qt in &unique_q {
        if doc_tokens.contains(qt) || (qt.len() >= 4 && doc_tokens.iter().any(|dt| dt.contains(qt)))
        {
            matched += 1;
        }
    }

    let ratio = matched as f64 / unique_q.len() as f64;
    // Exact phrase bonus when the full query appears verbatim.
    let lower_q = query.trim().to_ascii_lowercase();
    let lower_text = text.to_ascii_lowercase();
    let phrase_bonus = if lower_q.len() >= 3 && lower_text.contains(&lower_q) {
        0.15
    } else {
        0.0
    };
    (ratio + phrase_bonus).clamp(0.0, 1.0)
}

/// Conservative entity grounding: checks whether the item plausibly mentions
/// the head token of the primary entity.
///
/// Mirrors last30days `CONCEPTS.md`: grounding keys on the head token rather
/// than the full phrase, and degrades conservatively toward grounded (`true`)
/// if the head token is empty or very short (< 2 chars).
pub fn is_entity_grounded(item: &SourceItem, plan: &QueryPlan) -> bool {
    let head = plan.head_token.trim();
    if head.len() < 2 {
        return true;
    }
    let combined = format!(
        "{} {} {} {}",
        item.title,
        item.snippet,
        item.body,
        item.container.as_deref().unwrap_or("")
    )
    .to_ascii_lowercase();

    if combined.contains(head) {
        return true;
    }
    // For comparison queries ("A vs B"), matching any side's head token counts as grounded.
    if plan.subqueries.len() > 1 {
        for sq in &plan.subqueries[1..] {
            let side_head = crate::query::extract_head_token(&sq.search_query);
            if side_head.len() >= 2 && combined.contains(&side_head) {
                return true;
            }
        }
    }
    false
}

/// Compute recency freshness score in `[0, 100]` relative to `window.to`.
pub fn freshness_score(item: &SourceItem, window: DateWindow) -> u32 {
    let Some(pub_date) = item.published_date() else {
        return 45;
    };
    recency_from_date(pub_date, window.to, window.days())
}

fn recency_from_date(pub_date: NaiveDate, reference: NaiveDate, window_days: i64) -> u32 {
    let age = (reference - pub_date).num_days();
    if age <= 0 {
        return 100;
    }
    let span = window_days.max(1) as f64;
    let ratio = (age as f64) / span;
    if ratio <= 1.0 {
        // Linear + gentle curve inside window: 100 down to 35 at window edge.
        (100.0 - 65.0 * ratio).round().clamp(35.0, 100.0) as u32
    } else {
        // Out of window decays cleanly toward 5.
        (30.0 / ratio).round().clamp(5.0, 30.0) as u32
    }
}

/// Cross-platform log1p engagement score normalized to `[0.0, 1.0]`.
pub fn engagement_score(item: &SourceItem) -> f64 {
    let (raw, log_ref) = match item.source {
        SourceId::Reddit => {
            let s = item.engagement.score.unwrap_or(0).max(0) as f64;
            let c = item.engagement.comments.unwrap_or(0).max(0) as f64;
            (s + 1.5 * c, 7.6) // ~log1p(2000)
        }
        SourceId::HackerNews => {
            let s = item.engagement.score.unwrap_or(0).max(0) as f64;
            let c = item.engagement.comments.unwrap_or(0).max(0) as f64;
            (s + 1.2 * c, 6.5) // ~log1p(660)
        }
        SourceId::GitHub => {
            let st = item.engagement.stars.unwrap_or(0).max(0) as f64;
            let s = item.engagement.score.unwrap_or(0).max(0) as f64;
            let c = item.engagement.comments.unwrap_or(0).max(0) as f64;
            (st + 2.0 * s + 3.0 * c, 8.5)
        }
        SourceId::Polymarket => {
            let vol = item.engagement.volume_usd.unwrap_or(0.0).max(0.0);
            (vol, 13.8) // ~log1p($1M)
        }
        SourceId::DevTo => {
            let s = item.engagement.score.unwrap_or(0).max(0) as f64;
            let c = item.engagement.comments.unwrap_or(0).max(0) as f64;
            (s + 2.0 * c, 5.5)
        }
        SourceId::Arxiv => {
            // Academic papers have no social upvotes; neutral baseline.
            return 0.40;
        }
        SourceId::Bluesky => {
            let lk = item.engagement.likes.unwrap_or(0).max(0) as f64;
            let s = item.engagement.score.unwrap_or(0).max(0) as f64;
            let c = item.engagement.comments.unwrap_or(0).max(0) as f64;
            (lk + 2.0 * s + c, 9.2)
        }
        SourceId::X => {
            let lk = item.engagement.likes.unwrap_or(0).max(0) as f64;
            let s = item.engagement.score.unwrap_or(0).max(0) as f64;
            let c = item.engagement.comments.unwrap_or(0).max(0) as f64;
            (lk + 2.0 * s + c, 9.2)
        }
        SourceId::YouTube => {
            let v = item.engagement.views.unwrap_or(0).max(0) as f64;
            (v, 13.8) // ~log1p(1M views)
        }
        SourceId::TikTok | SourceId::Instagram => {
            let v = item.engagement.views.unwrap_or(0).max(0) as f64;
            let lk = item.engagement.likes.unwrap_or(0).max(0) as f64;
            (v + 10.0 * lk, 13.8)
        }
        SourceId::Threads | SourceId::LinkedIn => {
            let lk = item.engagement.likes.unwrap_or(0).max(0) as f64;
            let c = item.engagement.comments.unwrap_or(0).max(0) as f64;
            (lk + 2.0 * c, 7.6)
        }
        SourceId::Pinterest => {
            let s = item.engagement.score.unwrap_or(0).max(0) as f64;
            (s, 7.6) // saves
        }
        SourceId::Perplexity => return 0.50,
        SourceId::Web => return 0.40,
    };
    if raw <= 0.0 {
        return 0.0;
    }
    ((1.0 + raw).ln() / log_ref).clamp(0.0, 1.0)
}

/// Annotate every item in a subquery stream with local relevance, grounding, freshness,
/// engagement, source quality, and composite `local_rank_score`, then sort descending.
pub fn annotate_and_sort_stream(
    items: &mut [SourceItem],
    ranking_query: &str,
    plan: &QueryPlan,
    window: DateWindow,
) {
    for item in items.iter_mut() {
        let combined = format!("{} {} {}", item.title, item.snippet, item.body);
        let rel = token_overlap_relevance(ranking_query, &combined);
        let grounded = is_entity_grounded(item, plan);
        let fresh = freshness_score(item, window);
        let eng = engagement_score(item);
        let qual = item.source.quality_weight();

        // Composite local rank score in [0, 100]; entity-miss applies decisive demotion.
        let mut score = (rel * 45.0) + (eng * 25.0) + (f64::from(fresh) * 0.20) + (qual * 10.0);
        if !grounded {
            score *= 0.25;
        }
        if item.is_out_of_window(window) {
            score *= 0.50;
        }

        item.local_relevance = Some(rel);
        item.entity_grounded = Some(grounded);
        item.freshness = Some(fresh);
        item.engagement_score = Some(eng);
        item.source_quality = Some(qual);
        item.local_rank_score = Some((score * 100.0).round() / 100.0);
    }

    items.sort_by(|a, b| {
        let a_oow = a.is_out_of_window(window);
        let b_oow = b.is_out_of_window(window);
        a_oow
            .cmp(&b_oow)
            .then_with(|| {
                b.local_rank_score
                    .unwrap_or(0.0)
                    .total_cmp(&a.local_rank_score.unwrap_or(0.0))
            })
            .then_with(|| a.title.cmp(&b.title))
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::build_plan;
    use crate::schema::{DateConfidence, Engagement};
    use std::collections::BTreeMap;

    fn sample_item(title: &str, score: i64, date: &str) -> SourceItem {
        SourceItem {
            item_id: "1".to_string(),
            source: SourceId::HackerNews,
            title: title.to_string(),
            body: String::new(),
            url: "https://example.com/1".to_string(),
            author: None,
            container: None,
            published_at: Some(date.to_string()),
            date_confidence: DateConfidence::High,
            engagement: Engagement {
                score: Some(score),
                comments: Some(20),
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
    fn entity_miss_demotes_off_topic_viral_post() {
        let plan = build_plan("Rust compiler", &[SourceId::HackerNews]);
        let window = DateWindow {
            from: NaiveDate::from_ymd_opt(2026, 10, 20).unwrap(),
            to: NaiveDate::from_ymd_opt(2026, 11, 19).unwrap(),
        };
        let mut items = vec![
            sample_item("Unrelated viral meme", 5000, "2026-11-18"),
            sample_item(
                "Rust compiler speeds up incremental builds",
                80,
                "2026-11-17",
            ),
        ];
        annotate_and_sort_stream(&mut items, "Rust compiler", &plan, window);
        assert!(items[0].title.contains("Rust compiler"));
        assert_eq!(items[0].entity_grounded, Some(true));
        assert_eq!(items[1].entity_grounded, Some(false));
    }
}
