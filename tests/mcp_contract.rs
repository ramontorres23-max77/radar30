//! Contract tests keeping MCP tool metadata, prompts, error shapes, and `docs/tools.md` in step.

use std::path::Path;

use radar30::mcp;
use serde_json::json;

#[test]
fn every_mcp_tool_has_title_annotations_and_schema() {
    let tools = mcp::tools();
    assert!(tools.len() >= 4);
    for tool in &tools {
        let name = tool["name"].as_str().expect("tool name");
        assert!(name.starts_with("radar30_"), "unexpected tool name {name}");
        assert!(
            tool["title"].as_str().is_some_and(|s| !s.is_empty()),
            "{name} missing title"
        );
        let ann = &tool["annotations"];
        assert!(
            ann["readOnlyHint"].is_boolean(),
            "{name} missing readOnlyHint"
        );
        assert!(
            ann["destructiveHint"].is_boolean(),
            "{name} missing destructiveHint"
        );
        assert!(
            ann["openWorldHint"].is_boolean(),
            "{name} missing openWorldHint"
        );
        assert_eq!(tool["inputSchema"]["type"], "object");
    }
}

#[test]
fn failed_mcp_call_returns_structured_error_payload() {
    let req = json!({
        "jsonrpc": "2.0",
        "id": 99,
        "method": "tools/call",
        "params": {
            "name": "radar30_research",
            "arguments": { "topic": "   " }
        }
    });
    let resp = mcp::handle(&req).expect("response");
    assert_eq!(resp["result"]["isError"], true);
    let sc = &resp["result"]["structuredContent"];
    assert_eq!(sc["code"], "missing_topic");
    assert_eq!(sc["retryable"], false);
    assert!(sc["next"].as_str().is_some_and(|s| !s.is_empty()));
}

#[test]
fn docs_tools_md_matches_mcp_tool_registry() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/tools.md");
    let expected = mcp::render_tools_markdown();
    let actual = std::fs::read_to_string(&path).expect("docs/tools.md must exist");
    assert_eq!(
        actual, expected,
        "docs/tools.md drifted from src/mcp.rs::tools(); regenerate it"
    );
}
