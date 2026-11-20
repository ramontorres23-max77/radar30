# Changelog

## 0.5.1
- Patent digest: anonymous USPTO session fallback and 24h local cache when Google Patents refuses requests; cached items are labeled `[from 24h cache]`.
- Added GitHub Actions CI (fmt, clippy, tests, release build, secret scan) and a tagged-release workflow producing macOS (arm64/x86_64), Linux and Windows binaries.
- Added `scripts/build-macos-app.sh`: a double-clickable `Radar30.app` launcher (menu for search, company intel, doctor) with ad-hoc codesign, also attached to macOS release artifacts.

## 0.5.0
- Wave 4: `radar30 company` (US patent publications, job signals, stock vs S&P 500 proxy), `radar30 publish`, and the `radar30_company` MCP tool (8 tools total).

## 0.4.0
- Optional local `serve-http` transport (`GET /health`, `POST /mcp`). Docker/Compose scaffolding was added for a planned deploy and then removed when the project was declared local-only.

## 0.3.0
- Wave 3: credential chain (env → `.env` → macOS Keychain), bounded subprocesses, and 10 BYOK source lanes (X, YouTube, Bluesky, ScrapeCreators socials, Perplexity, Brave).

## 0.2.0
- Wave 2: bundled SQLite store with FTS5, watchlist, briefings, topic-less discovery, offline library search, Atom feed, and 3 additional MCP tools.

## 0.1.0
- Wave 1: pure-Rust CLI, zero-SDK stdio MCP server, keyless source adapters, RRF fusion, clustering, `setup`, `doctor`, and embedded `SKILL.md`.
