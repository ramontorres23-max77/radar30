//! Polymarket keyless adapter via Gamma public search API (`gamma-api.polymarket.com`).

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::json;
use url::Url;

use crate::http::{self, HttpError};
use crate::schema::{DateConfidence, DateWindow, Depth, Engagement, SourceId, SourceItem};

const GAMMA_SEARCH_URL: &str = "https://gamma-api.polymarket.com/public-search";

#[derive(Deserialize)]
struct GammaSearchResponse {
    #[serde(default)]
    events: Vec<GammaEvent>,
}

#[derive(Deserialize)]
struct GammaEvent {
    #[serde(default)]
    id: String,
    #[serde(default)]
    slug: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    closed: Option<bool>,
    #[serde(default)]
    volume: Option<f64>,
    #[serde(rename = "updatedAt", default)]
    updated_at: Option<String>,
    #[serde(default)]
    markets: Vec<GammaMarket>,
}

#[derive(Deserialize)]
struct GammaMarket {
    question: Option<String>,
    #[serde(rename = "outcomePrices", default)]
    outcome_prices: Option<String>,
    #[serde(default)]
    closed: Option<bool>,
    #[serde(rename = "volumeNum", default)]
    volume_num: Option<f64>,
}

pub fn fetch(
    query: &str,
    _window: DateWindow,
    depth: Depth,
    timeout: Duration,
    deadline: Instant,
) -> Result<(Vec<SourceItem>, Option<String>), HttpError> {
    let limit = match depth {
        Depth::Quick => 8,
        Depth::Default => 15,
        Depth::Deep => 25,
    };

    let mut url = Url::parse(GAMMA_SEARCH_URL).map_err(|e| HttpError::Decode {
        detail: e.to_string(),
    })?;
    url.query_pairs_mut()
        .append_pair("q", query)
        .append_pair("limit_per_type", &limit.to_string());

    let parsed: GammaSearchResponse = http::get_json(url.as_str(), &[], timeout, deadline, 2)?;
    Ok((
        normalize_events(parsed.events, limit),
        Some("gamma-api".to_string()),
    ))
}

fn normalize_events(events: Vec<GammaEvent>, limit: usize) -> Vec<SourceItem> {
    let mut out = Vec::new();
    for ev in events {
        if ev.closed == Some(true) || ev.title.trim().is_empty() {
            continue;
        }
        let (lead_prob, lead_question) = best_live_market(&ev.markets);
        let volume = ev
            .volume
            .unwrap_or_else(|| ev.markets.iter().filter_map(|m| m.volume_num).sum::<f64>());
        let event_url = if !ev.slug.is_empty() {
            format!("https://polymarket.com/event/{}", ev.slug)
        } else {
            "https://polymarket.com/".to_string()
        };

        let mut snippet_parts = Vec::new();
        if let Some(p) = lead_prob {
            snippet_parts.push(format!("Leading odds: {:.0}%", p * 100.0));
        }
        if volume > 0.0 {
            snippet_parts.push(format!("Volume: ${}", format_usd(volume)));
        }
        if let Some(q) = lead_question {
            snippet_parts.push(q);
        } else if let Some(desc) = ev.description.as_deref() {
            snippet_parts.push(desc.chars().take(160).collect());
        }

        let published_at = ev
            .updated_at
            .as_deref()
            .and_then(|s| s.get(..10))
            .map(ToOwned::to_owned);

        let mut metadata = BTreeMap::new();
        if let Some(p) = lead_prob {
            metadata.insert(
                "probability_pct".to_string(),
                json!((p * 1000.0).round() / 10.0),
            );
        }

        out.push(SourceItem {
            item_id: ev.id,
            source: SourceId::Polymarket,
            title: ev.title,
            body: ev.description.unwrap_or_default(),
            url: event_url,
            author: None,
            container: Some("polymarket.com".to_string()),
            published_at,
            date_confidence: DateConfidence::Med,
            engagement: Engagement {
                volume_usd: (volume > 0.0).then_some(volume),
                probability: lead_prob,
                ..Default::default()
            },
            snippet: snippet_parts.join(" · "),
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

fn best_live_market(markets: &[GammaMarket]) -> (Option<f64>, Option<String>) {
    let mut best: Option<(f64, f64, Option<String>)> = None;
    for m in markets {
        if m.closed == Some(true) {
            continue;
        }
        let prob = m
            .outcome_prices
            .as_deref()
            .and_then(parse_first_price)
            .filter(|&p| (0.005..=0.98).contains(&p));
        let vol = m.volume_num.unwrap_or(0.0);
        if let Some(p) = prob {
            let replace = match &best {
                None => true,
                Some((_, b_vol, _)) => vol > *b_vol,
            };
            if replace {
                best = Some((p, vol, m.question.clone()));
            }
        }
    }
    match best {
        Some((p, _, q)) => (Some(p), q),
        None => (None, None),
    }
}

fn parse_first_price(raw: &str) -> Option<f64> {
    let arr: Vec<serde_json::Value> = serde_json::from_str(raw).ok()?;
    let first = arr.first()?;
    if let Some(n) = first.as_f64() {
        Some(n)
    } else {
        first.as_str()?.parse::<f64>().ok()
    }
}

fn format_usd(amount: f64) -> String {
    if amount >= 1_000_000.0 {
        format!("{:.1}M", amount / 1_000_000.0)
    } else if amount >= 1_000.0 {
        format!("{:.1}K", amount / 1_000.0)
    } else {
        format!("{amount:.0}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_live_polymarket_events_and_skips_closed() {
        let events = vec![
            GammaEvent {
                id: "1".into(),
                slug: "closed-ev".into(),
                title: "Old resolved event".into(),
                description: None,
                closed: Some(true),
                volume: Some(50000.0),
                updated_at: None,
                markets: vec![],
            },
            GammaEvent {
                id: "2".into(),
                slug: "ai-event".into(),
                title: "Will Rust hit top 5 TIOBE?".into(),
                description: Some("Prediction market".into()),
                closed: Some(false),
                volume: Some(125_000.0),
                updated_at: Some("2026-11-18T10:00:00Z".into()),
                markets: vec![GammaMarket {
                    question: Some("Yes before Dec 31?".into()),
                    outcome_prices: Some(r#"["0.68", "0.32"]"#.into()),
                    closed: Some(false),
                    volume_num: Some(125_000.0),
                }],
            },
        ];
        let items = normalize_events(events, 10);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].engagement.probability, Some(0.68));
        assert!(items[0].snippet.contains("68%"));
    }
}
