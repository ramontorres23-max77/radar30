# Multi-Wave Plan

Roadmap for `radar30`, combining `rust-anything`'s single-binary Rust architecture with `last30days-skill`'s people-scored multi-source research engine.

## Wave 1 — Keyless Core + CLI + MCP + Setup/Doctor (Done)
- Pure-Rust crate (`#![forbid(unsafe_code)]`, `clippy -D warnings`, release LTO/strip profile)
- 6 keyless sources: Reddit (shreddit + Arctic Shift fallback), Hacker News (Algolia), GitHub (REST Search), Polymarket (Gamma), DEV.to (Forem, opt-in), arXiv (native Atom XML)
- Parallel fan-out with wall-clock deadline and typed degradation states
- Query planning (primary entity, intent modifier stripping, comparison split), entity grounding, recency decay, cross-platform log1p engagement, Weighted RRF fusion, greedy + MMR clustering
- Renderers: `compact` (with canonical boundary), `md`, and versioned `json` (`radar30/research@1`)
- `preflight` (zero-network/zero-write), `sources`, `doctor` (with live MCP self-test), `setup` (atomic + `.bak` + `--dry-run`/`--remove`), `mcp` (JSON-RPC 2.0, threaded calls, progress, cancellation, embedded prompts)
- Offline fixture test suite, MCP contract + `docs/tools.md` drift test, `scripts/check-secrets.sh`

## Wave 2 — Persistence, Library & Discovery (Done)
- Bundled SQLite store (`--store`, WAL + FTS5 `porter unicode61` + `topic_queue`) for accumulating sightings across runs
- `radar30 library search` (offline deterministic briefs + FTS5 merge), `radar30 library feed` (`index.html`, `briefs/*.html`, Atom `feed.xml`), and `radar30 library queue --cover`
- Topic-less `radar30 discover [domain]` mode (HN + Reddit river sweeps, junk-shape filter, confidence floor, honest `nothing-solid` outcome, repeat annotations)
- `radar30 watch` (`add`/`remove`/`list`/`run`) & `radar30 briefing [--weekly]`
- MCP tools expanded to 7 (`radar30_discover`, `radar30_library_search`, `radar30_briefing`) + updated `SKILL.md` and `docs/tools.md`

## Wave 3 — BYOK Social Lanes & Secure Credentials
- macOS Keychain + `.env` credential resolution (lowest-priority Keychain fallback)
- X/Twitter lane (official X API / xAI / Xquik with explicit backend chain)
- YouTube (`yt-dlp` transcript extraction), Bluesky (AT Protocol), ScrapeCreators (TikTok, Instagram Reels, Threads, LinkedIn, Pinterest), Perplexity & Brave Web Search

## Wave 4 — Company Intelligence & Publishing
- USPTO Patent Digest (First US publications)
- Careers / Jobs Page Secret Signals
- Yahoo Finance 30-day stock vs S&P 500 comparison
- Static HTML brief publisher
