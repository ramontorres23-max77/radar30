# Status

What works today and how each part was last checked. Updated 2026-11-19.

**Verified**: executed end to end against real public APIs or live stdio MCP process.
**Tested**: covered by automated offline unit/integration tests in `cargo test`.
**Planned**: scheduled for Waves 3–4 (see `docs/PLAN.md`).

## Wave 1 Components

| Component | Status | Notes |
| --- | --- | --- |
| `research` pipeline (query plan, grounding, RRF, clustering, renderers) | Verified | Live run (`radar30 research "Rust" --depth quick --sources reddit,hackernews,github,devto,arxiv`): 48 fetched, 15 candidates, 14 clusters; offline fixture test covers `compact`, `md`, and `json` (`radar30/research@1`). |
| Source: Hacker News (`algolia/v1`) | Verified | Live run returned 12 items in 1.3s. |
| Source: Reddit (`shreddit/search` + Arctic Shift fallback) | Verified | `shreddit/search` returned HTTP 403 on non-browser TLS; Arctic Shift subreddit-prefix fallback (`arctic-shift/r/rust`) recovered 12 items automatically. |
| Source: GitHub (`rest-search`) | Verified | Live anonymous run returned 10 items in 1.4s. |
| Source: DEV.to (`forem/tag`) | Verified | Live opt-in run returned 12 items in 1.0s. |
| Source: arXiv (`atom/phrase` + AND fallback) | Verified | Live run returned 2 recent papers in 0.9s via native `quick-xml` parser. |
| Source: Polymarket (`gamma-api`) | Tested | Parser & live-market filtering covered by unit tests; `gamma-api.polymarket.com` did not resolve on the test machine's DNS (`curl` exit 6), so live Polymarket verification depends on network/DNS access. |
| `preflight`, `sources` | Verified | Executed live with zero network calls and zero file writes. |
| `doctor` + live MCP self-test | Verified | `radar30 doctor` passed (`mcp:self-test`: `initialize` + `tools/list` with 7 tools in 5 ms). |
| `setup` (Claude, Codex, Cursor, Gemini, OpenCode, Hermes) | Verified | Executed live; all detected agents registered and verified `ok` by `radar30 doctor`. |
| `mcp` stdio server (JSON-RPC 2.0, progress, cancellation, prompts) | Verified | In-process stdio test + live `doctor` spawn + `tests/mcp_contract.rs` (`docs/tools.md` drift check). |

## Wave 2 Components

| Component | Status | Notes |
| --- | --- | --- |
| SQLite store (`src/store.rs`, bundled SQLite WAL + FTS5 + `topic_queue`) | Verified | Live `watch run` persisted 12 findings; unit & integration tests verify deduping, `sighting_count`, FTS5 BM25 search, and `covered` queue persistence. |
| `discover` (HN front page + Reddit river sweep, junk filter, confidence floor, `nothing-solid`) | Verified | Live global sweep collected 32 items (HN 20 + Reddit 12) and ranked 10 trends; unit tests verify confidence floor, junk-shape blocking, `nothing-solid`, and repeat queue annotations. |
| `library search` & `library feed` (`index.html`, `briefs/*.html`, Atom `feed.xml`) | Verified | Live smoke run merged saved brief + FTS5 findings and built `index.html` + `feed.xml`; offline integration test covers full round-trip. |
| `watch` (`add`, `remove`, `list`, `run`) & `briefing` (`--weekly`, `--since`) | Verified | Live `watch add` + `watch run` + `briefing` generated structured daily digest from SQLite; MCP tools `radar30_discover`, `radar30_library_search`, and `radar30_briefing` tested in `tests/wave2_workflows.rs`. |
