//! Query parsing, primary entity extraction, intent classification, and subquery planning.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use regex::Regex;

use crate::schema::{Intent, QueryPlan, SourceId, SubQuery};

/// Leading conversational wrappers stripped before extracting the core subject.
fn leading_prefixes() -> &'static [Regex] {
    static PATTERNS: OnceLock<Vec<Regex>> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        [
            r"(?i)^last\s+\d+\s+days?\s+",
            r"(?i)^what(?:'s| is| are)\s+(?:people saying about|happening with|going on with|new with|trending in)\s+",
            r"(?i)^how\s+(?:is|are)\s+",
            r"(?i)^tell\s+me\s+about\s+",
            r"(?i)^research\s+",
        ]
        .into_iter()
        .map(|p| Regex::new(p).expect("static regex is valid"))
        .collect()
    })
}

/// Trailing intent modifiers stripped when deriving the primary entity.
const TRAILING_MODIFIERS: &[&str] = &[
    "best practices",
    "use cases",
    "pros and cons",
    "benchmarks",
    "benchmark",
    "comparison",
    "alternatives",
    "alternative",
    "predictions",
    "prediction",
    "sentiment",
    "discussion",
    "reactions",
    "reaction",
    "opinion",
    "opinions",
    "reviews",
    "review",
    "pricing",
    "updates",
    "update",
    "release",
    "prompting",
    "prompts",
    "news",
    "odds",
];

/// Generic stop words skipped when picking a meaningful head token.
const GENERIC_HEAD_WORDS: &[&str] = &[
    "the", "a", "an", "in", "on", "of", "for", "and", "or", "to", "is", "are", "how", "what",
    "why", "best", "top", "new", "latest", "recent", "ai", "tech",
];

/// Strip conversational wrappers from a raw topic string.
pub fn extract_core_subject(topic: &str) -> String {
    let mut cleaned = topic.trim().trim_matches('?').trim().to_string();
    for re in leading_prefixes() {
        let next = re.replace(&cleaned, "").trim().to_string();
        if !next.is_empty() {
            cleaned = next;
        }
    }
    cleaned
}

/// Derive the primary entity (topic with trailing intent modifiers stripped).
pub fn extract_primary_entity(topic: &str) -> String {
    let core = extract_core_subject(topic);
    let lower = core.to_ascii_lowercase();
    for modifier in TRAILING_MODIFIERS {
        if let Some(prefix) = lower.strip_suffix(modifier) {
            let trimmed = prefix.trim_end_matches([' ', '-', ':', ',']);
            if !trimmed.is_empty() {
                return core[..trimmed.len()].trim().to_string();
            }
        }
    }
    core
}

/// Extract the head token of the primary entity used for entity grounding.
pub fn extract_head_token(primary_entity: &str) -> String {
    let tokens: Vec<String> = tokenize(primary_entity);
    for token in &tokens {
        if token.len() >= 2 && !GENERIC_HEAD_WORDS.contains(&token.as_str()) {
            return token.clone();
        }
    }
    tokens.into_iter().next().unwrap_or_default()
}

/// Tokenize text into lowercase alphanumeric tokens (preserving short tech terms like `go`, `c++`).
pub fn tokenize(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric() && c != '+' && c != '#')
        .filter_map(|part| {
            let lower = part.trim().to_ascii_lowercase();
            if lower.is_empty() { None } else { Some(lower) }
        })
        .collect()
}

/// Classify query intent from topic phrasing.
pub fn classify_intent(topic: &str) -> Intent {
    let lower = topic.to_ascii_lowercase();
    if lower.contains(" vs ") || lower.contains(" versus ") || lower.contains("compare ") {
        return Intent::Comparison;
    }
    if [
        "odds",
        "polymarket",
        "prediction",
        "forecast",
        "will ",
        "chance",
    ]
    .iter()
    .any(|kw| lower.contains(kw))
    {
        return Intent::Prediction;
    }
    if [
        "breaking", "launch", "released", "release", "dropped", "news", "outage", "ban",
    ]
    .iter()
    .any(|kw| lower.contains(kw))
    {
        return Intent::BreakingNews;
    }
    if [
        "review",
        "worth it",
        "opinion",
        "thoughts",
        "debate",
        "saying about",
        "pros and cons",
    ]
    .iter()
    .any(|kw| lower.contains(kw))
    {
        return Intent::Opinion;
    }
    Intent::BreakingNews
}

/// Build a deterministic `QueryPlan` for the selected sources.
pub fn build_plan(raw_topic: &str, sources: &[SourceId]) -> QueryPlan {
    let raw_trimmed = raw_topic.trim().to_string();
    let core = extract_core_subject(&raw_trimmed);
    let primary_entity = extract_primary_entity(&raw_trimmed);
    let head_token = extract_head_token(&primary_entity);
    let intent = classify_intent(&raw_trimmed);

    let mut subqueries = Vec::new();
    let mut notes = Vec::new();

    // Primary subquery across all enabled sources.
    subqueries.push(SubQuery {
        label: "primary".to_string(),
        search_query: core.clone(),
        ranking_query: core.clone(),
        sources: sources.to_vec(),
        weight: 1.0,
    });

    // If the user asked for a comparison ("A vs B"), add per-entity subqueries so
    // each side gets fair evidence coverage before RRF fusion.
    if intent == Intent::Comparison {
        let sides = split_comparison_sides(&core);
        if sides.len() >= 2 {
            notes.push(format!("comparison-split: {} sides", sides.len()));
            for (idx, side) in sides.into_iter().take(3).enumerate() {
                subqueries.push(SubQuery {
                    label: format!("side-{}", idx + 1),
                    search_query: side.clone(),
                    ranking_query: format!("{side} {core}"),
                    sources: sources.to_vec(),
                    weight: 0.75,
                });
            }
        }
    } else if !primary_entity.eq_ignore_ascii_case(&core) && !primary_entity.is_empty() {
        // Secondary entity-focused subquery when an intent modifier was stripped.
        subqueries.push(SubQuery {
            label: "entity".to_string(),
            search_query: primary_entity.clone(),
            ranking_query: core.clone(),
            sources: sources.to_vec(),
            weight: 0.70,
        });
    }

    let mut source_weights = BTreeMap::new();
    for &src in sources {
        let base = src.quality_weight();
        let adjusted = match (intent, src) {
            (Intent::Prediction, SourceId::Polymarket) => 1.15,
            (Intent::BreakingNews, SourceId::HackerNews | SourceId::Reddit) => base + 0.10,
            (Intent::Comparison, SourceId::GitHub | SourceId::HackerNews) => base + 0.08,
            _ => base,
        };
        source_weights.insert(src.as_str().to_string(), adjusted);
    }

    QueryPlan {
        raw_topic: raw_trimmed,
        primary_entity,
        head_token,
        intent,
        subqueries,
        source_weights,
        notes,
    }
}

fn split_comparison_sides(core: &str) -> Vec<String> {
    static VS_RE: OnceLock<Regex> = OnceLock::new();
    let re = VS_RE.get_or_init(|| Regex::new(r"(?i)\s+(?:vs\.?|versus)\s+").expect("valid regex"));
    re.split(core)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_primary_entity_and_head_token() {
        assert_eq!(
            extract_primary_entity("what are people saying about Rust pricing?"),
            "Rust"
        );
        assert_eq!(
            extract_primary_entity("Claude Code best practices"),
            "Claude Code"
        );
        assert_eq!(extract_head_token("Claude Code"), "claude");
        assert_eq!(extract_head_token("The Rust Compiler"), "rust");
    }

    #[test]
    fn classifies_comparison_and_splits_subqueries() {
        let plan = build_plan("Bun vs Deno", &[SourceId::HackerNews, SourceId::GitHub]);
        assert_eq!(plan.intent, Intent::Comparison);
        assert_eq!(plan.subqueries.len(), 3);
        assert_eq!(plan.subqueries[1].search_query, "Bun");
        assert_eq!(plan.subqueries[2].search_query, "Deno");
    }

    #[test]
    fn classifies_prediction_intent_and_boosts_polymarket() {
        let plan = build_plan(
            "Fed rate cut odds",
            &[SourceId::Polymarket, SourceId::Reddit],
        );
        assert_eq!(plan.intent, Intent::Prediction);
        assert!(plan.source_weights["polymarket"] > plan.source_weights["reddit"]);
    }
}
