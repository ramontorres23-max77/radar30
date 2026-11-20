//! MCP prompts built from the embedded workflow skill (`skills/radar30/SKILL.md`).

use serde_json::{Value, json};

const SKILL: &str = include_str!("../skills/radar30/SKILL.md");

/// Return the `prompts/list` payload.
pub fn list() -> Value {
    json!({
        "prompts": [
            {
                "name": "research_topic",
                "title": "Research a topic over the last 30 days",
                "description": "Gather multi-source community evidence (Reddit, HN, GitHub, Polymarket, arXiv) and synthesize a cited brief.",
                "arguments": [
                    { "name": "topic", "description": "Subject to research (product, person, company, or event).", "required": true },
                    { "name": "depth", "description": "quick, default, or deep (default: default).", "required": false }
                ]
            },
            {
                "name": "compare_topics",
                "title": "Compare two topics over the last 30 days",
                "description": "Run a comparison research pass across community sources and produce a side-by-side synthesis.",
                "arguments": [
                    { "name": "topic_a", "description": "First subject.", "required": true },
                    { "name": "topic_b", "description": "Second subject.", "required": true }
                ]
            }
        ]
    })
}

/// Resolve `prompts/get` into a ready-to-send MCP prompt message list.
pub fn get(name: &str, arguments: &Value) -> Result<Value, String> {
    let arg = |key: &str| arguments[key].as_str().filter(|s| !s.trim().is_empty());
    let (description, text) = match name {
        "research_topic" => {
            let topic = arg("topic")
                .ok_or_else(|| "prompt `research_topic` needs the `topic` argument".to_string())?;
            let depth = arg("depth").unwrap_or("default");
            (
                "Research a topic over the last 30 days",
                format!(
                    "Research `{topic}` over the last 30 days using `radar30_research` (depth={depth}), \
                     then synthesize a concise, cited brief.\n\nFollow this guide:\n\n{}",
                    body(SKILL)
                ),
            )
        }
        "compare_topics" => {
            let a = arg("topic_a")
                .ok_or_else(|| "prompt `compare_topics` needs `topic_a`".to_string())?;
            let b = arg("topic_b")
                .ok_or_else(|| "prompt `compare_topics` needs `topic_b`".to_string())?;
            (
                "Compare two topics over the last 30 days",
                format!(
                    "Compare `{a} vs {b}` over the last 30 days using `radar30_research` and produce a \
                     side-by-side synthesis with citations and caveats.\n\nFollow this guide:\n\n{}",
                    body(SKILL)
                ),
            )
        }
        _ => return Err(format!("unknown prompt `{name}`")),
    };

    Ok(json!({
        "description": description,
        "messages": [
            {
                "role": "user",
                "content": { "type": "text", "text": text }
            }
        ]
    }))
}

fn body(markdown: &str) -> &str {
    markdown
        .strip_prefix("---\n")
        .and_then(|rest| rest.split_once("\n---\n"))
        .map_or(markdown, |(_, body)| body)
        .trim()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompts_list_and_get_work() {
        let listed = list();
        assert_eq!(listed["prompts"].as_array().unwrap().len(), 2);

        let prompt = get("research_topic", &json!({ "topic": "Rust" })).unwrap();
        let text = prompt["messages"][0]["content"]["text"].as_str().unwrap();
        assert!(text.contains("Rust"));
        assert!(text.contains("radar30_research"));
        assert!(!text.starts_with("---"));

        assert!(get("research_topic", &json!({})).is_err());
        assert!(get("nonexistent", &json!({})).is_err());
    }
}
