# Pitfalls

Mistakes and upstream quirks documented as concrete rules so they are never repeated.

## Upstream APIs

- **HN Algolia rejects `points>N` in `numericFilters` with HTTP 400.** Only `created_at_i` is filterable server-side. Always overfetch and filter low-point stories client-side in `src/sources/hackernews.rs`.
- **Reddit anonymous `.json` endpoints return HTTP 403, and non-browser TLS clients may also get HTTP 403 on `/svc/shreddit/search/`.** Meanwhile Arctic Shift's `/api/posts/search?title=...` returns HTTP 400 unless `subreddit` or `author` is provided. `src/sources/reddit.rs` tries shreddit search first and falls back to Arctic Shift subreddit-prefix discovery (`/api/subreddits/search`) + subreddit post listing (`/api/posts/search?subreddit=...`).
- **arXiv quoted exact phrases miss multi-word natural queries.** Retry with `AND`-joined `all:"term"` clauses when the quoted phrase returns zero entries, and enforce a 365-day recency cutoff.

## MCP & CLI

- **Never write non-JSON logs to stdout in `radar30 mcp`.** All diagnostics go to stderr; stdout belongs exclusively to the MCP writer thread.
- **Never spawn a recursive MCP self-test inside `radar30_doctor` MCP tool.** CLI `radar30 doctor` spawns `<exe> mcp` for the live self-test; MCP `radar30_doctor` calls `doctor::run_without_mcp_selftest()`.
- **Keep `docs/tools.md` synchronized with `src/mcp.rs::tools()`.** `tests/mcp_contract.rs` fails if they drift.
