//! Perplexity Sonar synthesis lane (`api.perplexity.ai/chat/completions`, BYOK).
//! Returns one synthesis item carrying the model's answer and its citations.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::json;

use crate::http::{self, HttpError};
use crate::schema::{DateConfidence, DateWindow, Depth, Engagement, SourceId, SourceItem};

const CHAT_URL: &str = "https://api.perplexity.ai/chat/completions";

#[derive(Deserialize)]
struct ChatResponse {
    #[serde(default)]
    choices: Vec<Choice>,
    #[serde(default)]
    citations: Vec<String>,
}

#[derive(Deserialize)]
struct Choice {
    message: Option<Message>,
}

#[derive(Deserialize)]
struct Message {
    #[serde(default)]
    content: String,
}

pub fn fetch(
    query: &str,
    window: DateWindow,
    _depth: Depth,
    timeout: Duration,
    deadline: Instant,
    token: &str,
) -> Result<(Vec<SourceItem>, Option<String>), HttpError> {
    let recency = if window.days() <= 7 { "week" } else { "month" };
    let body = json!({
        "model": "sonar",
        "messages": [
            {"role": "system", "content": "Answer with concise, cited facts about recent developments. Prefer primary sources."},
            {"role": "user", "content": format!("What are the most significant recent developments about: {query}?")}
        ],
        "search_recency_filter": recency
    });
    let auth = format!("Bearer {token}");
    let headers = [("Authorization", auth.as_str())];
    let parsed: ChatResponse = http::post_json(CHAT_URL, &headers, &body, timeout, deadline, 1)?;

    let content = parsed
        .choices
        .first()
        .and_then(|c| c.message.as_ref())
        .map(|m| m.content.trim().to_string())
        .unwrap_or_default();
    if content.is_empty() {
        return Ok((Vec::new(), Some("sonar (empty)".to_string())));
    }

    let url = parsed
        .citations
        .first()
        .cloned()
        .unwrap_or_else(|| "https://www.perplexity.ai".to_string());
    let mut metadata = BTreeMap::new();
    metadata.insert("citations".to_string(), json!(parsed.citations));

    let item = SourceItem {
        item_id: format!("pplx-{}", crate::library::slugify(query)),
        source: SourceId::Perplexity,
        title: format!("Perplexity synthesis: {query}"),
        body: content.clone(),
        url,
        author: Some("sonar".to_string()),
        container: Some("perplexity.ai".to_string()),
        published_at: Some(window.to.format("%Y-%m-%d").to_string()),
        date_confidence: DateConfidence::Med,
        engagement: Engagement::default(),
        snippet: content.chars().take(240).collect(),
        metadata,
        local_relevance: None,
        entity_grounded: None,
        freshness: None,
        engagement_score: None,
        source_quality: None,
        local_rank_score: None,
    };
    Ok((vec![item], Some(format!("sonar/{recency}"))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserializes_sonar_response_with_citations() {
        let raw = json!({
            "choices": [{"message": {"content": "Rust 1.100 shipped faster builds [1]."}}],
            "citations": ["https://blog.rust-lang.org/1.100"]
        });
        let parsed: ChatResponse = serde_json::from_value(raw).unwrap();
        assert_eq!(parsed.citations.len(), 1);
        assert!(
            parsed.choices[0]
                .message
                .as_ref()
                .unwrap()
                .content
                .contains("Rust")
        );
    }
}
