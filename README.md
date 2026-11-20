# radar30

**Multi-source, engagement-scored research engine over the last 30 days — built as a single pure-Rust binary (CLI + stdio MCP server + Agent Skill).**

Inspired by the multi-source people-scored research pipeline of [`last30days-skill`](https://github.com/mvanhorn/last30days-skill) and engineered following the single-binary Rust CLI/MCP/setup/doctor architecture of [`rust-anything`](https://github.com/erkamyaman/rust-anything).

## Quick start

```bash
cargo build --release
./target/release/radar30 preflight "Rust"
./target/release/radar30 research "Rust" --depth quick --emit compact
./target/release/radar30 doctor
```

Register with local coding agents (Claude Code, Codex, Cursor, Gemini CLI, OpenCode, Hermes):

```bash
./target/release/radar30 setup --dry-run
./target/release/radar30 setup
```

## Wave 1 Sources (Keyless)

| Source | Endpoint | Default | Notes |
| --- | --- | --- | --- |
| **Reddit** | Arctic Shift (`/api/posts/search`) | Yes | Keyless public archive with scores and timestamps |
| **Hacker News** | Algolia HN API (`/api/v1/search`) | Yes | Overfetched + client-side min-points filter |
| **GitHub** | REST Search (`/search/issues`) | Yes | Keyless; honors optional `GITHUB_TOKEN` |
| **Polymarket** | Gamma API (`/public-search`) | Yes | Live markets, leading odds, USD volume |
| **arXiv** | Atom API (`export.arxiv.org`) | Yes | Native Rust XML parser, 365-day recency window |
| **DEV.to** | Forem API (`/api/articles`) | Opt-in | Enable with `--sources ...,devto` |

## Commands

- `radar30 research <topic>` — Parallel fetch, entity grounding, Weighted RRF fusion, clustering, rendering (`--emit compact|md|json`), optional `--store` (SQLite) and `--save-dir`
- `radar30 discover [domain]` — Topic-less discovery across HN & Reddit rivers with junk filtering, confidence floor, honest `nothing-solid` outcome, and repeat tracking via `topic_queue`
- `radar30 library search|feed|queue` — Offline FTS5 + Markdown brief search, static HTML + Atom `feed.xml` builder, and discovery queue (`--cover`)
- `radar30 watch add|remove|list|run` — Recurring watchlist topics and batch runs persisted into SQLite
- `radar30 briefing [--weekly]` — Daily / weekly briefings aggregated from stored findings
- `radar30 preflight [topic]` — Zero-network, zero-write forecast of sources, endpoints, query plan, and writes
- `radar30 sources` — List all supported sources and auth modes
- `radar30 doctor` — Check binary health, optional auth env, local agent setup, and run a live MCP stdio self-test
- `radar30 setup` — Atomically register `radar30 mcp` and embedded `SKILL.md` with `.radar30.bak` backups (`--dry-run`, `--remove`)
- `radar30 mcp` — Start the stdio JSON-RPC 2.0 MCP server (`2025-06-18`)

## Documentation

- [Multi-wave plan](docs/PLAN.md)
- [Status](docs/STATUS.md)
- [MCP tools](docs/tools.md)
- [Decisions](DECISIONS.md)
- [Pitfalls](docs/pitfalls.md)

## License

MIT © 2026 radar30 contributors
