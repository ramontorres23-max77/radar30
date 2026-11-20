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

## Wave 3 — BYOK Social Lanes & Secure Credentials (Done)
- Credential chain (`src/creds.rs`): process env → `./.env` → `~/.config/radar30/.env` → macOS Keychain; `radar30 creds` prints presence/origin only, `creds set <KEY> --stdin` writes via `security -i` so secrets never enter argv
- Bounded subprocess discipline (`src/process.rs`) for `yt-dlp` and `security`: own process group, stdin control, wall-clock kill
- 10 new source lanes: Bluesky (AT Protocol, authenticated AppView or public mirror), X (official X API v2 recent search), YouTube (`yt-dlp` flat search), 5 ScrapeCreators socials, Perplexity Sonar, Brave Web Search
- Unconfigured BYOK lanes report `unavailable` with an exact fix hint instead of failing the run; `doctor`, `preflight`, and `creds` all report readiness
- Parser/unit tests for every new adapter; `SourceState::Unavailable` added to `radar30/research@1` telemetry

## Wave 4 — Company Intelligence & Publishing
- USPTO Patent Digest (First US publications)
- Careers / Jobs Page Secret Signals
- Yahoo Finance 30-day stock vs S&P 500 comparison
- Static HTML brief publisher
