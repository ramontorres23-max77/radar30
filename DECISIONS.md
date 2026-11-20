# Decisions

Architectural and product choices for `radar30`, with reasons. Each decision can be revisited.

## Product & Architecture

| # | Decision | Reason |
| --- | --- | --- |
| D1 | Single pure-Rust binary (`#![forbid(unsafe_code)]`, Rust 2024) providing CLI, stdio MCP server, `setup`, `doctor`, and embedded `SKILL.md`. | Eliminates the Python engine + Go MCP wrapper split of `last30days-skill`, following `rust-anything`'s single-binary architecture for fast startup, zero runtime dependencies, and uniform distribution. |
| D2 | Synchronous HTTP (`ureq` + rustls) + `std::thread::scope` parallel fan-out instead of a heavy async runtime. | Keeps compilation fast, binary small, TLS pure-Rust, and matches `rust-anything`'s scoped OS thread model with explicit per-run deadlines. |
| D3 | Hand-written stdio JSON-RPC 2.0 MCP server (`src/mcp.rs`) without an MCP SDK. | The stdio MCP surface (`initialize`, `ping`, `tools/list`, `tools/call`, `prompts/list`, `prompts/get`, progress, cancellation) is compact; writing it directly avoids SDK churn and keeps full control over structured errors and thread isolation. |
| D4 | Keep `docs/tools.md` generated from `src/mcp.rs::tools()` and verified by `tests/mcp_contract.rs`. | Prevents documentation drift between the CLI/MCP schema and published tool reference. |
| D5 | Injectable `SourceFetcher` trait (`LiveFetcher` vs `FixtureFetcher`) + `--offline-fixtures`. | Allows 100% deterministic offline integration tests without mocking HTTP globals, while running the exact same pipeline code in production. |

## Sources & Pipeline

| # | Decision | Reason |
| --- | --- | --- |
| S1 | Wave 1 ships 6 keyless public sources: Reddit (Arctic Shift), Hacker News (Algolia), GitHub (REST Search), Polymarket (Gamma), DEV.to (Forem, opt-in), and arXiv (native Atom XML). | Works out of the box with zero API keys; `GITHUB_TOKEN` is optional for higher limits. |
| S2 | Query Reddit via `/svc/shreddit/search/` first, falling back to Arctic Shift subreddit-prefix discovery + `/api/posts/search?subreddit=...`. | Reddit's anonymous `.json` endpoints return HTTP 403, and rustls/ureq can also hit 403 on shreddit; Arctic Shift rejects `title` without `subreddit`/`author`, so discovering the top matching subreddit first recovers live Reddit threads keyless. |
| S3 | Parse arXiv Atom XML natively with `quick-xml` instead of shelling out to `arxiv-pp-cli`. | Zero external binary dependency; preserves quoted-phrase + AND fallback and 365-day recency window. |
| S4 | Filter HN minimum points client-side after 2x overfetch instead of Algolia `numericFilters=points>N`. | HN Algolia rejects `points` in `numericFilters` with HTTP 400. |
| S5 | Weighted RRF (`K = 60`), conservative head-token entity grounding, out-of-window partitioning, and greedy + MMR clustering. | Prevents off-topic viral posts and stale out-of-window items from outranking recent, corroborated on-entity signal. |
| S6 | `setup` writes Claude/Codex/Cursor/Gemini/OpenCode MCP configs atomically with `.radar30.bak` backups and installs `SKILL.md` for Claude/Codex/OpenCode/Hermes (without mutating `~/.hermes/config.yaml` comments). | Preserves user settings and YAML comments safely while giving zero-friction skill installation. |
